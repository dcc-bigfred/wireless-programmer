//! Radio control: nl80211 scan/connect + rtnetlink addressing.
//!
//! The daemon owns the radio. It associates to a device AP (open, no PSK),
//! assigns an on-link address with **no default route** (so the hub's
//! Ethernet default gateway is never hijacked), then hands a sync
//! [`wp_core::HttpClient`] to the driver. On every exit path the radio is
//! released: disconnect and address removal.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use wp_core::DriverError;

/// A scan result from the radio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanResult {
    /// SSID, when present.
    pub ssid: Option<String>,
    /// BSSID, when present.
    pub bssid: Option<String>,
    /// Signal strength in dBm, when known.
    pub rssi: Option<i32>,
}

/// Boxed future returned by [`Radio`] methods (dyn-compatible).
pub type RadioFut<'a, T> = Pin<Box<dyn Future<Output = Result<T, DriverError>> + Send + 'a>>;

/// The async radio contract. Implementations use nl80211 + rtnetlink.
///
/// Methods return boxed futures so the trait is dyn-compatible
/// (`Box<dyn Radio>` in the daemon runtime).
pub trait Radio: Send {
    /// Trigger a scan and return up to `max` results.
    fn scan(&mut self, max: usize) -> RadioFut<'_, Vec<ScanResult>>;

    /// Associate to an open AP identified by SSID (and optional BSSID hint).
    fn connect_open(&mut self, ssid: &str, bssid: Option<[u8; 6]>) -> RadioFut<'_, ()>;

    /// Assign `addr/prefix_len` to the wireless interface (on-link route only).
    fn set_address(&mut self, addr: std::net::Ipv4Addr, prefix_len: u8) -> RadioFut<'_, ()>;

    /// Bring the link up.
    fn link_up(&mut self) -> RadioFut<'_, ()>;

    /// Disconnect and remove the assigned address, releasing the radio.
    fn release(&mut self) -> RadioFut<'_, ()>;
}

/// Select the first wireless interface by scanning `/sys/class/net/*/wireless`.
///
/// # Errors
///
/// Returns [`DriverError::NoInterface`] when none is found.
pub fn first_wireless_interface() -> Result<String, DriverError> {
    let dir = Path::new("/sys/class/net");
    let entries = std::fs::read_dir(dir)
        .map_err(|e| DriverError::Other(format!("cannot read /sys/class/net: {e}")))?;
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.path().join("wireless").exists() {
            return Ok(name.into_owned());
        }
    }
    Err(DriverError::NoInterface)
}

/// Return `true` when `/sys/class/net/{name}/wireless` exists.
#[must_use]
pub fn is_wireless_interface(name: &str) -> bool {
    !name.is_empty()
        && Path::new("/sys/class/net")
            .join(name)
            .join("wireless")
            .exists()
}

/// Resolve the wireless interface to use.
///
/// When `preferred` is `Some(name)`, that name must exist and be wireless.
/// When `None`, the first wireless interface is selected (same as
/// [`first_wireless_interface`]).
///
/// # Errors
///
/// - [`DriverError::NoInterface`] when auto-select finds nothing.
/// - [`DriverError::Other`] when a preferred name is empty, missing, or not
///   wireless.
pub fn resolve_wireless_interface(preferred: Option<&str>) -> Result<String, DriverError> {
    match preferred {
        None => first_wireless_interface(),
        Some(name) => {
            let name = name.trim();
            if name.is_empty() {
                return Err(DriverError::Other(
                    "wireless interface name must not be empty".into(),
                ));
            }
            let path = Path::new("/sys/class/net").join(name);
            if !path.exists() {
                return Err(DriverError::Other(format!(
                    "interface {name} does not exist"
                )));
            }
            if !path.join("wireless").exists() {
                return Err(DriverError::Other(format!(
                    "interface {name} is not wireless"
                )));
            }
            Ok(name.to_string())
        }
    }
}

