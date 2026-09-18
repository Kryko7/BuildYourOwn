//! A thin async Kafka client: framed request/response, raw-byte mode, and hex dumps.
//!
//! Every test talks to the broker through [`Conn`]. Typed requests go through
//! [`Conn::request`], which encodes with `kafka-protocol`, checks the correlation id and
//! decodes the response; framing and malformed-input tests use [`Conn::send_bytes`] and
//! [`Conn::read_frame`] directly. The last request and response bytes stay on the
//! connection so a failing assertion can hex-dump them.

pub mod meta;
pub mod records;

use bytes::{Bytes, BytesMut};
use kafka_protocol::messages::{RequestHeader, ResponseHeader};
use kafka_protocol::protocol::{Decodable, Encodable, HeaderVersion, Request, StrBytes};
use std::fmt;
use std::net::SocketAddr;
use std::ops::Range;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Largest response frame the harness will read; anything bigger is a protocol error.
pub const MAX_FRAME: usize = 100 * 1024 * 1024;

/// Everything that can go wrong while talking to a broker.
#[derive(Debug)]
pub enum ProtoError {
    /// The peer closed the connection cleanly (or half-closed it) before sending enough bytes.
    Closed,
    /// Nothing arrived in time; the string says what was being waited for.
    Timeout(String),
    /// A socket error.
    Io(std::io::Error),
    /// The bytes on the wire are not a valid frame.
    Frame(String),
    /// The frame decoded, but its contents did not.
    Decode(String),
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtoError::Closed => write!(f, "the broker closed the connection"),
            ProtoError::Timeout(what) => write!(f, "timed out waiting for {what}"),
            ProtoError::Io(e) => write!(f, "socket error: {e}"),
            ProtoError::Frame(m) => write!(f, "bad frame: {m}"),
            ProtoError::Decode(m) => write!(f, "cannot decode: {m}"),
        }
    }
}

impl std::error::Error for ProtoError {}

impl From<std::io::Error> for ProtoError {
    fn from(e: std::io::Error) -> Self {
        ProtoError::Io(e)
    }
}

/// Result of a protocol operation.
pub type ProtoResult<T> = Result<T, ProtoError>;

/// A decoded response together with the bytes it came from.
#[derive(Debug, Clone)]
pub struct Decoded<T> {
    /// The decoded response body.
    pub body: T,
    /// The correlation id from the response header.
    pub correlation_id: i32,
    /// The full response frame, without the 4-byte length prefix.
    pub raw: Vec<u8>,
    /// Bytes left over after decoding the body (should always be 0).
    pub trailing: usize,
}

impl<T> std::ops::Deref for Decoded<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.body
    }
}

/// Encode a request frame payload (header + body), without the length prefix.
pub fn encode_request<R: Request>(
    version: i16,
    correlation_id: i32,
    client_id: Option<&str>,
    req: &R,
) -> ProtoResult<Vec<u8>> {
    let mut header = RequestHeader::default();
    header.request_api_key = R::KEY;
    header.request_api_version = version;
    header.correlation_id = correlation_id;
    header.client_id = client_id.map(|s| StrBytes::from_string(s.to_string()));
    let mut buf = BytesMut::new();
    header
        .encode(&mut buf, R::header_version(version))
        .map_err(|e| ProtoError::Decode(format!("cannot encode request header: {e}")))?;
    req.encode(&mut buf, version)
        .map_err(|e| ProtoError::Decode(format!("cannot encode request body: {e}")))?;
    Ok(buf.to_vec())
}

/// Decode a response frame payload (header + body) for the given request type.
pub fn decode_response<R: Request>(
    version: i16,
    payload: &[u8],
) -> ProtoResult<Decoded<R::Response>> {
    let mut buf = Bytes::copy_from_slice(payload);
    let header = ResponseHeader::decode(&mut buf, R::Response::header_version(version))
        .map_err(|e| ProtoError::Decode(format!("response header: {e}")))?;
    let body = R::Response::decode(&mut buf, version)
        .map_err(|e| ProtoError::Decode(format!("response body (v{version}): {e}")))?;
    Ok(Decoded {
        body,
        correlation_id: header.correlation_id,
        raw: payload.to_vec(),
        trailing: buf.len(),
    })
}

