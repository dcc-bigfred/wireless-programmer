//! Minimal Z21 LAN (UDP) client: framing, serial probe, LocoNet dispatch.
//!
//! The daemon owns the socket (guidelines §1.2). This module only encodes
//! packets and drives request/response on a caller-supplied [`UdpSocket`].

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use crate::mdns::{discover_mdns_hosts, OtaHost, Z21_UDP_SERVICE};

/// Default Z21 LAN port (spec §1.1).
pub const Z21_UDP_PORT: u16 = 21105;
/// Alternate Z21 LAN port (spec §1.1).
pub const Z21_UDP_PORT_ALT: u16 = 21106;

/// `LAN_GET_SERIAL_NUMBER` header.
pub const HEADER_GET_SERIAL_NUMBER: u16 = 0x0010;
/// `LAN_LOGOFF` header.
pub const HEADER_LOGOFF: u16 = 0x0030;
/// X-BUS tunnel header (`LAN_X_*`).
pub const HEADER_XBUS: u16 = 0x0040;
/// `LAN_LOCONET_DISPATCH_ADDR` header.
pub const HEADER_LOCONET_DISPATCH_ADDR: u16 = 0x00A3;

/// How long to wait for a DISPATCH_ADDR reply.
pub const DISPATCH_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait for the serial-number login probe.
pub const SERIAL_PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// One Z21 LAN record (`DataLen` + `Header` + `Data`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Header (little-endian on the wire).
    pub header: u16,
    /// Payload after the 4-byte header.
    pub data: Vec<u8>,
}

/// A Z21 command station found on the LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Z21Host {
    /// mDNS instance label, when known.
    pub hostname: String,
    /// IPv4.
    pub ipv4: Ipv4Addr,
    /// UDP port (typically 21105).
    pub port: u16,
    /// Serial from `LAN_GET_SERIAL_NUMBER`, when the UDP probe got a reply.
    pub serial: Option<u32>,
}

impl Z21Host {
    /// Candidate key `ip:port`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.ipv4, self.port)
    }

    /// Human-readable scan label.
    pub fn label(&self) -> String {
        let addr = self.key();
        if self.hostname.is_empty() || self.hostname == addr {
            match self.serial {
                Some(s) => format!("Z21 {addr} (serial {s})"),
                None => format!("Z21 {addr}"),
            }
        } else {
            match self.serial {
                Some(s) => format!("{} ({addr}, serial {s})", self.hostname),
                None => format!("{} ({addr})", self.hostname),
            }
        }
    }

    /// Parse an `ip:port` (or bare IPv4, defaulting to [`Z21_UDP_PORT`]) candidate
    /// key into a [`SocketAddr`].
    ///
    /// This is a synchronous IP-only fast path used by tests and the fake Z21.
    /// For arbitrary hostnames use [`Z21Host::normalize_key`] plus async DNS
    /// resolution (e.g. `tokio::net::lookup_host`) at the call site.
    pub fn parse_key(key: &str) -> Option<SocketAddr> {
        key.parse().ok().or_else(|| {
            let ip: Ipv4Addr = key.parse().ok()?;
            Some(SocketAddr::V4(SocketAddrV4::new(ip, Z21_UDP_PORT)))
        })
    }

    /// Normalize a candidate key to `host:port`, appending [`Z21_UDP_PORT`] when
    /// no port is present. Accepts a hostname or IP (DNS resolution is the
    /// caller's responsibility — use `tokio::net::lookup_host(&normalized)`).
    pub fn normalize_key(key: &str) -> String {
        if key.rsplit_once(':').is_some() {
            key.to_string()
        } else {
            format!("{key}:{Z21_UDP_PORT}")
        }
    }
}

/// Outcome of `LAN_LOCONET_DISPATCH_ADDR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// FW ≥ 1.22: DISPATCH_PUT succeeded; value is the LocoNet slot.
    Slot(u8),
    /// Z21 answered the serial probe but sent no `0xA3` reply (FW &lt; 1.22).
    NoAck,
}

/// Encode a Z21 record.
pub fn encode(header: u16, data: &[u8]) -> Vec<u8> {
    let len = 4u16.saturating_add(u16::try_from(data.len()).unwrap_or(u16::MAX));
    let mut out = Vec::with_capacity(usize::from(len));
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&header.to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// `LAN_GET_SERIAL_NUMBER` request (also used as a UDP discovery probe).
pub fn serial_number_request() -> Vec<u8> {
    encode(HEADER_GET_SERIAL_NUMBER, &[])
}

/// `LAN_LOCONET_DISPATCH_ADDR` request: 16-bit loco address, little-endian.
pub fn dispatch_addr_request(loco: u16) -> Vec<u8> {
    encode(HEADER_LOCONET_DISPATCH_ADDR, &loco.to_le_bytes())
}

/// `LAN_LOGOFF` request.
pub fn logoff_request() -> Vec<u8> {
    encode(HEADER_LOGOFF, &[])
}

/// Walk concatenated Z21 records in one UDP datagram.
pub fn parse_records(buf: &[u8]) -> Vec<Record> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 4 <= buf.len() {
        let data_len = u16::from_le_bytes([buf[off], buf[off + 1]]) as usize;
        let header = u16::from_le_bytes([buf[off + 2], buf[off + 3]]);
        if data_len < 4 || off + data_len > buf.len() {
            break;
        }
        out.push(Record {
            header,
            data: buf[off + 4..off + data_len].to_vec(),
        });
        off += data_len;
    }
    out
}

