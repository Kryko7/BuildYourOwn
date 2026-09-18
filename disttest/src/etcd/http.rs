//! A minimal HTTP/1.1 client, written for this suite.
//!
//! Nothing here is general-purpose: it speaks exactly the two shapes the etcd v3 JSON
//! gateway uses — a unary `POST` answered with `Content-Length`, and a streaming `POST`
//! answered `Transfer-Encoding: chunked` with one JSON object per line — and it keeps
//! connections alive, because the linearizability workload opens thousands of requests and
//! a connection per request would exhaust the ephemeral port range.
//!
//! Writing it by hand is also what lets a failure show the exact status line, headers and
//! body the program under test produced, however malformed they were.

use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

/// Everything that can go wrong below the JSON layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// The connection could not be opened.
    Connect(String),
    /// The connection failed or was closed in the middle of a message.
    Io(String),
    /// Nothing arrived within the deadline.
    Timeout(String),
    /// Something arrived, but it was not HTTP.
    Protocol(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Connect(s) => write!(f, "cannot connect: {s}"),
            HttpError::Io(s) => write!(f, "connection failed: {s}"),
            HttpError::Timeout(s) => write!(f, "timed out: {s}"),
            HttpError::Protocol(s) => write!(f, "not valid HTTP: {s}"),
        }
    }
}

/// One complete HTTP response.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// The status code from the status line.
    pub status: u16,
    /// The reason phrase, kept because a failure prints the status line verbatim.
    pub reason: String,
    /// Header names lower-cased, values as they arrived.
    pub headers: Vec<(String, String)>,
    /// The body, already de-chunked.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// The first value of a header, looked up case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        let want = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == want)
            .map(|(_, v)| v.as_str())
    }

    /// The body as text, lossily, for error messages.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The status line and headers, the way a failure block prints them.
    pub fn head(&self) -> String {
        let mut s = format!("HTTP/1.1 {} {}\n", self.status, self.reason);
        for (k, v) in &self.headers {
            s.push_str(&format!("{k}: {v}\n"));
        }
        s.trim_end().to_string()
    }
}

/// A keep-alive connection to one address.
struct Conn {
    reader: BufReader<TcpStream>,
}

/// A client for one `http://host:port` base URL.
pub struct Http {
    /// Where requests go.
    pub addr: SocketAddr,
    /// The `Host:` header value, kept exactly as the base URL spelled it.
    pub host: String,
    /// Per-request deadline.
    pub timeout: Duration,
    idle: Mutex<Vec<Conn>>,
}

impl Http {
    /// Build a client for a `http://host:port` base URL.
    pub fn new(base_url: &str, timeout: Duration) -> Result<Http, HttpError> {
        let rest = base_url
            .strip_prefix("http://")
            .ok_or_else(|| HttpError::Protocol(format!("{base_url} is not an http:// URL")))?;
        let host = rest.trim_end_matches('/').to_string();
        let addr: SocketAddr = host
            .parse()
            .map_err(|e| HttpError::Protocol(format!("{host} is not an address: {e}")))?;
        Ok(Http {
            addr,
            host,
            timeout,
            idle: Mutex::new(Vec::new()),
        })
    }

    async fn checkout(&self) -> Result<Conn, HttpError> {
        if let Some(c) = self.idle.lock().await.pop() {
            return Ok(c);
        }
        let stream = tokio::time::timeout(self.timeout, TcpStream::connect(self.addr))
            .await
            .map_err(|_| HttpError::Timeout(format!("connecting to {}", self.addr)))?
            .map_err(|e| HttpError::Connect(format!("{}: {e}", self.addr)))?;
        let _ = stream.set_nodelay(true);
        Ok(Conn {
            reader: BufReader::new(stream),
        })
    }

    /// Drop every pooled connection; used when a node has been killed under us.
    pub async fn reset(&self) {
        self.idle.lock().await.clear();
    }

