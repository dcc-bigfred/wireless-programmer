//! Bounded HTTP/1.1 client for device config pages.
//!
//! Plain HTTP only — Soft-AP config pages serve on an on-link address with no
//! TLS. The client binds to a caller-supplied source address (e.g.
//! `192.168.4.2` or `192.168.0.2`) so requests leave the wireless interface,
//! and enforces a deadline, a maximum response body size, and a bounded retry
//! count.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use socket2::{Domain, Socket, Type};
use wp_core::HttpClient;

/// Socket I/O slice used when a cancel flag is armed, so a firmware POST can
/// abort within about a second of `job cancel`.
const CANCEL_POLL: Duration = Duration::from_secs(1);

/// Maximum response body: 64 KiB.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// Default per-request deadline.
pub const REQUEST_DEADLINE: Duration = Duration::from_secs(5);

/// Default connect deadline.
pub const CONNECT_DEADLINE: Duration = Duration::from_secs(3);

/// Default retry count (total attempts = retries + 1).
pub const RETRIES: u32 = 3;

/// A bounded HTTP/1.1 client.
pub struct BoundedHttpClient {
    /// Target host (IP literal).
    host: String,
    /// Target port.
    port: u16,
    /// Source address to bind, when supplied.
    source: Option<SocketAddr>,
    /// Per-request deadline.
    deadline: Duration,
    /// Connect deadline.
    connect_deadline: Duration,
    /// Retry count.
    retries: u32,
    /// Maximum response body size.
    max_body: usize,
    /// When set, long reads/writes abort with [`io::ErrorKind::Interrupted`].
    cancel: Option<Arc<AtomicBool>>,
}

impl BoundedHttpClient {
    /// Construct a client targeting `host:port`.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            source: None,
            deadline: REQUEST_DEADLINE,
            connect_deadline: CONNECT_DEADLINE,
            retries: RETRIES,
            max_body: MAX_BODY_BYTES,
            cancel: None,
        }
    }

    /// Bind the TCP socket to `source` so traffic leaves a specific interface.
    pub fn with_source(mut self, source: SocketAddr) -> Self {
        self.source = Some(source);
        self
    }

    /// Override the per-request deadline.
    pub fn with_deadline(mut self, d: Duration) -> Self {
        self.deadline = d;
        self
    }

    /// Override the retry count.
    pub fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// Abort in-flight I/O when `cancel` becomes true (firmware POST).
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Issue a single request, returning the raw body.
    fn request_once(
        &mut self,
        method: &str,
        path: &str,
        body: Option<(&str, &[u8])>,
    ) -> io::Result<Vec<u8>> {
        let addr: SocketAddr = format!("{}:{}", self.host, self.port)
            .parse()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        let stream = match self.source {
            Some(src) => {
                let socket = Socket::new(Domain::IPV4, Type::STREAM, None)?;
                socket.bind(&src.into())?;
                socket.connect_timeout(&addr.into(), self.connect_deadline)?;
                TcpStream::from(socket)
            }
            None => TcpStream::connect_timeout(&addr, self.connect_deadline)?,
        };
        stream.set_read_timeout(Some(self.deadline))?;
        stream.set_write_timeout(Some(self.deadline))?;
        let mut stream = stream;
        let cancel = self.cancel.as_deref();
        let io_deadline = Instant::now() + self.deadline;

        if cancelled(cancel) {
            return Err(io_cancelled());
        }

        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n",
            host = self.host,
            port = self.port,
        );
        if let Some((content_type, bytes)) = body {
            request.push_str(&format!(
                "Content-Type: {content_type}\r\nContent-Length: {}\r\n",
                bytes.len()
            ));
        }
        request.push_str("\r\n");
        write_all_interruptible(&mut stream, request.as_bytes(), io_deadline, cancel)?;
        if let Some((_, bytes)) = body {
            write_all_interruptible(&mut stream, bytes, io_deadline, cancel)?;
        }
        flush_interruptible(&mut stream, io_deadline, cancel)?;

        let mut buf = Vec::with_capacity(4096);
        let mut chunk = [0u8; 4096];
        loop {
            if cancelled(cancel) {
                return Err(io_cancelled());
            }
            if buf.len() > self.max_body {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "response exceeds max body size",
                ));
            }
            let remaining = io_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "request deadline elapsed",
                ));
            }
            let slice = if cancel.is_some() {
                remaining.min(CANCEL_POLL)
            } else {
                remaining
            };
            stream.set_read_timeout(Some(slice))?;
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) && cancel.is_some()
                        && io_deadline.saturating_duration_since(Instant::now())
                            > Duration::ZERO =>
                {
                    continue;
                }
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "read deadline elapsed",
                    ));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "read deadline elapsed",
                    ));
                }
                Err(e) => return Err(e),
            }
        }

        let body_start = locate_body(&buf)?;
        if buf.len() - body_start > self.max_body {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "response body exceeds max body size",
            ));
        }
        let status = parse_status(&buf)?;
        if !(200..300).contains(&status) {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("unexpected HTTP status {status}"),
            ));
        }
        Ok(buf[body_start..].to_vec())
    }
}

impl HttpClient for BoundedHttpClient {
    fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Option<(&str, &[u8])>,
    ) -> io::Result<Vec<u8>> {
        let mut last = io::Error::other("no attempt made");
        for _ in 0..=self.retries {
            match self.request_once(method, path, body) {
                Ok(body) => return Ok(body),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
                Err(e) => {
                    last = e;
                }
            }
        }
        Err(last)
    }
}

fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|c| c.load(Ordering::Relaxed))
}

fn io_cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancelled")
}

fn write_all_interruptible(
    stream: &mut TcpStream,
    mut bytes: &[u8],
    deadline: Instant,
    cancel: Option<&AtomicBool>,
) -> io::Result<()> {
    while !bytes.is_empty() {
        if cancelled(cancel) {
            return Err(io_cancelled());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "write deadline elapsed",
            ));
        }
        let slice = if cancel.is_some() {
            remaining.min(CANCEL_POLL)
        } else {
            remaining
        };
        stream.set_write_timeout(Some(slice))?;
        match stream.write(bytes) {
            Ok(0) => {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "write zero"));
            }
            Ok(n) => bytes = &bytes[n..],
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn flush_interruptible(
    stream: &mut TcpStream,
    deadline: Instant,
    cancel: Option<&AtomicBool>,
) -> io::Result<()> {
    loop {
        if cancelled(cancel) {
            return Err(io_cancelled());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "write deadline elapsed",
            ));
        }
        let slice = if cancel.is_some() {
            remaining.min(CANCEL_POLL)
        } else {
            remaining
        };
        stream.set_write_timeout(Some(slice))?;
        match stream.flush() {
            Ok(()) => return Ok(()),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Find the start of the response body (after the blank line).
fn locate_body(buf: &[u8]) -> io::Result<usize> {
    for i in 3..buf.len() {
        if buf[i - 3..=i] == *b"\r\n\r\n" {
            return Ok(i + 1);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "no header/body separator",
    ))
}

/// Parse the HTTP status code from the status line.
fn parse_status(buf: &[u8]) -> io::Result<u16> {
    let line_end = buf.iter().position(|&b| b == b'\r').unwrap_or(buf.len());
    let line = std::str::from_utf8(&buf[..line_end])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut parts = line.split_whitespace();
    let _version = parts.next();
    let status = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no status code"))?;
    status
        .parse::<u16>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Percent-encode a query value per RFC 3986 (unreserved set kept).
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn percent_encode_keeps_unreserved_and_escapes_rest() {
        assert_eq!(percent_encode("abc-._~"), "abc-._~");
        assert_eq!(percent_encode("a b/c"), "a%20b%2Fc");
        assert_eq!(percent_encode("wifred"), "wifred");
        assert_eq!(percent_encode("P@ss word!"), "P%40ss%20word%21");
    }

    #[test]
    fn locate_body_finds_separator() {
        let buf = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi";
        assert_eq!(locate_body(buf).unwrap(), buf.len() - 2);
    }

    #[test]
    fn parse_status_reads_code() {
        assert_eq!(parse_status(b"HTTP/1.1 200 OK\r\n").unwrap(), 200);
        assert_eq!(parse_status(b"HTTP/1.1 404 Not Found\r\n").unwrap(), 404);
    }

    #[test]
    fn parse_status_rejects_missing_code() {
        assert!(parse_status(b"HTTP/1.1\r\n").is_err());
    }

    #[test]
    fn locate_body_rejects_no_separator() {
        let buf = b"HTTP/1.1 200 OK\r\nContent-Length: 0";
        assert!(locate_body(buf).is_err());
    }

    #[test]
    fn bounded_client_records_source() {
        let src: SocketAddr = "192.168.4.2:0".parse().unwrap();
        let c = BoundedHttpClient::new("192.168.4.1", 80).with_source(src);
        assert_eq!(c.source, Some(src));
        assert_eq!(c.port, 80);
        assert_eq!(c.host, "192.168.4.1");
    }

    #[test]
    fn request_aborts_on_cancel() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 64];
            while s.read(&mut buf).unwrap_or(0) > 0 {}
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::Relaxed);
        });
        let mut c = BoundedHttpClient::new(addr.ip().to_string(), addr.port())
            .with_deadline(Duration::from_secs(10))
            .with_retries(0)
            .with_cancel(Arc::clone(&cancel));
        let body = vec![0u8; 64];
        let err = c
            .request("POST", "/", Some(("application/octet-stream", &body)))
            .expect_err("cancel");
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        let _ = server.join();
    }

    // A trivial in-memory HttpClient for driver tests.
    #[derive(Default)]
    pub struct FakeHttp {
        pub requests: Vec<(String, String, Option<Vec<u8>>)>,
        pub responses: std::collections::VecDeque<Vec<u8>>,
    }

    impl HttpClient for FakeHttp {
        fn request(
            &mut self,
            method: &str,
            path: &str,
            body: Option<(&str, &[u8])>,
        ) -> io::Result<Vec<u8>> {
            self.requests.push((
                method.to_string(),
                path.to_string(),
                body.map(|(_, b)| b.to_vec()),
            ));
            self.responses
                .pop_front()
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no fake response"))
        }
    }

    #[test]
    fn fake_http_records_and_replies() {
        let mut f = FakeHttp {
            requests: Vec::new(),
            responses: [b"ok".to_vec()].into(),
        };
        let body = <FakeHttp as HttpClient>::get(&mut f, "/index.html?x=1");
        assert_eq!(body.unwrap(), b"ok");
        assert_eq!(f.requests.len(), 1);
        assert_eq!(f.requests[0].0, "GET");
        assert_eq!(f.requests[0].1, "/index.html?x=1");
        let _ = Cursor::new(b"");
    }
}