/// Resolve an interface name to its netlink ifindex via `/sys/class/net`.
fn interface_index(name: &str) -> Result<u32, DriverError> {
    let p = Path::new("/sys/class/net").join(name).join("ifindex");
    let s = std::fs::read_to_string(&p)
        .map_err(|e| DriverError::Other(format!("cannot read ifindex for {name}: {e}")))?;
    s.trim()
        .parse::<u32>()
        .map_err(|e| DriverError::Other(format!("bad ifindex for {name}: {e}")))
}

/// Format a MAC as lowercase colon-separated hex.
fn format_bssid(mac: &[u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

/// Extract an SSID from raw 802.11 information elements (TLV: id, len, data).
fn ssid_from_ies(ies: &[u8]) -> Option<String> {
    let mut i = 0;
    while i + 1 < ies.len() {
        let id = ies[i];
        let len = usize::from(ies[i + 1]);
        if i + 2 + len > ies.len() {
            break;
        }
        if id == 0 {
            let bytes = &ies[i + 2..i + 2 + len];
            if bytes.is_empty() {
                return None;
            }
            return Some(String::from_utf8_lossy(bytes).into_owned());
        }
        i += 2 + len;
    }
    None
}

/// Parse one BSS info vector into a [`ScanResult`].
pub fn parse_bss_infos(bss: &[wl_nl80211::Nl80211BssInfo]) -> Option<ScanResult> {
    use wl_nl80211::Nl80211BssInfo;

    let mut ssid = None;
    let mut bssid = None;
    let mut rssi = None;

    for info in bss {
        match info {
            Nl80211BssInfo::Bssid(mac) => {
                bssid = Some(format_bssid(mac));
            }
            Nl80211BssInfo::SignalMbm(mbm) => {
                rssi = Some(mbm / 100);
            }
            Nl80211BssInfo::RawInformationElements(ies)
            | Nl80211BssInfo::RawBeaconInformationElements(ies)
            | Nl80211BssInfo::RawProbeResponseInformationElements(ies)
                if ssid.is_none() =>
            {
                ssid = ssid_from_ies(ies);
            }
            _ => {}
        }
    }

    if ssid.is_none() && bssid.is_none() {
        return None;
    }
    Some(ScanResult { ssid, bssid, rssi })
}

/// Parse a dump message's attributes into a [`ScanResult`].
pub fn parse_scan_attrs(attrs: &[wl_nl80211::Nl80211Attr]) -> Option<ScanResult> {
    for attr in attrs {
        if let wl_nl80211::Nl80211Attr::Bss(bss) = attr {
            return parse_bss_infos(bss);
        }
    }
    None
}

/// `wl-nl80211` + `rtnetlink` backed radio.
///
/// Construct with [`Nl80211Radio::new`] or [`Nl80211Radio::with_interface`];
/// requires `CAP_NET_ADMIN` and `CAP_NET_RAW`. All operations are async and
/// run on a tokio runtime.
pub struct Nl80211Radio {
    iface: String,
    if_index: u32,
}

impl Nl80211Radio {
    /// Bind to the first wireless interface.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::NoInterface`] when no wireless interface exists.
    pub fn new() -> Result<Self, DriverError> {
        Self::with_interface_opt(None)
    }

    /// Bind to a named wireless interface (e.g. `wlan0`, `wlp2s0`).
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::Other`] when the name is missing or not wireless.
    pub fn with_interface(name: &str) -> Result<Self, DriverError> {
        Self::with_interface_opt(Some(name))
    }

    /// Bind to `preferred` when set, otherwise the first wireless interface.
    ///
    /// # Errors
    ///
    /// See [`resolve_wireless_interface`].
    pub fn with_interface_opt(preferred: Option<&str>) -> Result<Self, DriverError> {
        let iface = resolve_wireless_interface(preferred)?;
        let if_index = interface_index(&iface)?;
        Ok(Self { iface, if_index })
    }

    /// Interface name.
    #[must_use]
    pub fn iface(&self) -> &str {
        &self.iface
    }
}

impl Radio for Nl80211Radio {
    fn scan(&mut self, max: usize) -> RadioFut<'_, Vec<ScanResult>> {
        let if_index = self.if_index;
        Box::pin(async move {
            use futures::stream::TryStreamExt;
            use wl_nl80211::Nl80211Scan;

            let (connection, handle, _) = wl_nl80211::new_connection()
                .map_err(|e| DriverError::Other(format!("nl80211 connection: {e}")))?;
            tokio::spawn(connection);

            // Trigger a passive scan, then dump the cached results.
            let attrs = Nl80211Scan::new(if_index).passive(true).build();
            let mut trigger = handle.scan().trigger(attrs).execute().await;
            while trigger.try_next().await.is_ok() {
                // drain acks
            }
            // Give the kernel a moment to populate the cache.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            let mut dump = handle.scan().dump(if_index).execute().await;
            let mut results = Vec::new();
            while let Ok(Some(msg)) = dump.try_next().await {
                if results.len() >= max {
                    break;
                }
                if let Some(r) = parse_scan_attrs(&msg.payload.attributes) {
                    results.push(r);
                }
            }
            Ok(results)
        })
    }

    fn connect_open(&mut self, ssid: &str, bssid: Option<[u8; 6]>) -> RadioFut<'_, ()> {
        let if_index = self.if_index;
        let ssid = ssid.to_string();
        Box::pin(async move {
            use futures::stream::TryStreamExt;
            use wl_nl80211::{Nl80211AuthType, Nl80211Connect};

            let (connection, handle, _) = wl_nl80211::new_connection()
                .map_err(|e| DriverError::Other(format!("nl80211 connection: {e}")))?;
            tokio::spawn(connection);

            let mut builder = Nl80211Connect::new(if_index)
                .ssid(&ssid)
                .auth_type(Nl80211AuthType::OpenSystem)
                .privacy(false);
            if let Some(mac) = bssid {
                builder = builder.mac(mac);
            }
            let attrs = builder.build();

            let mut stream = handle.connection().connect(attrs).execute().await;
            while stream.try_next().await.is_ok() {
                // drain acks
            }
            Ok(())
        })
    }

    fn set_address(&mut self, addr: std::net::Ipv4Addr, prefix_len: u8) -> RadioFut<'_, ()> {
        let if_index = self.if_index;
        Box::pin(async move {
            use rtnetlink::new_connection;

            let (connection, handle, _) = new_connection()
                .map_err(|e| DriverError::Other(format!("rtnetlink connection: {e}")))?;
            tokio::spawn(connection);

            handle
                .address()
                .add(if_index, std::net::IpAddr::V4(addr), prefix_len)
                .execute()
                .await
                .map_err(|e| DriverError::Other(format!("address add: {e}")))
        })
    }

    fn link_up(&mut self) -> RadioFut<'_, ()> {
        let if_index = self.if_index;
        Box::pin(async move {
            use rtnetlink::{new_connection, LinkUnspec};

            let (connection, handle, _) = new_connection()
                .map_err(|e| DriverError::Other(format!("rtnetlink connection: {e}")))?;
            tokio::spawn(connection);
            let msg = LinkUnspec::new_with_index(if_index).up().build();
            handle
                .link()
                .set(msg)
                .execute()
                .await
                .map_err(|e| DriverError::Other(format!("link up: {e}")))
        })
    }

    fn release(&mut self) -> RadioFut<'_, ()> {
        let if_index = self.if_index;
        Box::pin(async move {
            use futures::stream::TryStreamExt;
            use wl_nl80211::Nl80211Disconnect;

            // Best-effort disconnect; report only hard failures.
            if let Ok((connection, handle, _)) = wl_nl80211::new_connection() {
                tokio::spawn(connection);
                let attrs = Nl80211Disconnect::new(if_index).build();
                let mut stream = handle.connection().disconnect(attrs).execute().await;
                let _ = stream.try_next().await;
            }

            // Best-effort link down; the interface staying up is harmless (no
            // default route, no address left after the kernel clears it on
            // disconnect), but bringing it down is tidy.
            use rtnetlink::{new_connection, LinkUnspec};
            if let Ok((connection, handle, _)) = new_connection() {
                tokio::spawn(connection);
                let msg = LinkUnspec::new_with_index(if_index).down().build();
                let _ = handle.link().set(msg).execute().await;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wl_nl80211::Nl80211BssInfo;

    #[test]
    fn resolve_rejects_empty_preferred() {
        let err = resolve_wireless_interface(Some("")).expect_err("empty");
        assert!(
            matches!(err, DriverError::Other(ref m) if m.contains("empty")),
            "{err:?}"
        );
        let err = resolve_wireless_interface(Some("   ")).expect_err("whitespace");
        assert!(
            matches!(err, DriverError::Other(ref m) if m.contains("empty")),
            "{err:?}"
        );
    }

    #[test]
    fn resolve_rejects_missing_interface() {
        let err = resolve_wireless_interface(Some("wlan-does-not-exist-xyz")).expect_err("missing");
        assert!(
            matches!(err, DriverError::Other(ref m) if m.contains("does not exist")),
            "{err:?}"
        );
    }

    #[test]
    fn resolve_rejects_non_wireless_loopback() {
        // `lo` exists on every Linux host and is never wireless.
        if !Path::new("/sys/class/net/lo").exists() {
            return;
        }
        let err = resolve_wireless_interface(Some("lo")).expect_err("not wireless");
        assert!(
            matches!(err, DriverError::Other(ref m) if m.contains("not wireless")),
            "{err:?}"
        );
        assert!(!is_wireless_interface("lo"));
    }

    #[test]
    fn resolve_accepts_existing_wireless_when_present() {
        let Ok(first) = first_wireless_interface() else {
            return;
        };
        let resolved = resolve_wireless_interface(Some(&first)).expect("named wireless");
        assert_eq!(resolved, first);
        assert!(is_wireless_interface(&first));
    }

    #[test]
    fn ssid_from_ies_reads_test_wifi() {
        // IE: id=0, len=9, "Test-WIFI"
        let ies = [
            0u8, 9, b'T', b'e', b's', b't', b'-', b'W', b'I', b'F', b'I', 1, 8, 130, 132, 139, 150,
            12, 18, 24, 36,
        ];
        assert_eq!(ssid_from_ies(&ies).as_deref(), Some("Test-WIFI"));
    }

    #[test]
    fn parse_bss_infos_from_fixture() {
        let bss = vec![
            Nl80211BssInfo::Bssid([214, 178, 106, 168, 188, 177]),
            Nl80211BssInfo::RawInformationElements(vec![
                0, 9, 84, 101, 115, 116, 45, 87, 73, 70, 73, 1, 8, 130, 132, 139, 150, 12, 18, 24,
                36,
            ]),
            Nl80211BssInfo::SignalMbm(-3000),
        ];
        let r = parse_bss_infos(&bss).expect("parsed");
        assert_eq!(r.ssid.as_deref(), Some("Test-WIFI"));
        assert_eq!(r.bssid.as_deref(), Some("d6:b2:6a:a8:bc:b1"));
        assert_eq!(r.rssi, Some(-30));
    }

    #[test]
    fn parse_scan_attrs_finds_bss() {
        let attrs = vec![wl_nl80211::Nl80211Attr::Bss(vec![
            Nl80211BssInfo::Bssid([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]),
            Nl80211BssInfo::RawInformationElements(vec![0, 4, b't', b'e', b's', b't']),
            Nl80211BssInfo::SignalMbm(-5500),
        ])];
        let r = parse_scan_attrs(&attrs).expect("parsed");
        assert_eq!(r.ssid.as_deref(), Some("test"));
        assert_eq!(r.bssid.as_deref(), Some("aa:bb:cc:dd:ee:ff"));
        assert_eq!(r.rssi, Some(-55));
    }
}
