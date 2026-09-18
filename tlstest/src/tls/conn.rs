//! One TLS connection: the socket, the record layer, and a trace of everything that
//! crossed it.
//!
//! [`TlsConn`] deliberately separates the three layers a learner has to keep apart —
//! TCP bytes, records, and handshake messages — because almost every early bug lives in
//! the joins between them. A record can hold two handshake messages; a handshake message
//! can span three records; a `ChangeCipherSpec` record is not part of the handshake at all
//! and never reaches the transcript. [`TlsConn::next_message`] does all of that and leaves
//! a [`TraceEntry`] behind for each step, which is what a failing test prints.

use super::msg::{Alert, HandshakeMessage};
use super::record::{Record, RecordLayer};
use super::{hex_prefix, ContentType, TlsError, TlsResult};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Which way a traced item went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The client wrote it.
    Sent,
    /// The server wrote it.
    Received,
}

impl Direction {
    /// The arrow shown in a report.
    pub fn arrow(self) -> &'static str {
        match self {
            Direction::Sent => "-->",
            Direction::Received => "<--",
        }
    }
}

/// One line of the connection's history.
#[derive(Debug, Clone)]
pub struct TraceEntry {
    /// Which way it went.
    pub direction: Direction,
    /// What it was: `record handshake(22)`, `client_hello(1)`, `alert`, ...
    pub what: String,
    /// The bytes, as they were on the wire for a record and after decryption for a message.
    pub bytes: Vec<u8>,
    /// Whether the record carrying it was protected.
    pub encrypted: bool,
}

impl TraceEntry {
    /// The one-line form used in a failure block.
    pub fn line(&self) -> String {
        format!(
            "{} {:<34} {:>5} bytes  {}{}",
            self.direction.arrow(),
            self.what,
            self.bytes.len(),
            if self.encrypted { "[encrypted] " } else { "" },
            hex_prefix(&self.bytes, 12)
        )
    }
}

/// What came off the connection next.
#[derive(Debug, Clone)]
pub enum Incoming {
    /// A complete handshake message, reassembled across records if it had to be.
    Handshake(HandshakeMessage),
    /// An alert.
    Alert(Alert),
    /// Application data.
    AppData(Vec<u8>),
    /// A `ChangeCipherSpec` record, which TLS 1.3 keeps only for middleboxes.
    ChangeCipherSpec(Record),
    /// A record whose content type does not belong in a TLS 1.3 stream.
    Unexpected(Record),
}

impl Incoming {
    /// A short description for a report.
    pub fn describe(&self) -> String {
        match self {
            Incoming::Handshake(m) => format!("handshake {}", m.name()),
            Incoming::Alert(a) => format!("alert {}", a.describe()),
            Incoming::AppData(d) => format!("application_data, {} bytes", d.len()),
            Incoming::ChangeCipherSpec(_) => "change_cipher_spec".to_string(),
            Incoming::Unexpected(r) => format!("a {} record", r.content_type.name()),
        }
    }
}

/// One TCP connection speaking TLS.
pub struct TlsConn {
    stream: TcpStream,
    /// The address this connection talks to.
    pub addr: SocketAddr,
    /// How long to wait for the server before giving up.
    pub timeout: Duration,
    /// The keys and sequence numbers in force.
    pub layer: RecordLayer,
    /// Raw bytes read from the socket but not yet framed into records.
    inbuf: Vec<u8>,
    /// Handshake bytes reassembled across records but not yet a complete message.
    hs_buf: Vec<u8>,
    /// Application data read out of order with a handshake message.
    app_buf: Vec<Vec<u8>>,
    /// Everything that crossed the connection, in order.
    pub trace: Vec<TraceEntry>,
    /// Every record the server sent, in order, raw.
    pub records_in: Vec<Record>,
    /// The last alert the server sent, if any.
    pub last_alert: Option<Alert>,
    /// True once the server has closed its write side.
    pub server_closed: bool,
}

impl TlsConn {
    /// Open a TCP connection with no TLS state yet.
    pub async fn connect(addr: SocketAddr, timeout: Duration) -> TlsResult<TlsConn> {
        let stream = tokio::time::timeout(timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| TlsError::Timeout(format!("a TCP connection to {addr}")))??;
        stream.set_nodelay(true)?;
        Ok(TlsConn {
            stream,
            addr,
            timeout,
            layer: RecordLayer::new(),
            inbuf: Vec::new(),
            hs_buf: Vec::new(),
            app_buf: Vec::new(),
            trace: Vec::new(),
            records_in: Vec::new(),
            last_alert: None,
            server_closed: false,
        })
    }

