//! Bounded HTTP/1.1 client for device config pages.
//!
//! Plain HTTP only — Soft-AP config pages serve on an on-link address with no
//! TLS. The client binds to a caller-supplied source address (e.g.
//! `192.168.4.2`) so requests leave the wireless interface,
//! and enforces a deadline, a maximum response body size, and a bounded retry
//! count.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};
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

/// Default delay between retries. Soft-AP HTTP servers need a moment
/// after association before they accept connections.
pub const RETRY_DELAY: Duration = Duration::from_millis(500);

/// A bounded HTTP/1.1 client.
pub struct BoundedHttpClient {
    /// Target host (IP literal).
    host: String,
    /// Target port.
    port: u16,
    /// Source address to bind, when supplied.
    source: Option<SocketAddr>,
    /// Network interface used for diagnostics and, when the destination is
    /// **not** a local address, `SO_BINDTODEVICE`.
    device: Option<String>,
    /// Per-request deadline.
    deadline: Duration,
    /// Connect deadline.
    connect_deadline: Duration,
    /// Retry count.
    retries: u32,
    /// Delay between retry attempts. Soft-AP HTTP servers (ESP32 lwIP) need
    /// a moment after association before they accept connections; without
    /// backoff, four retries fire in under 200 ms and all get RST.
    retry_delay: Duration,
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
            device: None,
            deadline: REQUEST_DEADLINE,
            connect_deadline: CONNECT_DEADLINE,
            retries: RETRIES,
            retry_delay: RETRY_DELAY,
            max_body: MAX_BODY_BYTES,
            cancel: None,
        }
    }

    /// Bind the TCP socket to `source` so traffic leaves a specific interface.
    pub fn with_source(mut self, source: SocketAddr) -> Self {
        self.source = Some(source);
        self
    }

    /// Remember the wireless interface name for diagnostics and for
    /// `SO_BINDTODEVICE` when the destination is **not** a local address.
    ///
    /// When the Soft-AP IP is also assigned on another interface (e.g. a
    /// device AP at `192.168.0.1` vs hub LAN), `SO_BINDTODEVICE` must **not**
    /// be set:
    /// the SYN-ACK's source is a local address, so the kernel may deliver
    /// it with `skb->dev = lo`, and a socket bound to `wlan0` will not match
    /// — Linux then generates RST (`Connection reset by peer`). Output is
    /// forced by binding [`Self::with_source`] plus the fib rule in
    /// [`crate::netcfg`].
    pub fn with_device(mut self, device: impl Into<String>) -> Self {
        self.device = Some(device.into());
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

        let dest_is_local = match addr.ip() {
            std::net::IpAddr::V4(v4) => crate::netcfg::is_local_address(v4),
            std::net::IpAddr::V6(_) => false,
        };

        let socket = Socket::new(Domain::IPV4, Type::STREAM, None)?;
        // SO_BINDTODEVICE only when the destination is not also local — see
        // [`Self::with_device`].
        if let Some(dev) = self.device.as_deref() {
            if dest_is_local {
                log::debug!(
                    "http: skip SO_BINDTODEVICE on {dev}; {addr} collides with a local address"
                );
            } else {
                socket.bind_device(Some(dev.as_bytes()))?;
            }
        }
        if let Some(src) = self.source {
            socket
                .bind(&src.into())
                .map_err(|e| io::Error::new(e.kind(), format!("bind {src}: {e}")))?;
        }
        socket
            .connect_timeout(&addr.into(), self.connect_deadline)
            .map_err(|e| io::Error::new(e.kind(), format!("connect {addr}: {e}")))?;
        let stream = TcpStream::from(socket);
        stream.set_read_timeout(Some(self.deadline))?;
        stream.set_write_timeout(Some(self.deadline))?;
        let _ = stream.set_nodelay(true);
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
        write_all_interruptible(&mut stream, request.as_bytes(), io_deadline, cancel)
            .map_err(|e| io::Error::new(e.kind(), format!("write headers: {e}")))?;
        if let Some((_, bytes)) = body {
            write_all_interruptible(&mut stream, bytes, io_deadline, cancel)
                .map_err(|e| io::Error::new(e.kind(), format!("write body: {e}")))?;
        }
        flush_interruptible(&mut stream, io_deadline, cancel)
            .map_err(|e| io::Error::new(e.kind(), format!("flush: {e}")))?;

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
            if http_message_complete(&buf) {
                break;
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
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {
                    // LongFred (embassy-net) calls abort() after the response,
                    // which is a TCP RST. Linux then errors the next read even
                    // when the full HTTP message is already in `buf`.
                    log::debug!(
                        "read RST after {} bytes (complete={})",
                        buf.len(),
                        http_message_complete(&buf)
                    );
                    if http_message_complete(&buf) {
                        break;
                    }
                    return Err(io::Error::new(e.kind(), format!("read: {e}")));
                }
                Err(e) => {
                    return Err(io::Error::new(e.kind(), format!("read: {e}")));
                }
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
        let attempts = self.retries + 1;
        let mut last = io::Error::other("no attempt made");
        for attempt in 1..=attempts {
            if attempt == 1 {
                self.probe_before_connect(method, path);
            }
            if attempt > 1 {
                log::debug!(
                    "{} retrying {method} {path} after {} (attempt {attempt}/{attempts})",
                    self.target(),
                    self.retry_delay.as_millis()
                );
                std::thread::sleep(self.retry_delay);
            }
            match self.request_once(method, path, body) {
                Ok(body) => {
                    if attempt > 1 {
                        log::debug!(
                            "{} succeeded on attempt {attempt}/{attempts}",
                            self.target()
                        );
                    }
                    return Ok(body);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
                Err(e) => {
                    log::debug!(
                        "{} {method} {path} attempt {attempt}/{attempts} failed: {e} (kind={:?})",
                        self.target(),
                        e.kind()
                    );
                    last = e;
                }
            }
        }
        self.log_failure_hint(&last);
        Err(last)
    }
}

impl BoundedHttpClient {
    /// `dst`, plus the source address and device the socket is pinned to.
    fn target(&self) -> String {
        format!(
            "http {}:{} (source={} device={})",
            self.host,
            self.port,
            self.source.map_or("any".into(), |s| s.ip().to_string()),
            self.device.as_deref().unwrap_or("any"),
        )
    }

    /// Log the route, ARP state, and an ICMP probe before the first TCP
    /// attempt. An ICMP reply is only meaningful when the route is *not*
    /// local: otherwise this host answers and the Soft-AP is never reached.
    fn probe_before_connect(&self, method: &str, path: &str) {
        let Ok(dst) = self.host.parse::<Ipv4Addr>() else {
            return;
        };
        let src = self.source.and_then(|s| match s.ip() {
            std::net::IpAddr::V4(v4) => Some(v4),
            std::net::IpAddr::V6(_) => None,
        });
        let dev = self.device.as_deref();
        log::debug!(
            "{} {method} {path}: probing before connect (dst={dst} source={:?} device={:?})",
            self.target(),
            src,
            dev
        );
        if let Some(dev) = dev {
            if let Ok(neigh) = std::fs::read_to_string("/proc/net/arp") {
                for line in neigh.lines().skip(1) {
                    let cols: Vec<&str> = line.split_whitespace().collect();
                    if cols.len() >= 7 && cols[0] == dst.to_string() && cols[5] == dev {
                        log::debug!("arp: {dst} -> {} dev={dev} state={}", cols[3], cols[5]);
                    }
                }
            }
        }
        if let Some(src) = src {
            let src_s = src.to_string();
            let dst_s = dst.to_string();
            if let Ok(out) = Command::new("ip")
                .args(["-4", "route", "get", &dst_s, "from", &src_s])
                .output()
            {
                let got = String::from_utf8_lossy(&out.stdout);
                let got = got.trim();
                log::info!("ip -4 route get {dst_s} from {src_s}: {got}");
                if crate::netcfg::route_is_via_loopback(got) {
                    log::warn!(
                        "route to {dst} is still local; ICMP probe would hit this host, not the Soft-AP"
                    );
                    return;
                }
            }
            if let Ok(out) = Command::new("ip").args(["-4", "rule", "list"]).output() {
                log::debug!(
                    "ip -4 rule list:\n{}",
                    String::from_utf8_lossy(&out.stdout).trim()
                );
            }
        }
        match icmp_probe(dst, src, dev, Duration::from_secs(2)) {
            Ok(()) => log::info!("icmp probe: {dst} replied (L3 ok)"),
            Err(e) => log::warn!("icmp probe: {dst} failed: {e} (kind={:?})", e.kind()),
        }
    }

    /// Explain the two failure modes caused by a Soft-AP whose address the
    /// host already owns, so the log points at the fix instead of just the
    /// errno. Only emitted once per exhausted request.
    fn log_failure_hint(&self, err: &io::Error) {
        log::warn!(
            "{} failed after {} attempts: {err}",
            self.target(),
            self.retries + 1
        );

        let Ok(dst) = self.host.parse::<std::net::Ipv4Addr>() else {
            return;
        };
        if !crate::netcfg::is_local_address(dst) {
            return;
        }
        log::warn!(
            "{dst} is also a local address on this host — the Soft-AP subnet \
             collides with a local interface"
        );
        match err.kind() {
            io::ErrorKind::ConnectionRefused => log::warn!(
                "connection refused: the SYN was delivered locally; the fib rule \
                 from the wireless source to {dst} is missing or still after lookup local"
            ),
            io::ErrorKind::ConnectionReset => log::warn!(
                "connection reset during `{err}` — empty-buffer RST means the Soft-AP \
                 aborted before a complete HTTP response; a RST after Content-Length \
                 is normal for current LongFred firmware (abort after respond)"
            ),
            io::ErrorKind::TimedOut => log::warn!(
                "timed out: replies from {dst} are most likely dropped as a martian \
                 source; needs accept_local=1 (and rp_filter=0) on {}",
                self.device.as_deref().unwrap_or("the wireless interface")
            ),
            _ => {}
        }
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

/// `Content-Length` from the header block, if present.
fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(headers).ok()?;
    for line in text.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            return value.trim().parse().ok();
        }
    }
    None
}

