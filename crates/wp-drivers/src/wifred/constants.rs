//! WiFred firmware constants.
//!
//! Verified against `newHeiko/wiFred@master` `software/esp-firmware/`.

#![allow(dead_code)]

/// SSID prefix of the config AP. The firmware builds the SSID as
/// `"wiFred-config" + String(mac[3],16) + String(mac[4],16) + String(mac[5],16)`.
/// Arduino's hex conversion does **not** zero-pad, so match on the prefix,
/// never on a fixed length.
pub const WIFI_CONFIG_SSID_PREFIX: &str = "wiFred-config";

/// Config AP HTTP port (firmware default `WebServer server(80)`).
pub const CONFIG_AP_PORT: u16 = 80;

/// Config AP address. The firmware never calls `softAPConfig`, so the ESP-IDF
/// default holds: `192.168.4.1/24`.
pub const CONFIG_HOST: &str = "192.168.4.1";

/// Source address the daemon assigns to the wireless interface. Stays inside
/// the AP's `/24` and never gets a default route.
pub const CONFIG_SOURCE_ADDR: &str = "192.168.4.2";

/// On-link prefix length for the config AP subnet.
pub const CONFIG_PREFIX_LEN: u8 = 24;

/// WiFred stores exactly 4 loco slots (`locos[4]` in `locoHandling.h`).
pub const MAX_ROSTER_SLOTS: u8 = 4;

/// Highest function index the firmware understands (`MAX_FUNCTION = 16`).
pub const MAX_FUNCTION: u8 = 16;

/// XML structure version the firmware reports (`<structurVersion value="1"/>`).
pub const STRUCTURE_VERSION: &str = "1";

/// `functionInfo` enum values from `locoHandling.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FunctionInfo {
    /// Throttle-controlled.
    Throttle = 0,
    /// Throttle-controlled, force momentary.
    ThrottleMomentary = 1,
    /// Throttle-controlled, force locking.
    ThrottleLocking = 2,
    /// Throttle-controlled if this is the only loco.
    ThrottleSingle = 3,
    /// Force function always on.
    AlwaysOn = 4,
    /// Force function always off.
    AlwaysOff = 5,
    /// Ignore function key.
    Ignore = 6,
}

/// `eDirection` enum values from `locoHandling.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Direction {
    /// Forward.
    Normal = 0,
    /// Reverse.
    Reverse = 1,
    /// Do not change.
    DontChange = 2,
}

/// Speed-step mode strings accepted by the firmware (`MODES` table in
/// `locoHandling.cpp`). The empty string means "do not set speed step mode".
pub const MODES: &[&str] = &[
    "",
    "128",
    "28",
    "27",
    "14",
    "motorola_28",
    "tmcc_32",
    "incremental",
    "1",
    "2",
    "4",
    "8",
    "16",
];
