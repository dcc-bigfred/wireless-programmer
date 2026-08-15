//! Transport adapters for `wireless-programmer`: bounded HTTP/1.1 client
//! and radio control (nl80211 + rtnetlink).

#![forbid(unsafe_code)]

pub mod http;
pub mod mdns;
pub mod radio;
pub mod rfkill;

pub use http::{percent_encode, BoundedHttpClient, MAX_BODY_BYTES};
pub use mdns::{discover_ota_hosts, parse_ota_hosts, OtaHost, OTA_HTTP_SERVICE};
pub use radio::{
    first_wireless_interface, is_wireless_interface, parse_bss_infos, parse_scan_attrs,
    resolve_wireless_interface, Nl80211Radio, Radio, RadioFut, ScanResult,
};
pub use rfkill::{aggregate_state, RfkillState};
