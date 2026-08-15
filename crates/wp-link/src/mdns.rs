//! Minimal mDNS query for `_longfred-ota._tcp.local`.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

/// LongFred STA HTTP OTA service.
pub const OTA_HTTP_SERVICE: &str = "_longfred-ota._tcp.local";

const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;
const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_SRV: u16 = 33;

/// A LongFred advertising HTTP OTA on the LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtaHost {
    /// Instance / hostname label.
    pub hostname: String,
    /// IPv4 from an A record.
    pub ipv4: Ipv4Addr,
    /// SRV port (HTTP, typically 80).
    pub port: u16,
}

/// Send a PTR query and collect A/SRV answers for [`OTA_HTTP_SERVICE`].
///
/// # Errors
///
/// Returns [`std::io::Error`] on socket failure.
pub fn discover_ota_hosts(wait: Duration) -> std::io::Result<Vec<OtaHost>> {
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(Duration::from_millis(200)))?;
    sock.set_multicast_ttl_v4(1)?;
    let q = ptr_query(OTA_HTTP_SERVICE);
    sock.send_to(&q, SocketAddrV4::new(MDNS_GROUP, MDNS_PORT))?;

    let deadline = Instant::now() + wait;
    let mut found: Vec<OtaHost> = Vec::new();
    let mut buf = [0u8; 1500];
    while Instant::now() < deadline {
        match sock.recv_from(&mut buf) {
            Ok((n, _)) => {
                for h in parse_ota_hosts(&buf[..n]) {
                    if !found.iter().any(|e| e.ipv4 == h.ipv4 && e.port == h.port) {
                        found.push(h);
                    }
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

fn ptr_query(service: &str) -> Vec<u8> {
    let mut q = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in service.split('.') {
        q.push(u8::try_from(label.len()).unwrap_or(0));
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&[0x00, TYPE_PTR as u8, 0x00, 0x01]);
    q
}

fn be16(pkt: &[u8], off: usize) -> Option<u16> {
    Some((u16::from(*pkt.get(off)?) << 8) | u16::from(*pkt.get(off + 1)?))
}

fn read_name(pkt: &[u8], start: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut off = start;
    let mut next_after: Option<usize> = None;
    let mut jumps = 0usize;
    loop {
        let len = *pkt.get(off)?;
        if len == 0 {
            off += 1;
            break;
        }
        if len & 0xc0 == 0xc0 {
            let ptr = (usize::from(len & 0x3f) << 8) | usize::from(*pkt.get(off + 1)?);
            if next_after.is_none() {
                next_after = Some(off + 2);
            }
            jumps += 1;
            if jumps > 16 {
                return None;
            }
            off = ptr;
            continue;
        }
        let n = usize::from(len);
        off += 1;
        let bytes = pkt.get(off..off + n)?;
        labels.push(String::from_utf8_lossy(bytes).into_owned());
        off += n;
    }
    Some((labels.join("."), next_after.unwrap_or(off)))
}

/// Parse A/SRV records from an mDNS packet (host-testable).
pub fn parse_ota_hosts(pkt: &[u8]) -> Vec<OtaHost> {
    let mut out = Vec::new();
    if pkt.len() < 12 {
        return out;
    }
    let an = be16(pkt, 6).unwrap_or(0);
    let ns = be16(pkt, 8).unwrap_or(0);
    let ar = be16(pkt, 10).unwrap_or(0);
    let mut off = 12usize;
    let mut port = 80u16;
    let mut hostname = String::new();
    for _ in 0..an.saturating_add(ns).saturating_add(ar) {
        let Some((name, nend)) = read_name(pkt, off) else {
            break;
        };
        off = nend;
        let Some(typ) = be16(pkt, off) else { break };
        off += 8;
        let Some(rdlen) = be16(pkt, off) else { break };
        off += 2;
        let rdata = off;
        off = off.saturating_add(usize::from(rdlen));
        if typ == TYPE_SRV && rdlen >= 6 {
            if let Some(p) = be16(pkt, rdata + 4) {
                port = p;
            }
            hostname = name.split('.').next().unwrap_or("longfred").to_string();
        }
        if typ == TYPE_A && rdlen == 4 {
            if let (Some(&a), Some(&b), Some(&c), Some(&d)) = (
                pkt.get(rdata),
                pkt.get(rdata + 1),
                pkt.get(rdata + 2),
                pkt.get(rdata + 3),
            ) {
                let host = if hostname.is_empty() {
                    name.split('.').next().unwrap_or("longfred").to_string()
                } else {
                    hostname.clone()
                };
                out.push(OtaHost {
                    hostname: host,
                    ipv4: Ipv4Addr::new(a, b, c, d),
                    port,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ptr_query_contains_service_labels() {
        let q = ptr_query(OTA_HTTP_SERVICE);
        assert!(q
            .windows(b"_longfred-ota".len())
            .any(|w| w == b"_longfred-ota"));
    }

    #[test]
    fn parse_empty_packet() {
        assert!(parse_ota_hosts(&[]).is_empty());
    }
}