/// `true` when `buf` holds a full HTTP response (headers + `Content-Length` body).
///
/// Stop reading on a complete message instead of waiting for EOF: embassy-net
/// `abort()` after the response is a RST, not a FIN.
fn http_message_complete(buf: &[u8]) -> bool {
    let Ok(start) = locate_body(buf) else {
        return false;
    };
    let Some(len) = parse_content_length(&buf[..start]) else {
        return false;
    };
    buf.len() - start >= len
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

/// Send one ICMP echo to `dst`, bound to `source` and `device`.
///
/// Returns `Ok(())` when an echo reply arrives within `deadline`, or an
/// `Err` describing what failed. Used as a diagnostic before the first TCP
/// attempt: if ICMP works but TCP doesn't, the SYN-ACK is being dropped by
/// the local-address check in the kernel's TCP stack.
pub fn icmp_probe(
    dst: Ipv4Addr,
    source: Option<Ipv4Addr>,
    device: Option<&str>,
    deadline: Duration,
) -> io::Result<()> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4))?;
    if let Some(dev) = device {
        socket.bind_device(Some(dev.as_bytes()))?;
    }
    if let Some(src) = source {
        socket.bind(&SocketAddr::from((src, 0)).into())?;
    }
    socket.set_read_timeout(Some(deadline))?;

    let mut packet = [8u8, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
    let cksum = icmp_checksum(&packet);
    packet[2] = (cksum >> 8) as u8;
    packet[3] = cksum as u8;

    let dst_addr = SocketAddr::from((dst, 0));
    socket.send_to(&packet, &dst_addr.into())?;

    let mut buf = [std::mem::MaybeUninit::new(0u8); 64];
    let start = Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "icmp probe: no reply",
            ));
        }
        let remaining = deadline - elapsed;
        socket.set_read_timeout(Some(remaining))?;
        match socket.recv_from(&mut buf) {
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

fn icmp_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for (i, &b) in data.iter().enumerate() {
        if i % 2 == 0 {
            sum += (b as u32) << 8;
        } else {
            sum += b as u32;
        }
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !sum as u16
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
    fn http_message_complete_uses_content_length() {
        let buf =
            b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}";
        assert!(http_message_complete(buf));
        assert!(!http_message_complete(&buf[..buf.len() - 1]));
        assert!(!http_message_complete(b"HTTP/1.1 200 OK\r\n\r\n"));
    }

    #[test]
    fn parse_content_length_is_case_insensitive() {
        let headers = b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\n";
        assert_eq!(parse_content_length(headers), Some(3));
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
    fn bounded_client_records_device() {
        let c = BoundedHttpClient::new("192.168.0.1", 80).with_device("wlan0");
        assert_eq!(c.device.as_deref(), Some("wlan0"));
        assert!(c.source.is_none());
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