    fn trace(&mut self, direction: Direction, what: impl Into<String>, bytes: &[u8], enc: bool) {
        self.trace.push(TraceEntry {
            direction,
            what: what.into(),
            bytes: bytes.to_vec(),
            encrypted: enc,
        });
    }

    /// Add a note to the trace without any bytes, so a report can say what the test did.
    pub fn note(&mut self, what: impl Into<String>) {
        self.trace.push(TraceEntry {
            direction: Direction::Sent,
            what: what.into(),
            bytes: Vec::new(),
            encrypted: false,
        });
    }

    // -----------------------------------------------------------------------------------
    // Writing
    // -----------------------------------------------------------------------------------

    /// Write bytes exactly as given: no framing, no encryption, no checks.
    pub async fn write_raw(&mut self, bytes: &[u8]) -> TlsResult<()> {
        self.stream.write_all(bytes).await?;
        self.stream.flush().await?;
        self.trace(Direction::Sent, "raw bytes", bytes, false);
        Ok(())
    }

    /// Write one record, encrypting it when write keys are in force.
    pub async fn write_record(&mut self, content_type: ContentType, content: &[u8]) -> TlsResult<()> {
        self.write_record_padded(content_type, content, 0).await
    }

    /// Write one record with `padding` zero bytes inside the `TLSInnerPlaintext`.
    pub async fn write_record_padded(
        &mut self,
        content_type: ContentType,
        content: &[u8],
        padding: usize,
    ) -> TlsResult<()> {
        let encrypted = self.layer.writing_encrypted();
        let bytes = self.layer.seal(content_type, content, padding)?;
        self.stream.write_all(&bytes).await?;
        self.stream.flush().await?;
        let what = format!("record {}", content_type.name());
        self.trace(Direction::Sent, what, &bytes, encrypted);
        Ok(())
    }

    /// Write one handshake message, in whatever protection is in force.
    pub async fn write_handshake(&mut self, message: &[u8]) -> TlsResult<()> {
        let name = HandshakeMessage::parse(message)
            .map(|(m, _)| m.name())
            .unwrap_or_else(|_| "handshake (unparsable)".to_string());
        let encrypted = self.layer.writing_encrypted();
        let bytes = self.layer.seal(ContentType::Handshake, message, 0)?;
        self.stream.write_all(&bytes).await?;
        self.stream.flush().await?;
        self.trace(Direction::Sent, name, message, encrypted);
        Ok(())
    }

    /// Write application data.
    pub async fn write_app_data(&mut self, data: &[u8]) -> TlsResult<()> {
        self.write_record(ContentType::ApplicationData, data).await
    }

    /// Write the `ChangeCipherSpec` compatibility record: one byte, 0x01, never encrypted.
    pub async fn write_change_cipher_spec(&mut self) -> TlsResult<()> {
        let bytes = Record::build(
            ContentType::ChangeCipherSpec.as_u8(),
            super::LEGACY_VERSION_TLS12,
            &[1],
        );
        self.stream.write_all(&bytes).await?;
        self.stream.flush().await?;
        self.trace(Direction::Sent, "record change_cipher_spec(20)", &bytes, false);
        Ok(())
    }

    /// Write an alert.
    pub async fn write_alert(&mut self, alert: Alert) -> TlsResult<()> {
        self.write_record(ContentType::Alert, &alert.encode()).await
    }

    /// Shut the write half down, leaving the read half open.
    pub async fn shutdown_write(&mut self) -> TlsResult<()> {
        self.stream.shutdown().await?;
        self.trace(Direction::Sent, "TCP FIN (write half closed)", &[], false);
        Ok(())
    }

    // -----------------------------------------------------------------------------------
    // Reading
    // -----------------------------------------------------------------------------------

    /// Pull more bytes off the socket into the buffer; `false` when the peer closed.
    async fn fill(&mut self, timeout: Duration, what: &str) -> TlsResult<bool> {
        let mut chunk = [0u8; 16384];
        match tokio::time::timeout(timeout, self.stream.read(&mut chunk)).await {
            Err(_) => Err(TlsError::Timeout(what.to_string())),
            Ok(Ok(0)) => {
                self.server_closed = true;
                Ok(false)
            }
            Ok(Ok(n)) => {
                self.inbuf.extend_from_slice(&chunk[..n]);
                Ok(true)
            }
            Ok(Err(e)) if is_reset(&e) => {
                self.server_closed = true;
                Ok(false)
            }
            Ok(Err(e)) => Err(TlsError::Io(e)),
        }
    }