/// One TCP connection to a broker.
pub struct Conn {
    stream: TcpStream,
    /// The address this connection talks to.
    pub addr: SocketAddr,
    next_id: i32,
    /// `client_id` put into every request header; `None` sends a null client id.
    pub client_id: Option<String>,
    /// How long to wait for a response before giving up.
    pub timeout: Duration,
    /// The last request frame written (without the length prefix).
    pub last_request: Option<Vec<u8>>,
    /// The last response frame read (without the length prefix).
    pub last_response: Option<Vec<u8>>,
}

impl Conn {
    /// Open a connection, failing with [`ProtoError::Timeout`] if the broker does not accept.
    pub async fn connect(addr: SocketAddr, timeout: Duration) -> ProtoResult<Conn> {
        let stream = tokio::time::timeout(timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| ProtoError::Timeout(format!("a TCP connection to {addr}")))??;
        stream.set_nodelay(true)?;
        Ok(Conn {
            stream,
            addr,
            next_id: 1,
            client_id: Some("kafkatest".to_string()),
            timeout,
            last_request: None,
            last_response: None,
        })
    }

    /// The correlation id the next [`Conn::request`] will use.
    pub fn peek_correlation_id(&self) -> i32 {
        self.next_id
    }

    /// Take the next correlation id.
    pub fn next_correlation_id(&mut self) -> i32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Send a typed request and decode the matching response.
    ///
    /// Fails when the correlation id does not come back unchanged — the single most common
    /// bug in a young broker, and the reason the message names the field path.
    pub async fn request<R: Request>(
        &mut self,
        version: i16,
        req: &R,
    ) -> ProtoResult<Decoded<R::Response>> {
        let id = self.next_correlation_id();
        self.request_with_id(version, id, req).await
    }

    /// Like [`Conn::request`] but with an explicit correlation id.
    pub async fn request_with_id<R: Request>(
        &mut self,
        version: i16,
        correlation_id: i32,
        req: &R,
    ) -> ProtoResult<Decoded<R::Response>> {
        let payload = encode_request(version, correlation_id, self.client_id.as_deref(), req)?;
        self.send_frame(&payload).await?;
        let resp = self.read_frame().await?;
        let decoded = decode_response::<R>(version, &resp)?;
        if decoded.correlation_id != correlation_id {
            return Err(ProtoError::Decode(format!(
                "response.correlation_id: expected {correlation_id}, got {}",
                decoded.correlation_id
            )));
        }
        Ok(decoded)
    }

    /// Send a typed request without reading the response (for pipelining tests).
    pub async fn send_request<R: Request>(
        &mut self,
        version: i16,
        correlation_id: i32,
        req: &R,
    ) -> ProtoResult<Vec<u8>> {
        let payload = encode_request(version, correlation_id, self.client_id.as_deref(), req)?;
        self.send_frame(&payload).await?;
        Ok(payload)
    }

    /// Write a length-prefixed frame.
    pub async fn send_frame(&mut self, payload: &[u8]) -> ProtoResult<()> {
        let mut frame = Vec::with_capacity(payload.len() + 4);
        frame.extend_from_slice(&(payload.len() as i32).to_be_bytes());
        frame.extend_from_slice(payload);
        self.last_request = Some(payload.to_vec());
        self.send_bytes(&frame).await
    }

