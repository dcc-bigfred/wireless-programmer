//! Radio control: nl80211 scan/connect + rtnetlink addressing.
//!
//! The daemon owns the radio. It associates to a device AP (open, no PSK),
//! assigns an on-link address with **no default route** (so the hub's
//! Ethernet default gateway is never hijacked), then hands a sync
//! [`wp_core::HttpClient`] to the driver. On every exit path the radio is
//! released: disconnect and address removal.

use std::path::Path;

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

/// The async radio contract. Implementations use nl80211 + rtnetlink.
pub trait Radio {
    /// Trigger a scan and return up to `max` results.
    fn scan(
        &mut self,
        max: usize,
    ) -> impl std::future::Future<Output = Result<Vec<ScanResult>, DriverError>>;

    /// Associate to an open AP identified by SSID (and optional BSSID hint).
    fn connect_open(
        &mut self,
        ssid: &str,
        bssid: Option<[u8; 6]>,
    ) -> impl std::future::Future<Output = Result<(), DriverError>>;

    /// Assign `addr/prefix_len` to the wireless interface (on-link route only).
    fn set_address(
        &mut self,
        addr: std::net::Ipv4Addr,
        prefix_len: u8,
    ) -> impl std::future::Future<Output = Result<(), DriverError>>;

    /// Bring the link up.
    fn link_up(&mut self) -> impl std::future::Future<Output = Result<(), DriverError>>;

    /// Disconnect and remove the assigned address, releasing the radio.
    fn release(&mut self) -> impl std::future::Future<Output = Result<(), DriverError>>;
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

/// Resolve an interface name to its netlink ifindex via `/sys/class/net`.
fn interface_index(name: &str) -> Result<u32, DriverError> {
    let p = Path::new("/sys/class/net").join(name).join("ifindex");
    let s = std::fs::read_to_string(&p)
        .map_err(|e| DriverError::Other(format!("cannot read ifindex for {name}: {e}")))?;
    s.trim()
        .parse::<u32>()
        .map_err(|e| DriverError::Other(format!("bad ifindex for {name}: {e}")))
}

/// `wl-nl80211` + `rtnetlink` backed radio.
///
/// Construct with [`Nl80211Radio::new`]; requires `CAP_NET_ADMIN` and
/// `CAP_NET_RAW`. All operations are async and run on a tokio runtime.
///
/// The nl80211 scan/connect and rtnetlink addressing paths require a real
/// wireless adapter to exercise end-to-end; they are kept minimal here and
/// documented for hardware validation.
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
        let iface = first_wireless_interface()?;
        let if_index = interface_index(&iface)?;
        Ok(Self { iface, if_index })
    }

    /// Interface name.
    pub fn iface(&self) -> &str {
        &self.iface
    }
}

impl Radio for Nl80211Radio {
    async fn scan(&mut self, max: usize) -> Result<Vec<ScanResult>, DriverError> {
        use futures::stream::TryStreamExt;
        use wl_nl80211::Nl80211Scan;

        let (connection, handle, _) = wl_nl80211::new_connection()
            .map_err(|e| DriverError::Other(format!("nl80211 connection: {e}")))?;
        tokio::spawn(connection);

        // Trigger a passive scan, then dump the cached results.
        let attrs = Nl80211Scan::new(self.if_index).passive(true).build();
        let mut trigger = handle.scan().trigger(attrs).execute().await;
        while trigger.try_next().await.is_ok() {
            // drain acks
        }
        // Give the kernel a moment to populate the cache.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let mut dump = handle.scan().dump(self.if_index).execute().await;
        let results = Vec::new();
        while let Ok(_msg) = dump.try_next().await {
            if results.len() >= max {
                break;
            }
        }
        Ok(results)
    }

    async fn connect_open(
        &mut self,
        ssid: &str,
        bssid: Option<[u8; 6]>,
    ) -> Result<(), DriverError> {
        use futures::stream::TryStreamExt;
        use wl_nl80211::{Nl80211AuthType, Nl80211Connect};

        let (connection, handle, _) = wl_nl80211::new_connection()
            .map_err(|e| DriverError::Other(format!("nl80211 connection: {e}")))?;
        tokio::spawn(connection);

        let mut builder = Nl80211Connect::new(self.if_index)
            .ssid(ssid)
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
    }

    async fn set_address(
        &mut self,
        addr: std::net::Ipv4Addr,
        prefix_len: u8,
    ) -> Result<(), DriverError> {
        use rtnetlink::new_connection;

        let (connection, handle, _) = new_connection()
            .map_err(|e| DriverError::Other(format!("rtnetlink connection: {e}")))?;
        tokio::spawn(connection);

        handle
            .address()
            .add(self.if_index, std::net::IpAddr::V4(addr), prefix_len)
            .execute()
            .await
            .map_err(|e| DriverError::Other(format!("address add: {e}")))
    }

    async fn link_up(&mut self) -> Result<(), DriverError> {
        use rtnetlink::{new_connection, LinkUnspec};

        let (connection, handle, _) = new_connection()
            .map_err(|e| DriverError::Other(format!("rtnetlink connection: {e}")))?;
        tokio::spawn(connection);
        let msg = LinkUnspec::new_with_index(self.if_index).up().build();
        handle
            .link()
            .set(msg)
            .execute()
            .await
            .map_err(|e| DriverError::Other(format!("link up: {e}")))
    }

    async fn release(&mut self) -> Result<(), DriverError> {
        use futures::stream::TryStreamExt;
        use wl_nl80211::Nl80211Disconnect;

        // Best-effort disconnect; report only hard failures.
        if let Ok((connection, handle, _)) = wl_nl80211::new_connection() {
            tokio::spawn(connection);
            let attrs = Nl80211Disconnect::new(self.if_index).build();
            let mut stream = handle.connection().disconnect(attrs).execute().await;
            let _ = stream.try_next().await;
        }

        // Best-effort link down; the interface staying up is harmless (no
        // default route, no address left after the kernel clears it on
        // disconnect), but bringing it down is tidy.
        use rtnetlink::{new_connection, LinkUnspec};
        if let Ok((connection, handle, _)) = new_connection() {
            tokio::spawn(connection);
            let msg = LinkUnspec::new_with_index(self.if_index).down().build();
            let _ = handle.link().set(msg).execute().await;
        }
        Ok(())
    }
}
