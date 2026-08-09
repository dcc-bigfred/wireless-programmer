//! Transport adapters for `wireless-programmer`: bounded HTTP/1.1 client
//! and radio control (nl80211 + rtnetlink).

#![forbid(unsafe_code)]

pub mod http;
pub mod radio;
pub mod rfkill;

pub use http::{percent_encode, BoundedHttpClient, MAX_BODY_BYTES};
pub use radio::{first_wireless_interface, Nl80211Radio, Radio, ScanResult};
pub use rfkill::{aggregate_state, RfkillState};