    /// Read one record off the wire, without decrypting it.
    pub async fn read_record(&mut self) -> TlsResult<Record> {
        let timeout = self.timeout;
        self.read_record_within(timeout).await
    }

    /// Read one record with an explicit deadline.
    pub async fn read_record_within(&mut self, timeout: Duration) -> TlsResult<Record> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.inbuf.len() >= 5 {
                let length = u16::from_be_bytes([self.inbuf[3], self.inbuf[4]]) as usize;
                if self.inbuf.len() >= 5 + length {
                    let (record, used) = Record::parse(&self.inbuf)?;
                    self.inbuf.drain(..used);
                    let enc = record.content_type == ContentType::ApplicationData
                        && self.layer.reading_encrypted();
                    let what = format!("record {}", record.content_type.name());
                    self.trace(Direction::Received, what, &record.raw, enc);
                    self.records_in.push(record.clone());
                    return Ok(record);
                }
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Err(TlsError::Timeout("a record from the server".into()));
            }
            if !self.fill(left, "a record from the server").await? {
                return Err(TlsError::Closed);
            }
        }
    }

    /// The next handshake message, alert or chunk of application data.
    ///
    /// Reassembles handshake messages across records and splits records that carry more
    /// than one, which is what makes stages 5 and 6 possible to write as tests rather than
    /// as hope.
    pub async fn next_message(&mut self) -> TlsResult<Incoming> {
        let timeout = self.timeout;
        self.next_message_within(timeout).await
    }

    /// [`TlsConn::next_message`] with an explicit deadline.
    pub async fn next_message_within(&mut self, timeout: Duration) -> TlsResult<Incoming> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(message) = self.pop_handshake()? {
                return Ok(Incoming::Handshake(message));
            }
            if !self.app_buf.is_empty() {
                return Ok(Incoming::AppData(self.app_buf.remove(0)));
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Err(TlsError::Timeout("the next TLS message".into()));
            }
            let record = self.read_record_within(left).await?;
            match record.content_type {
                // A ChangeCipherSpec record is never encrypted and never reaches the
                // transcript, whether or not keys are already in force (RFC 8446 §5).
                ContentType::ChangeCipherSpec => return Ok(Incoming::ChangeCipherSpec(record)),
                ContentType::ApplicationData if self.layer.reading_encrypted() => {
                    let inner = self.layer.open(&record)?;
                    let what = format!("decrypted {}", inner.content_type.name());
                    self.trace(Direction::Received, what, &inner.content, true);
                    match inner.content_type {
                        ContentType::Handshake => self.hs_buf.extend_from_slice(&inner.content),
                        ContentType::Alert => {
                            let alert = Alert::parse(&inner.content)?;
                            self.last_alert = Some(alert);
                            return Ok(Incoming::Alert(alert));
                        }
                        ContentType::ApplicationData => {
                            self.app_buf.push(inner.content);
                        }
                        _ => return Ok(Incoming::Unexpected(record)),
                    }
                }
                ContentType::Handshake if !self.layer.reading_encrypted() => {
                    self.hs_buf.extend_from_slice(&record.fragment);
                }
                ContentType::Alert if !self.layer.reading_encrypted() => {
                    let alert = Alert::parse(&record.fragment)?;
                    self.last_alert = Some(alert);
                    return Ok(Incoming::Alert(alert));
                }
                _ => return Ok(Incoming::Unexpected(record)),
            }
        }
    }

    fn pop_handshake(&mut self) -> TlsResult<Option<HandshakeMessage>> {
        if self.hs_buf.len() < 4 {
            return Ok(None);
        }
        let length = u32::from_be_bytes([0, self.hs_buf[1], self.hs_buf[2], self.hs_buf[3]]) as usize;
        if self.hs_buf.len() < 4 + length {
            return Ok(None);
        }
        let (message, used) = HandshakeMessage::parse(&self.hs_buf)?;
        self.hs_buf.drain(..used);
        Ok(Some(message))
    }

    /// True when handshake bytes are buffered that do not yet make a whole message.
    pub fn has_partial_handshake(&self) -> bool {
        !self.hs_buf.is_empty()
    }

    /// Application data already read while waiting for something else.
    pub fn buffered_app_data(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.app_buf)
    }

    /// Read until the server closes, returning everything it said first.
    pub async fn expect_closed(&mut self, timeout: Duration) -> TlsResult<Vec<u8>> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut seen = std::mem::take(&mut self.inbuf);
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                self.inbuf = seen;
                return Err(TlsError::Timeout(
                    "the server to close the connection".into(),
                ));
            }
            let before = self.inbuf.len();
            if !self.fill(left, "the server to close the connection").await? {
                seen.extend_from_slice(&self.inbuf);
                self.inbuf.clear();
                self.trace(Direction::Received, "TCP FIN (server closed)", &[], false);
                return Ok(seen);
            }
            seen.extend_from_slice(&self.inbuf[before..]);
            self.inbuf.truncate(before);
        }
    }

    /// Prove nothing arrives within `quiet`; returns whatever did arrive.
    pub async fn read_silence(&mut self, quiet: Duration) -> TlsResult<Vec<u8>> {
        let mut chunk = [0u8; 4096];
        match tokio::time::timeout(quiet, self.stream.read(&mut chunk)).await {
            Err(_) => Ok(Vec::new()),
            Ok(Ok(0)) => {
                self.server_closed = true;
                Err(TlsError::Closed)
            }
            Ok(Ok(n)) => {
                let seen = chunk[..n].to_vec();
                self.inbuf.extend_from_slice(&seen);
                Ok(seen)
            }
            Ok(Err(e)) if is_reset(&e) => {
                self.server_closed = true;
                Err(TlsError::Closed)
            }
            Ok(Err(e)) => Err(TlsError::Io(e)),
        }
    }

    /// Whatever the server does next — an alert, a close, or nothing — described in words.
    ///
    /// This is the shape almost every robustness test wants: RFC 8446 lets a server answer
    /// a broken flight with an alert *or* simply drop the connection, so a test that
    /// demanded one of them would be wrong half the time.
    pub async fn reaction(&mut self, timeout: Duration) -> Reaction {
        match self.next_message_within(timeout).await {
            Ok(Incoming::Alert(alert)) => Reaction::Alert(alert),
            Ok(other) => Reaction::Message(other.describe()),
            Err(TlsError::Closed) => Reaction::Closed,
            Err(TlsError::Timeout(_)) => Reaction::Silence,
            Err(e) => Reaction::Error(e.to_string()),
        }
    }

    /// The trace as report lines.
    pub fn trace_lines(&self) -> Vec<String> {
        self.trace.iter().map(TraceEntry::line).collect()
    }

    /// The bytes of the last record the server sent, for a hex block.
    pub fn last_record_bytes(&self) -> Option<Vec<u8>> {
        self.records_in.last().map(|r| r.raw.clone())
    }
}

