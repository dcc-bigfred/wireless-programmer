//! Transport adapters for `wireless-programmer`: bounded HTTP/1.1 client
//! and radio control (nl80211 + rtnetlink).

#![forbid(unsafe_code)]

pub mod espflash;
pub mod http;
pub mod mdns;
pub mod radio;
pub mod rfkill;
pub mod z21;

pub use espflash::{
    classify_image, flash as flash_usb, flash_argv, list_usb_ports, parse_list_ports_output,
    resolve_partition_table, ImageKind, UsbPort, CHIP, FLASH_SIZE, OTA0_OFFSET, USB_FLASH_DEADLINE,
};

pub use http::{percent_encode, BoundedHttpClient, MAX_BODY_BYTES};
pub use mdns::{
    discover_mdns_hosts, discover_ota_hosts, parse_ota_hosts, OtaHost, OTA_HTTP_SERVICE,
    Z21_UDP_SERVICE,
};
pub use radio::{
    first_wireless_interface, is_wireless_interface, parse_bss_infos, parse_scan_attrs,
    resolve_wireless_interface, Nl80211Radio, Radio, RadioFut, ScanResult,
};
pub use rfkill::{aggregate_state, RfkillState};
pub use z21::{
    discover_z21, dispatch_addr, DispatchError, DispatchOutcome, Z21Host, Z21_UDP_PORT,
};
