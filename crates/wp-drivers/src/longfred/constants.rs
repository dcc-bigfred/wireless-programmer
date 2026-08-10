//! LongFred Soft-AP programming constants.

#![allow(dead_code)]

use std::net::Ipv4Addr;

/// SSID prefix of the programming Soft-AP (`longfred_prog_XXXXXX`).
pub const WIFI_CONFIG_SSID_PREFIX: &str = "longfred_prog";

/// Config AP HTTP port.
pub const CONFIG_AP_PORT: u16 = 80;

/// Config AP address (firmware static Soft-AP IP).
pub const CONFIG_HOST: Ipv4Addr = Ipv4Addr::new(192, 168, 0, 1);

/// Source address the daemon assigns to the wireless interface.
pub const CONFIG_SOURCE: Ipv4Addr = Ipv4Addr::new(192, 168, 0, 2);

/// On-link prefix length for the config AP subnet.
pub const CONFIG_PREFIX_LEN: u8 = 24;

/// LongFred static roster capacity (`MAX_SAVED_LOCOS`).
pub const MAX_ROSTER_SLOTS: u8 = 12;

/// LongFred programming does not write per-function maps via `/settings`.
pub const MAX_FUNCTION: u8 = 0;

/// Settings read endpoint.
pub const SETTINGS_PATH: &str = "/api/v1/settings";

/// Exit programming mode endpoint.
pub const PROGRAMMING_MODE_OFF_PATH: &str = "/api/v1/programming-mode/off";

/// JSON content type for PUT bodies.
pub const JSON_CONTENT_TYPE: &str = "application/json";