/// Serial number from a `LAN_GET_SERIAL_NUMBER` reply payload.
pub fn parse_serial(data: &[u8]) -> Option<u32> {
    if data.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
}

/// `true` when this X-BUS record is `LAN_X_UNKNOWN_COMMAND`.
pub fn is_unknown_command(rec: &Record) -> bool {
    rec.header == HEADER_XBUS && rec.data.len() >= 2 && rec.data[0] == 0x61 && rec.data[1] == 0x82
}

/// Parse a DISPATCH_ADDR reply: `(loco, result)`.
///
/// `result == 0` means DISPATCH_PUT failed. `result > 0` is the slot number.
pub fn parse_dispatch_reply(data: &[u8]) -> Option<(u16, u8)> {
    if data.len() < 3 {
        return None;
    }
    let loco = u16::from_le_bytes([data[0], data[1]]);
    Some((loco, data[2]))
}

/// Discover Z21 LAN endpoints: mDNS `_z21._udp` plus UDP serial broadcast.
///
/// # Errors
///
/// Returns [`std::io::Error`] when both the mDNS socket and the UDP probe
/// socket fail to bind. An empty result is success.
pub fn discover_z21(wait: Duration) -> std::io::Result<Vec<Z21Host>> {
    let mut found: Vec<Z21Host> = Vec::new();

    match discover_mdns_hosts(Z21_UDP_SERVICE, wait) {
        Ok(hosts) => {
            for h in hosts {
                let port = if h.port == 0 {
                    log::debug!(
                        "z21 mdns: {} advertised without SRV port, defaulting to {}",
                        h.ipv4,
                        Z21_UDP_PORT
                    );
                    Z21_UDP_PORT
                } else {
                    h.port
                };
                push_host(
                    &mut found,
                    Z21Host {
                        hostname: h.hostname,
                        ipv4: h.ipv4,
                        port,
                        serial: None,
                    },
                );
            }
        }
        Err(e) => log::debug!("z21 mdns: {e}"),
    }

    match probe_serial_broadcast(wait) {
        Ok(hosts) => {
            for h in hosts {
                push_host(&mut found, h);
            }
        }
        Err(e) => log::debug!("z21 udp probe: {e}"),
    }

    Ok(found)
}

fn push_host(found: &mut Vec<Z21Host>, host: Z21Host) {
    if let Some(existing) = found
        .iter_mut()
        .find(|e| e.ipv4 == host.ipv4 && e.port == host.port)
    {
        if existing.hostname.is_empty() {
            existing.hostname.clone_from(&host.hostname);
        }
        if existing.serial.is_none() {
            existing.serial = host.serial;
        }
        return;
    }
    found.push(host);
}

/// Broadcast `LAN_GET_SERIAL_NUMBER` to 21105/21106 and collect replies.
fn probe_serial_broadcast(wait: Duration) -> std::io::Result<Vec<Z21Host>> {
    let sock = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    sock.set_broadcast(true)?;
    sock.set_read_timeout(Some(Duration::from_millis(200)))?;
    let req = serial_number_request();
    let _ = sock.send_to(&req, SocketAddrV4::new(Ipv4Addr::BROADCAST, Z21_UDP_PORT));
    let _ = sock.send_to(
        &req,
        SocketAddrV4::new(Ipv4Addr::BROADCAST, Z21_UDP_PORT_ALT),
    );

    let deadline = Instant::now() + wait;
    let mut found = Vec::new();
    let mut buf = [0u8; 1500];
    while Instant::now() < deadline {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                let SocketAddr::V4(v4) = from else {
                    continue;
                };
                for rec in parse_records(&buf[..n]) {
                    if rec.header != HEADER_GET_SERIAL_NUMBER {
                        continue;
                    }
                    push_host(
                        &mut found,
                        Z21Host {
                            hostname: String::new(),
                            ipv4: *v4.ip(),
                            port: v4.port(),
                            serial: parse_serial(&rec.data),
                        },
                    );
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e),
        }
    }
    Ok(found)
}

