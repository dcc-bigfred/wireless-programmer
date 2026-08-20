//! Minimal mDNS query for `_longfred-ota._tcp.local`.
//!
//! LongFred STA OTA does not answer PTR queries. It sends unsolicited
//! announcements to `224.0.0.251:5353` every 2 s while the Firmware update
//! menu is open. Discovery therefore **joins the multicast group and binds
//! 5353** so those packets are received.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

/// LongFred STA HTTP OTA service.
pub const OTA_HTTP_SERVICE: &str = "_longfred-ota._tcp.local";

/// Z21 LAN protocol DNS-SD type (Roco Z21 / BigFred inbound / RB1110).
pub const Z21_UDP_SERVICE: &str = "_z21._udp.local";

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

fn mdns_listener() -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MDNS_PORT).into())?;
    socket.join_multicast_v4(&MDNS_GROUP, &Ipv4Addr::UNSPECIFIED)?;
    socket.set_multicast_ttl_v4(1)?;
    let sock = UdpSocket::from(socket);
    sock.set_read_timeout(Some(Duration::from_millis(200)))?;
    Ok(sock)
}

/// Send a PTR query and collect A/SRV answers / unsolicited announcements
/// for [`OTA_HTTP_SERVICE`].
///
/// # Errors
///
/// Returns [`std::io::Error`] on socket failure (including inability to bind
/// UDP 5353).
pub fn discover_ota_hosts(wait: Duration) -> std::io::Result<Vec<OtaHost>> {
    discover_mdns_hosts(OTA_HTTP_SERVICE, wait)
}

/// PTR-query `service` (e.g. [`Z21_UDP_SERVICE`]) and collect A/SRV answers.
///
/// # Errors
///
/// Returns [`std::io::Error`] on socket failure (including inability to bind
/// UDP 5353).
pub fn discover_mdns_hosts(service: &str, wait: Duration) -> std::io::Result<Vec<OtaHost>> {
    let sock = mdns_listener()?;
    let q = ptr_query(service);
    let _ = sock.send_to(&q, SocketAddrV4::new(MDNS_GROUP, MDNS_PORT));

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

fn skip_questions(pkt: &[u8], mut off: usize, qd: u16) -> Option<usize> {
    for _ in 0..qd {
        let (_, nend) = read_name(pkt, off)?;
        off = nend.checked_add(4)?; // TYPE + CLASS
    }
    Some(off)
}

/// Parse A/SRV records from an mDNS packet (host-testable).
pub fn parse_ota_hosts(pkt: &[u8]) -> Vec<OtaHost> {
    let mut out = Vec::new();
    if pkt.len() < 12 {
        return out;
    }
    let qd = be16(pkt, 4).unwrap_or(0);
    let an = be16(pkt, 6).unwrap_or(0);
    let ns = be16(pkt, 8).unwrap_or(0);
    let ar = be16(pkt, 10).unwrap_or(0);
    let Some(mut off) = skip_questions(pkt, 12, qd) else {
        return out;
    };
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

/// Build an unsolicited OTA announcement matching LongFred firmware
/// (`build_ota_announce`): 0 questions, PTR + SRV + A.
pub fn encode_ota_announce(hostname: &str, ipv4: Ipv4Addr, port: u16) -> Vec<u8> {
    let mut n = Vec::new();
    n.extend_from_slice(&[0, 0, 0x84, 0, 0, 0, 0, 3, 0, 0, 0, 0]);
    put_name(&mut n, &["_longfred-ota", "_tcp", "local"]);
    n.extend_from_slice(&[0, 12, 0, 1, 0, 0, 0, 120]);
    let instance = [hostname, "_longfred-ota", "_tcp", "local"];
    let instance_len = name_len(&instance);
    n.extend_from_slice(&u16::try_from(instance_len).unwrap_or(0).to_be_bytes());
    put_name(&mut n, &instance);
    put_name(&mut n, &[hostname, "_longfred-ota", "_tcp", "local"]);
    n.extend_from_slice(&[0, 33, 0, 1, 0, 0, 0, 120]);
    let target = [hostname, "local"];
    let target_len = 6 + name_len(&target);
    n.extend_from_slice(&u16::try_from(target_len).unwrap_or(0).to_be_bytes());
    n.extend_from_slice(&[0, 0, 0, 0]);
    n.extend_from_slice(&port.to_be_bytes());
    put_name(&mut n, &target);
    put_name(&mut n, &target);
    n.extend_from_slice(&[0, 1, 0, 1, 0, 0, 0, 120, 0, 4]);
    n.extend_from_slice(&ipv4.octets());
    n
}

fn name_len(labels: &[&str]) -> usize {
    labels.iter().map(|l| 1 + l.len()).sum::<usize>() + 1
}

fn put_name(buf: &mut Vec<u8>, labels: &[&str]) {
    for lab in labels {
        buf.push(u8::try_from(lab.len()).unwrap_or(0));
        buf.extend_from_slice(lab.as_bytes());
    }
    buf.push(0);
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
        let z21 = ptr_query(Z21_UDP_SERVICE);
        assert!(z21.windows(b"_z21".len()).any(|w| w == b"_z21"));
        assert!(z21.windows(b"_udp".len()).any(|w| w == b"_udp"));
    }

    #[test]
    fn parse_empty_packet() {
        assert!(parse_ota_hosts(&[]).is_empty());
    }

    #[test]
    fn parse_longfred_unsolicited_announce() {
        let pkt = encode_ota_announce("pilot1", Ipv4Addr::new(192, 168, 1, 40), 80);
        let hosts = parse_ota_hosts(&pkt);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].ipv4, Ipv4Addr::new(192, 168, 1, 40));
        assert_eq!(hosts[0].port, 80);
        assert_eq!(hosts[0].hostname, "pilot1");
    }

    #[test]
    fn parse_skips_question_section() {
        let announce = encode_ota_announce("pilot1", Ipv4Addr::new(10, 0, 0, 9), 80);
        // Prepend a query header with QDCOUNT=1 and one question, then the
        // original answers with ANCOUNT preserved from `announce`.
        let mut pkt = vec![0, 0, 0x84, 0, 0, 1]; // flags + QD=1
        pkt.extend_from_slice(&announce[6..12]); // AN/NS/AR from announce
        put_name(&mut pkt, &["_longfred-ota", "_tcp", "local"]);
        pkt.extend_from_slice(&[0, 12, 0, 1]); // PTR IN
        pkt.extend_from_slice(&announce[12..]);
        let hosts = parse_ota_hosts(&pkt);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].ipv4, Ipv4Addr::new(10, 0, 0, 9));
    }
}