/// What a server did when it was sent something it should refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reaction {
    /// It sent an alert.
    Alert(Alert),
    /// It closed the connection without saying anything.
    Closed,
    /// It said nothing at all and left the connection open.
    Silence,
    /// It answered with a normal TLS message.
    Message(String),
    /// Something else went wrong while listening.
    Error(String),
}

impl Reaction {
    /// How this reads in a report.
    pub fn describe(&self) -> String {
        match self {
            Reaction::Alert(a) => format!("alert {}", a.describe()),
            Reaction::Closed => "the connection was closed".to_string(),
            Reaction::Silence => "nothing at all, connection left open".to_string(),
            Reaction::Message(m) => format!("a normal message: {m}"),
            Reaction::Error(e) => format!("an error: {e}"),
        }
    }

    /// True when the server refused: an alert, or a close. Both are allowed by RFC 8446.
    pub fn is_refusal(&self) -> bool {
        matches!(self, Reaction::Alert(_) | Reaction::Closed)
    }

    /// True when the server refused with this specific alert, or closed without one.
    pub fn is_refusal_with(&self, description: super::AlertDescription) -> bool {
        match self {
            Reaction::Alert(a) => a.description == description,
            Reaction::Closed => true,
            _ => false,
        }
    }

    /// The alert, when there was one.
    pub fn alert(&self) -> Option<Alert> {
        match self {
            Reaction::Alert(a) => Some(*a),
            _ => None,
        }
    }
}