    /// Write arbitrary bytes, exactly as given (framing tests).
    pub async fn send_bytes(&mut self, bytes: &[u8]) -> ProtoResult<()> {
        self.stream.write_all(bytes).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Shut the write half down, leaving the read half open (half-close tests).
    pub async fn shutdown_write(&mut self) -> ProtoResult<()> {
        self.stream.shutdown().await?;
        Ok(())
    }

    /// Read one length-prefixed frame and return its payload.
    pub async fn read_frame(&mut self) -> ProtoResult<Vec<u8>> {
        let timeout = self.timeout;
        self.read_frame_within(timeout).await
    }

    /// Read one frame with an explicit deadline.
    pub async fn read_frame_within(&mut self, timeout: Duration) -> ProtoResult<Vec<u8>> {
        let mut len = [0u8; 4];
        read_exact_within(
            &mut self.stream,
            &mut len,
            timeout,
            "a response length prefix",
        )
        .await?;
        let size = i32::from_be_bytes(len);
        if size < 0 {
            return Err(ProtoError::Frame(format!(
                "response length prefix is negative ({size})"
            )));
        }
        if size as usize > MAX_FRAME {
            return Err(ProtoError::Frame(format!(
                "response length prefix is {size} bytes, more than the {MAX_FRAME} byte limit"
            )));
        }
        let mut payload = vec![0u8; size as usize];
        read_exact_within(
            &mut self.stream,
            &mut payload,
            timeout,
            &format!("{size} bytes of response body"),
        )
        .await?;
        self.last_response = Some(payload.clone());
        Ok(payload)
    }

    /// Read bytes until the peer closes, or fail if it does not close in time.
    pub async fn expect_closed(&mut self, timeout: Duration) -> ProtoResult<Vec<u8>> {
        let mut seen = Vec::new();
        let deadline = tokio::time::Instant::now() + timeout;
        let mut chunk = [0u8; 4096];
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Err(ProtoError::Timeout(
                    "the broker to close the connection".to_string(),
                ));
            }
            match tokio::time::timeout(left, self.stream.read(&mut chunk)).await {
                Err(_) => {
                    return Err(ProtoError::Timeout(
                        "the broker to close the connection".to_string(),
                    ))
                }
                Ok(Ok(0)) => return Ok(seen),
                Ok(Ok(n)) => seen.extend_from_slice(&chunk[..n]),
                Ok(Err(e)) if is_reset(&e) => return Ok(seen),
                Ok(Err(e)) => return Err(ProtoError::Io(e)),
            }
        }
    }

    /// Assert nothing arrives within `quiet`; returns the bytes that did arrive, if any.
    pub async fn read_silence(&mut self, quiet: Duration) -> ProtoResult<Vec<u8>> {
        let mut chunk = [0u8; 4096];
        match tokio::time::timeout(quiet, self.stream.read(&mut chunk)).await {
            Err(_) => Ok(Vec::new()),
            Ok(Ok(0)) => Err(ProtoError::Closed),
            Ok(Ok(n)) => Ok(chunk[..n].to_vec()),
            Ok(Err(e)) if is_reset(&e) => Err(ProtoError::Closed),
            Ok(Err(e)) => Err(ProtoError::Io(e)),
        }
    }
}

fn is_reset(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
    )
}

async fn read_exact_within(
    stream: &mut TcpStream,
    buf: &mut [u8],
    timeout: Duration,
    what: &str,
) -> ProtoResult<()> {
    let mut read = 0usize;
    let deadline = tokio::time::Instant::now() + timeout;
    while read < buf.len() {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Err(ProtoError::Timeout(what.to_string()));
        }
        match tokio::time::timeout(left, stream.read(&mut buf[read..])).await {
            Err(_) => return Err(ProtoError::Timeout(what.to_string())),
            Ok(Ok(0)) => return Err(ProtoError::Closed),
            Ok(Ok(n)) => read += n,
            Ok(Err(e)) if is_reset(&e) => return Err(ProtoError::Closed),
            Ok(Err(e)) => return Err(ProtoError::Io(e)),
        }
    }
    Ok(())
}

