//! rfkill state inspection via `/sys/class/rfkill/*`.
//!
//! Avoids `/dev/rfkill` (which needs a dedicated fd and event loop) for a
//! simple read-only status check.

use std::fs;
use std::path::Path;

/// rfkill state for one switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RfkillState {
    /// Whether the radio is soft-blocked (userspace).
    pub soft: bool,
    /// Whether the radio is hard-blocked (hardware).
    pub hard: bool,
}

impl RfkillState {
    /// Returns `true` when the radio is blocked by either soft or hard block.
    pub fn blocked(self) -> bool {
        self.soft || self.hard
    }
}

/// Read the aggregate rfkill state across all switches.
///
/// Returns [`None`] when no rfkill switches exist (the radio is unmanaged).
///
/// # Errors
///
/// Returns [`std::io::Error`] on read failure.
pub fn aggregate_state() -> std::io::Result<Option<RfkillState>> {
    let dir = Path::new("/sys/class/rfkill");
    let read = dir.read_dir();
    let entries = match read {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };

    let mut soft = false;
    let mut hard = false;
    let mut any = false;
    for entry in entries {
        let entry = entry?;
        let p = entry.path();
        let read_one = |name: &str| -> std::io::Result<bool> {
            let v = fs::read_to_string(p.join(name))?;
            Ok(v.trim() == "1")
        };
        soft |= read_one("soft")?;
        hard |= read_one("hard")?;
        any = true;
    }
    if any {
        Ok(Some(RfkillState { soft, hard }))
    } else {
        Ok(None)
    }
}