fn is_reset(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::crypto::{Suite, TrafficKeys};
    use crate::tls::msg::encode_handshake;
    use crate::tls::HandshakeType;
    use tokio::net::TcpListener;

    fn suite() -> Suite {
        Suite::from_code(crate::tls::TLS_AES_128_GCM_SHA256).expect("suite")
    }

    /// A listener that writes `script` and then closes.
    async fn scripted(script: Vec<u8>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let _ = socket.write_all(&script).await;
                let _ = socket.shutdown().await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn a_handshake_message_split_across_records_is_reassembled() {
        let message = encode_handshake(HandshakeType::SERVER_HELLO, &[7u8; 100]);
        let mut script = Record::build(22, 0x0303, &message[..10]);
        script.extend_from_slice(&Record::build(22, 0x0303, &message[10..60]));
        script.extend_from_slice(&Record::build(22, 0x0303, &message[60..]));
        let addr = scripted(script).await;
        let mut conn = TlsConn::connect(addr, Duration::from_secs(2))
            .await
            .expect("connect");
        match conn.next_message().await.expect("message") {
            Incoming::Handshake(m) => {
                assert_eq!(m.msg_type, HandshakeType::SERVER_HELLO);
                assert_eq!(m.body.len(), 100);
                assert_eq!(m.raw, message);
            }
            other => panic!("expected a handshake message, got {}", other.describe()),
        }
    }

    #[tokio::test]
    async fn two_messages_in_one_record_come_out_separately() {
        let mut fragment = encode_handshake(HandshakeType::ENCRYPTED_EXTENSIONS, &[0, 0]);
        fragment.extend_from_slice(&encode_handshake(HandshakeType::FINISHED, &[1u8; 32]));
        let addr = scripted(Record::build(22, 0x0303, &fragment)).await;
        let mut conn = TlsConn::connect(addr, Duration::from_secs(2))
            .await
            .expect("connect");
        let first = conn.next_message().await.expect("first");
        let second = conn.next_message().await.expect("second");
        assert!(matches!(
            first,
            Incoming::Handshake(ref m) if m.msg_type == HandshakeType::ENCRYPTED_EXTENSIONS
        ));
        assert!(matches!(
            second,
            Incoming::Handshake(ref m) if m.msg_type == HandshakeType::FINISHED
        ));
    }

    #[tokio::test]
    async fn an_encrypted_record_is_opened_and_its_inner_type_used() {
        let keys = TrafficKeys::derive(suite(), &[3u8; 32]).expect("keys");
        let mut writer = RecordLayer::new();
        writer.set_write(keys.clone());
        let message = encode_handshake(HandshakeType::FINISHED, &[9u8; 32]);
        let script = writer
            .seal(ContentType::Handshake, &message, 5)
            .expect("seal");
        let addr = scripted(script).await;
        let mut conn = TlsConn::connect(addr, Duration::from_secs(2))
            .await
            .expect("connect");
        conn.layer.set_read(keys);
        match conn.next_message().await.expect("message") {
            Incoming::Handshake(m) => assert_eq!(m.msg_type, HandshakeType::FINISHED),
            other => panic!("expected Finished, got {}", other.describe()),
        }
        assert_eq!(conn.layer.read_seq, 1);
    }

    #[tokio::test]
    async fn a_plaintext_alert_is_reported_as_an_alert() {
        let addr = scripted(Record::build(21, 0x0303, &[2, 70])).await;
        let mut conn = TlsConn::connect(addr, Duration::from_secs(2))
            .await
            .expect("connect");
        match conn.next_message().await.expect("message") {
            Incoming::Alert(a) => {
                assert_eq!(a.description, crate::tls::AlertDescription::PROTOCOL_VERSION)
            }
            other => panic!("expected an alert, got {}", other.describe()),
        }
        assert!(conn.last_alert.is_some());
    }

    #[tokio::test]
    async fn a_server_that_says_nothing_reads_as_silence_then_close() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let held = listener.accept().await;
            tokio::time::sleep(Duration::from_millis(400)).await;
            drop(held);
        });
        let mut conn = TlsConn::connect(addr, Duration::from_secs(2))
            .await
            .expect("connect");
        assert_eq!(
            conn.reaction(Duration::from_millis(100)).await,
            Reaction::Silence
        );
        assert_eq!(conn.reaction(Duration::from_secs(2)).await, Reaction::Closed);
    }
}