/// Render bytes as an annotated hex dump, marking the given byte ranges with `^`.
pub fn hexdump(bytes: &[u8], marks: &[Range<usize>], max_bytes: usize) -> String {
    let shown = bytes.len().min(max_bytes);
    let mut out = String::new();
    for row in 0..shown.div_ceil(16) {
        let start = row * 16;
        let end = (start + 16).min(shown);
        let mut hex = String::new();
        let mut ascii = String::new();
        for (i, cell) in (start..start + 16).enumerate() {
            if i % 8 == 0 && i != 0 {
                hex.push(' ');
            }
            match bytes.get(cell).filter(|_| cell < end) {
                Some(&c) => {
                    hex.push_str(&format!("{c:02x} "));
                    ascii.push(if (0x20..0x7f).contains(&c) {
                        char::from(c)
                    } else {
                        '.'
                    });
                }
                None => {
                    hex.push_str("   ");
                    ascii.push(' ');
                }
            }
        }
        out.push_str(&format!("{start:04x}  {hex} |{ascii}|\n"));
        // Caret line under any marked byte in this row.
        let mut caret = String::new();
        let mut any = false;
        for i in start..end {
            if i % 8 == 0 && i != start {
                caret.push(' ');
            }
            if marks.iter().any(|m| m.contains(&i)) {
                caret.push_str("^^ ");
                any = true;
            } else {
                caret.push_str("   ");
            }
        }
        if any {
            out.push_str(&format!("      {caret}\n"));
        }
    }
    if bytes.len() > shown {
        out.push_str(&format!("      ... {} more bytes\n", bytes.len() - shown));
    }
    if out.is_empty() {
        out.push_str("      (no bytes)\n");
    }
    out.trim_end().to_string()
}

/// Byte ranges inside `haystack` that hold the big-endian encoding of `needle`.
///
/// Used to point the hex dump at the region an assertion is about when the test does not
/// know the exact offset.
pub fn find_be(haystack: &[u8], needle: &[u8]) -> Vec<Range<usize>> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..=haystack.len() - needle.len() {
        if &haystack[i..i + needle.len()] == needle {
            out.push(i..i + needle.len());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kafka_protocol::messages::ApiVersionsRequest;

    #[test]
    fn request_frames_carry_the_header() {
        let req = ApiVersionsRequest::default();
        let payload = encode_request(4, 7, Some("me"), &req).expect("encode");
        // api_key(2) api_version(2) correlation_id(4) client_id(2 + 2) ...
        assert_eq!(&payload[0..2], &18i16.to_be_bytes());
        assert_eq!(&payload[2..4], &4i16.to_be_bytes());
        assert_eq!(&payload[4..8], &7i32.to_be_bytes());
        assert_eq!(&payload[8..10], &2i16.to_be_bytes());
        assert_eq!(&payload[10..12], b"me");
    }

    #[test]
    fn null_client_id_is_encoded_as_minus_one() {
        let req = ApiVersionsRequest::default();
        let payload = encode_request(4, 1, None, &req).expect("encode");
        assert_eq!(&payload[8..10], &(-1i16).to_be_bytes());
    }

    #[test]
    fn hexdump_marks_the_region() {
        let d = hexdump(&[0, 1, 2, 3, 0x41, 0x42], std::slice::from_ref(&(4..6)), 64);
        assert!(d.contains("0000  "), "{d}");
        assert!(d.contains("....AB"), "{d}");
        assert!(d.contains("^^"), "{d}");
    }

    #[test]
    fn hexdump_truncates() {
        let d = hexdump(&[0u8; 200], &[], 32);
        assert!(d.contains("168 more bytes"), "{d}");
    }

    #[test]
    fn find_be_locates_values() {
        let hay = [0u8, 0, 0, 3, 9, 9, 0, 3];
        assert_eq!(find_be(&hay, &3i16.to_be_bytes()), vec![2..4, 6..8]);
        assert!(find_be(&hay, &[]).is_empty());
    }
}
