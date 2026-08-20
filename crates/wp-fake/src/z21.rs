//! In-process UDP Z21 mock for FRED dispatch tests.

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use wp_link::z21::{
    encode, parse_records, HEADER_GET_SERIAL_NUMBER, HEADER_LOCONET_DISPATCH_ADDR, HEADER_XBUS,
};

/// Behaviour of [`FakeZ21`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeZ21Mode {
    /// Reply to DISPATCH_ADDR with `result > 0`.
    Accept,
    /// Reply with `result = 0`.
    Reject,
    /// Reply `LAN_X_UNKNOWN_COMMAND`.
    UnknownCommand,
    /// Answer serial probe, ignore DISPATCH (FW &lt; 1.22).
    NoAck,
}

/// UDP Z21 mock. Dropping the handle stops the background thread.
pub struct FakeZ21 {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    dispatches: Arc<AtomicU8>,
}

impl FakeZ21 {
    /// Bind `127.0.0.1:0` and serve `mode`.
    pub fn spawn(mode: FakeZ21Mode) -> std::io::Result<Self> {
        let sock = UdpSocket::bind("127.0.0.1:0")?;
        sock.set_read_timeout(Some(Duration::from_millis(100)))?;
        let addr = sock.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let dispatches = Arc::new(AtomicU8::new(0));
        let stop_t = Arc::clone(&stop);
        let disp_t = Arc::clone(&dispatches);
        thread::spawn(move || loop {
            if stop_t.load(Ordering::Relaxed) {
                break;
            }
            let mut buf = [0u8; 1500];
            match sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    for rec in parse_records(&buf[..n]) {
                        let reply = match rec.header {
                            HEADER_GET_SERIAL_NUMBER => Some(encode(
                                HEADER_GET_SERIAL_NUMBER,
                                &0x00C0_FFEEu32.to_le_bytes(),
                            )),
                            HEADER_LOCONET_DISPATCH_ADDR => {
                                disp_t.fetch_add(1, Ordering::Relaxed);
                                let loco = if rec.data.len() >= 2 {
                                    u16::from_le_bytes([rec.data[0], rec.data[1]])
                                } else {
                                    0
                                };
                                match mode {
                                    FakeZ21Mode::Accept => {
                                        let mut data = loco.to_le_bytes().to_vec();
                                        data.push(3);
                                        Some(encode(HEADER_LOCONET_DISPATCH_ADDR, &data))
                                    }
                                    FakeZ21Mode::Reject => {
                                        let mut data = loco.to_le_bytes().to_vec();
                                        data.push(0);
                                        Some(encode(HEADER_LOCONET_DISPATCH_ADDR, &data))
                                    }
                                    FakeZ21Mode::UnknownCommand => {
                                        Some(encode(HEADER_XBUS, &[0x61, 0x82, 0xE3]))
                                    }
                                    FakeZ21Mode::NoAck => None,
                                }
                            }
                            _ => None,
                        };
                        if let Some(pkt) = reply {
                            let _ = sock.send_to(&pkt, from);
                        }
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break,
            }
        });
        Ok(Self {
            addr,
            stop,
            dispatches,
        })
    }

    /// Bound address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// How many DISPATCH_ADDR requests were seen.
    pub fn dispatch_count(&self) -> u8 {
        self.dispatches.load(Ordering::Relaxed)
    }
}

impl Drop for FakeZ21 {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wp_link::{dispatch_addr, DispatchOutcome};

    #[test]
    fn accept_returns_slot() {
        let fake = FakeZ21::spawn(FakeZ21Mode::Accept).unwrap();
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let out = dispatch_addr(&sock, fake.addr(), 42).unwrap();
        assert_eq!(out, DispatchOutcome::Slot(3));
    }

    #[test]
    fn reject_is_error() {
        let fake = FakeZ21::spawn(FakeZ21Mode::Reject).unwrap();
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let err = dispatch_addr(&sock, fake.addr(), 7).unwrap_err();
        assert!(matches!(err, wp_link::DispatchError::Rejected));
    }

    #[test]
    fn unknown_command_is_error() {
        let fake = FakeZ21::spawn(FakeZ21Mode::UnknownCommand).unwrap();
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let err = dispatch_addr(&sock, fake.addr(), 7).unwrap_err();
        assert!(matches!(err, wp_link::DispatchError::UnknownCommand));
    }

    #[test]
    fn no_ack_is_success_after_serial() {
        let fake = FakeZ21::spawn(FakeZ21Mode::NoAck).unwrap();
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let out = dispatch_addr(&sock, fake.addr(), 99).unwrap();
        assert_eq!(out, DispatchOutcome::NoAck);
    }
}
