//! RailBOX RB23xx Soft-AP constants.

#![allow(dead_code)]

use std::net::Ipv4Addr;

/// SSID prefixes of the decoder Soft-AP (`RB2300_XXXXX`, `RB2310_…`, `RB2302_…`).
pub const WIFI_CONFIG_SSID_PREFIXES: &[&str] = &["RB2300_", "RB2310_", "RB2302_"];

/// Factory Soft-AP passphrase. Never logged.
pub const SOFTAP_PSK: &str = "000000000";

/// Config AP HTTP port.
pub const CONFIG_AP_PORT: u16 = 80;

/// Config AP address (ESP-IDF Soft-AP default).
pub const CONFIG_HOST: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);

/// Source address the daemon assigns to the wireless interface.
pub const CONFIG_SOURCE: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 2);

/// On-link prefix length for the config AP subnet.
pub const CONFIG_PREFIX_LEN: u8 = 24;

/// Firmware POST cap (5 MiB). Distinct from the LongFred OTA slot.
pub const MAX_FIRMWARE_BYTES: u64 = 5 * 1024 * 1024;

/// Root listing used to confirm the file browser is up.
pub const LIST_PATH: &str = "/?p=/";

/// Upload content type. The decoder expects this header with a raw body
/// (no multipart boundary), matching the `rb` CLI.
pub const UPLOAD_CONTENT_TYPE: &str = "multipart/form-data";

/// RB23xx does not store a throttle roster over HTTP.
pub const MAX_ROSTER_SLOTS: u8 = 0;

/// RB23xx does not write function maps over HTTP.
pub const MAX_FUNCTION: u8 = 0;