/// Send DISPATCH_PUT for `loco` to `target`.
///
/// # Errors
///
/// Socket, timeout with no prior serial reply, DISPATCH_PUT rejected
/// (`result == 0`), or `LAN_X_UNKNOWN_COMMAND`.
pub fn dispatch_addr(
    sock: &UdpSocket,
    target: SocketAddr,
    loco: u16,
) -> Result<DispatchOutcome, DispatchError> {
    sock.set_read_timeout(Some(Duration::from_millis(200)))?;
    let _ = sock.send_to(&serial_number_request(), target);

    let mut z21_alive = false;
    let serial_deadline = Instant::now() + SERIAL_PROBE_TIMEOUT;
    let mut buf = [0u8; 1500];
    while Instant::now() < serial_deadline {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) if from == target || from.ip() == target.ip() => {
                for rec in parse_records(&buf[..n]) {
                    if rec.header == HEADER_GET_SERIAL_NUMBER {
                        z21_alive = true;
                    }
                    if is_unknown_command(&rec) {
                        let _ = sock.send_to(&logoff_request(), target);
                        return Err(DispatchError::UnknownCommand);
                    }
                }
            }
            Ok(_) => {}
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(DispatchError::Io(e.to_string())),
        }
        if z21_alive {
            break;
        }
    }

    let _ = sock.send_to(&dispatch_addr_request(loco), target);
    let deadline = Instant::now() + DISPATCH_TIMEOUT;
    while Instant::now() < deadline {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) if from == target || from.ip() == target.ip() => {
                for rec in parse_records(&buf[..n]) {
                    if is_unknown_command(&rec) {
                        let _ = sock.send_to(&logoff_request(), target);
                        return Err(DispatchError::UnknownCommand);
                    }
                    if rec.header != HEADER_LOCONET_DISPATCH_ADDR {
                        continue;
                    }
                    let Some((addr, result)) = parse_dispatch_reply(&rec.data) else {
                        continue;
                    };
                    if addr != loco {
                        continue;
                    }
                    if result == 0 {
                        let _ = sock.send_to(&logoff_request(), target);
                        return Err(DispatchError::Rejected);
                    }
                    let _ = sock.send_to(&logoff_request(), target);
                    return Ok(DispatchOutcome::Slot(result));
                }
            }
            Ok(_) => {}
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(DispatchError::Io(e.to_string())),
        }
    }

    let _ = sock.send_to(&logoff_request(), target);
    if z21_alive {
        Ok(DispatchOutcome::NoAck)
    } else {
        Err(DispatchError::Unreachable(target.to_string()))
    }
}

/// Failures from [`dispatch_addr`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    /// No serial reply and no DISPATCH reply.
    Unreachable(String),
    /// `Result = 0` — DISPATCH_PUT rejected (Z21 is slave / slot busy).
    Rejected,
    /// Z21 does not speak LocoNet dispatch (`LAN_X_UNKNOWN_COMMAND`).
    UnknownCommand,
    /// Socket I/O.
    Io(String),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(a) => write!(f, "device unreachable: {a}"),
            Self::Rejected => write!(f, "DISPATCH_PUT rejected"),
            Self::UnknownCommand => write!(f, "z21 does not support LocoNet dispatch"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DispatchError {}

impl From<std::io::Error> for DispatchError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Re-export so callers can label mDNS-only hits without a second type.
pub type MdnsHost = OtaHost;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_dispatch_matches_spec() {
        let pkt = dispatch_addr_request(42);
        assert_eq!(pkt, vec![0x06, 0x00, 0xA3, 0x00, 42, 0x00]);
    }

    #[test]
    fn encode_serial_request() {
        assert_eq!(serial_number_request(), vec![0x04, 0x00, 0x10, 0x00]);
    }

    #[test]
    fn parse_concatenated_records() {
        let mut buf = serial_number_request();
        buf.extend_from_slice(&encode(
            HEADER_GET_SERIAL_NUMBER,
            &0x1234_5678u32.to_le_bytes(),
        ));
        let recs = parse_records(&buf);
        assert_eq!(recs.len(), 2);
        assert_eq!(parse_serial(&recs[1].data), Some(0x1234_5678));
    }

    #[test]
    fn parse_dispatch_ok_and_fail() {
        assert_eq!(parse_dispatch_reply(&[0x2A, 0x00, 0x03]), Some((42, 3)));
        assert_eq!(parse_dispatch_reply(&[0x2A, 0x00, 0x00]), Some((42, 0)));
        assert_eq!(parse_dispatch_reply(&[0x2A]), None);
    }

    #[test]
    fn unknown_command_detect() {
        let rec = Record {
            header: HEADER_XBUS,
            data: vec![0x61, 0x82, 0xE3],
        };
        assert!(is_unknown_command(&rec));
        assert!(!is_unknown_command(&Record {
            header: HEADER_XBUS,
            data: vec![0x61, 0x00, 0x61],
        }));
    }

    #[test]
    fn parse_key_with_and_without_port() {
        let with = Z21Host::parse_key("192.168.0.111:21105").unwrap();
        assert_eq!(with.to_string(), "192.168.0.111:21105");
        let bare = Z21Host::parse_key("10.0.0.5").unwrap();
        assert_eq!(bare.to_string(), "10.0.0.5:21105");
    }

    #[test]
    fn normalize_key_appends_default_port() {
        assert_eq!(Z21Host::normalize_key("10.0.0.5"), "10.0.0.5:21105");
        assert_eq!(
            Z21Host::normalize_key("192.168.0.111:21106"),
            "192.168.0.111:21106"
        );
        assert_eq!(Z21Host::normalize_key("z21.local:21105"), "z21.local:21105");
        assert_eq!(Z21Host::normalize_key("z21.local"), "z21.local:21105");
    }
}