    /// Send one request and read the whole response.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, HttpError> {
        // One retry, and only on a connection that came out of the pool: a keep-alive
        // connection the server has since closed is normal, a fresh one failing is not.
        for attempt in 0..2 {
            let pooled = !self.idle.lock().await.is_empty();
            let mut conn = self.checkout().await?;
            match self.round_trip(&mut conn, method, path, body).await {
                Ok(resp) => {
                    if resp
                        .header("connection")
                        .is_none_or(|v| !v.eq_ignore_ascii_case("close"))
                    {
                        self.idle.lock().await.push(conn);
                    }
                    return Ok(resp);
                }
                Err(e) => {
                    if attempt == 0 && pooled && matches!(e, HttpError::Io(_)) {
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        Err(HttpError::Io("no attempt succeeded".into()))
    }

    async fn round_trip(
        &self,
        conn: &mut Conn,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, HttpError> {
        let req = build_request(method, path, &self.host, body);
        let deadline = self.timeout;
        tokio::time::timeout(deadline, async {
            conn.reader
                .get_mut()
                .write_all(&req)
                .await
                .map_err(|e| HttpError::Io(e.to_string()))?;
            conn.reader
                .get_mut()
                .flush()
                .await
                .map_err(|e| HttpError::Io(e.to_string()))?;
            read_response(&mut conn.reader).await
        })
        .await
        .map_err(|_| HttpError::Timeout(format!("{method} {path} after {deadline:?}")))?
    }

    /// Open a streaming request: the body is sent whole, the response arrives as chunks.
    pub async fn open_stream(
        &self,
        path: &str,
        body: &[u8],
        timeout: Duration,
    ) -> Result<Stream, HttpError> {
        let stream = tokio::time::timeout(timeout, TcpStream::connect(self.addr))
            .await
            .map_err(|_| HttpError::Timeout(format!("connecting to {}", self.addr)))?
            .map_err(|e| HttpError::Connect(format!("{}: {e}", self.addr)))?;
        let _ = stream.set_nodelay(true);
        let mut reader = BufReader::new(stream);
        let req = build_request("POST", path, &self.host, Some(body));
        tokio::time::timeout(timeout, async {
            reader
                .get_mut()
                .write_all(&req)
                .await
                .map_err(|e| HttpError::Io(e.to_string()))?;
            reader
                .get_mut()
                .flush()
                .await
                .map_err(|e| HttpError::Io(e.to_string()))
        })
        .await
        .map_err(|_| HttpError::Timeout(format!("POST {path}")))??;
        let head = tokio::time::timeout(timeout, read_head(&mut reader))
            .await
            .map_err(|_| HttpError::Timeout(format!("waiting for the response head of {path}")))??;
        let chunked = head
            .headers
            .iter()
            .any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
        Ok(Stream {
            reader,
            head,
            chunked,
            pending: Vec::new(),
            done: false,
        })
    }
}

/// An open streaming response: one JSON object per line, as they arrive.
pub struct Stream {
    reader: BufReader<TcpStream>,
    /// The status line and headers the stream started with.
    pub head: HttpResponse,
    chunked: bool,
    pending: Vec<u8>,
    done: bool,
}

impl Stream {
    /// The next line of the body, or `None` when the stream ended or the deadline passed.
    ///
    /// A deadline that passes is not an error: a watch that legitimately has nothing to say
    /// looks exactly like this, and every caller wants to tell the two apart by what it
    /// asked for, not by an error type.
    pub async fn next_line(&mut self, timeout: Duration) -> Result<Option<String>, HttpError> {
        loop {
            if let Some(line) = self.take_line() {
                return Ok(Some(line));
            }
            if self.done {
                return Ok(None);
            }
            let got = match tokio::time::timeout(timeout, self.fill()).await {
                Ok(r) => r?,
                Err(_) => return Ok(None),
            };
            if !got {
                self.done = true;
                return Ok(self.take_line());
            }
        }
    }

    fn take_line(&mut self) -> Option<String> {
        let pos = self.pending.iter().position(|b| *b == b'\n')?;
        let line: Vec<u8> = self.pending.drain(..=pos).collect();
        let text = String::from_utf8_lossy(&line).trim().to_string();
        if text.is_empty() {
            return self.take_line();
        }
        Some(text)
    }

    /// Read one more piece of body; `false` once the stream is over.
    async fn fill(&mut self) -> Result<bool, HttpError> {
        if self.chunked {
            let mut size_line = String::new();
            let n = self
                .reader
                .read_line(&mut size_line)
                .await
                .map_err(|e| HttpError::Io(e.to_string()))?;
            if n == 0 {
                return Ok(false);
            }
            let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or(""), 16)
                .map_err(|_| HttpError::Protocol(format!("bad chunk size {size_line:?}")))?;
            if size == 0 {
                return Ok(false);
            }
            let mut buf = vec![0u8; size + 2];
            self.reader
                .read_exact(&mut buf)
                .await
                .map_err(|e| HttpError::Io(e.to_string()))?;
            buf.truncate(size);
            self.pending.extend_from_slice(&buf);
            Ok(true)
        } else {
            let mut buf = [0u8; 8192];
            let n = self
                .reader
                .read(&mut buf)
                .await
                .map_err(|e| HttpError::Io(e.to_string()))?;
            if n == 0 {
                return Ok(false);
            }
            self.pending.extend_from_slice(&buf[..n]);
            Ok(true)
        }
    }
}

fn build_request(method: &str, path: &str, host: &str, body: Option<&[u8]>) -> Vec<u8> {
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n");
    req.push_str("User-Agent: disttest\r\nAccept: application/json\r\n");
    if let Some(b) = body {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    let mut out = req.into_bytes();
    if let Some(b) = body {
        out.extend_from_slice(b);
    }
    out
}

async fn read_head(reader: &mut BufReader<TcpStream>) -> Result<HttpResponse, HttpError> {
    let mut status = String::new();
    let n = reader
        .read_line(&mut status)
        .await
        .map_err(|e| HttpError::Io(e.to_string()))?;
    if n == 0 {
        return Err(HttpError::Io(
            "the connection was closed before a status line arrived".into(),
        ));
    }
    let mut parts = status.trim_end().splitn(3, ' ');
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/1.") {
        return Err(HttpError::Protocol(format!(
            "the first line was {status:?}, not an HTTP status line"
        )));
    }
    let code: u16 = parts
        .next()
        .unwrap_or_default()
        .parse()
        .map_err(|_| HttpError::Protocol(format!("no status code in {status:?}")))?;
    let reason = parts.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| HttpError::Io(e.to_string()))?;
        if n == 0 {
            return Err(HttpError::Io(
                "the connection was closed inside the response headers".into(),
            ));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let Some((k, v)) = line.split_once(':') else {
            return Err(HttpError::Protocol(format!("bad header line {line:?}")));
        };
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    Ok(HttpResponse {
        status: code,
        reason,
        headers,
        body: Vec::new(),
    })
}

async fn read_response(reader: &mut BufReader<TcpStream>) -> Result<HttpResponse, HttpError> {
    let mut resp = read_head(reader).await?;
    let chunked = resp
        .headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
    let length: Option<usize> = resp
        .headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok());
    if chunked {
        loop {
            let mut size_line = String::new();
            let n = reader
                .read_line(&mut size_line)
                .await
                .map_err(|e| HttpError::Io(e.to_string()))?;
            if n == 0 {
                break;
            }
            let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or(""), 16)
                .map_err(|_| HttpError::Protocol(format!("bad chunk size {size_line:?}")))?;
            if size == 0 {
                let mut trailer = String::new();
                let _ = reader.read_line(&mut trailer).await;
                break;
            }
            let mut buf = vec![0u8; size + 2];
            reader
                .read_exact(&mut buf)
                .await
                .map_err(|e| HttpError::Io(e.to_string()))?;
            buf.truncate(size);
            resp.body.extend_from_slice(&buf);
        }
    } else if let Some(len) = length {
        let mut buf = vec![0u8; len];
        reader
            .read_exact(&mut buf)
            .await
            .map_err(|e| HttpError::Io(e.to_string()))?;
        resp.body = buf;
    } else if resp.status != 204 && resp.status != 304 {
        // No framing at all: the body runs to end of stream, so this connection is spent.
        reader
            .read_to_end(&mut resp.body)
            .await
            .map_err(|e| HttpError::Io(e.to_string()))?;
        resp.headers
            .push(("connection".into(), "close".into()));
    }
    Ok(resp)
}

/// A blocking one-shot `GET`, used while waiting for a node to come up.
///
/// The runner is not inside a tokio runtime when it starts a node, and a readiness probe is
/// the one place where blocking is simpler than being asynchronous.
pub fn blocking_get(base_url: &str, path: &str, timeout: Duration) -> Result<(u16, String), String> {
    use std::io::{Read, Write};
    let host = base_url
        .strip_prefix("http://")
        .ok_or_else(|| format!("{base_url} is not an http:// URL"))?
        .trim_end_matches('/');
    let addr: SocketAddr = host.parse().map_err(|e| format!("{host}: {e}"))?;
    let mut s = std::net::TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| format!("connect {addr}: {e}"))?;
    s.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: disttest\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut text = Vec::new();
    s.read_to_end(&mut text).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&text).to_string();
    let status: u16 = text
        .lines()
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("no status line in {:?}", &text[..text.len().min(60)]))?;
    let body = text.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    async fn serve(reply: &'static [u8]) -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = l.local_addr().expect("addr");
        tokio::spawn(async move {
            for _ in 0..4 {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(reply).await;
                let _ = s.flush().await;
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn content_length_bodies_are_read_exactly() {
        let url = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Type: application/json\r\n\r\nhello").await;
        let http = Http::new(&url, Duration::from_secs(2)).expect("client");
        let r = http.request("POST", "/v3/kv/put", Some(b"{}")).await.expect("request");
        assert_eq!(r.status, 200);
        assert_eq!(r.text(), "hello");
        assert_eq!(r.header("content-type"), Some("application/json"));
        assert!(r.head().starts_with("HTTP/1.1 200 OK"));
    }

    #[tokio::test]
    async fn chunked_bodies_are_reassembled() {
        let url = serve(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n",
        )
        .await;
        let http = Http::new(&url, Duration::from_secs(2)).expect("client");
        let r = http.request("POST", "/x", Some(b"{}")).await.expect("request");
        assert_eq!(r.text(), "abcde");
    }

    #[tokio::test]
    async fn a_streaming_response_yields_one_line_at_a_time() {
        let url = serve(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\n{\"a\"}\n\r\n6\r\n{\"b\"}\n\r\n0\r\n\r\n",
        )
        .await;
        let http = Http::new(&url, Duration::from_secs(2)).expect("client");
        let mut st = http
            .open_stream("/v3/watch", b"{}", Duration::from_secs(2))
            .await
            .expect("stream");
        assert_eq!(st.head.status, 200);
        assert_eq!(
            st.next_line(Duration::from_secs(2)).await.expect("line"),
            Some("{\"a\"}".to_string())
        );
        assert_eq!(
            st.next_line(Duration::from_secs(2)).await.expect("line"),
            Some("{\"b\"}".to_string())
        );
        assert_eq!(st.next_line(Duration::from_millis(200)).await.expect("end"), None);
    }

    #[tokio::test]
    async fn a_reply_that_is_not_http_is_a_protocol_error() {
        let url = serve(b"I am not a web server\r\n\r\n").await;
        let http = Http::new(&url, Duration::from_secs(2)).expect("client");
        let e = http.request("POST", "/x", Some(b"{}")).await.expect_err("must fail");
        assert!(matches!(e, HttpError::Protocol(_)), "{e:?}");
    }

    #[tokio::test]
    async fn a_closed_port_is_a_connect_error() {
        let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = l.local_addr().expect("addr");
        drop(l);
        let http = Http::new(&format!("http://{addr}"), Duration::from_millis(500)).expect("client");
        let e = http.request("POST", "/x", Some(b"{}")).await.expect_err("must fail");
        assert!(matches!(e, HttpError::Connect(_)), "{e:?}");
    }
}
