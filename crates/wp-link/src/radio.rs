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

    /// Kernel network device this radio drives, when it has one.
    ///
    /// Kernel name of the wireless interface, when this radio is real.
    ///
    /// The daemon passes it to the HTTP client for diagnostics and for
    /// `SO_BINDTODEVICE` when the Soft-AP address is **not** local. Colliding
    /// destinations use the policy route in [`crate::netcfg`] plus a source
    /// bind instead. Mock radios keep the `None` default.
    fn device(&self) -> Option<&str> {
        None
    }

    /// Associate to an open AP identified by SSID (and optional BSSID hint).
    fn connect_open(&mut self, ssid: &str, bssid: Option<[u8; 6]>) -> RadioFut<'_, ()>;

    /// Assign `addr/prefix_len` to the wireless interface (on-link route only).
    fn set_address(&mut self, addr: std::net::Ipv4Addr, prefix_len: u8) -> RadioFut<'_, ()>;

    /// Bring the link up.
    fn link_up(&mut self) -> RadioFut<'_, ()>;

    /// Disconnect and remove the assigned address, releasing the radio.
    fn release(&mut self) -> RadioFut<'_, ()>;

    /// Apply Soft-AP sysctls and policy routing for the device subnet.
    ///
    /// Called after [`set_address`] when the device address may collide
    /// with a local address on another interface. The radio restores
    /// everything in [`release`]. Default is a no-op (mock radios).
    fn prepare_softap(
        &mut self,
        _source: std::net::Ipv4Addr,
        _host: std::net::Ipv4Addr,
    ) -> RadioFut<'_, ()> {
        Box::pin(async { Ok(()) })
    }
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
    /// Sysctls changed for the current Soft-AP association, restored on
    /// [`Radio::release`]. See [`crate::netcfg`].
    saved_conf: crate::netcfg::SavedConf,
    /// Address assigned by [`Radio::set_address`], removed on
    /// [`Radio::release`]. A leftover address keeps an on-link route for the
    /// device subnet, which competes with the hub's own LAN when the two
    /// collide.
    assigned: Option<(std::net::Ipv4Addr, u8)>,
    /// Fib-rule exception installed by [`Radio::prepare_softap`], removed on
    /// [`Radio::release`].
    policy: Option<crate::netcfg::PolicyRoute>,
}

/// Hard cap on waiting for `NEW_SCAN_RESULTS` after TRIGGER_SCAN.
const SCAN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(8);
/// Timed wait when the nl80211 `scan` multicast group is unavailable.
const SCAN_FALLBACK_WAIT: std::time::Duration = std::time::Duration::from_secs(4);
/// How long to wait for carrier after a CONNECT is accepted.
const ASSOCIATE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
/// Carrier poll interval while waiting for association.
const ASSOCIATE_POLL: std::time::Duration = std::time::Duration::from_millis(100);
/// Pause after association so the device's HTTP server is ready. ESP32
/// lwIP needs longer than a typical Wi-Fi settle to start accepting TCP
/// connections on port 80.
const ASSOCIATE_SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

/// 2.4 GHz centre frequencies (channels 1–13). LongFred/WiFred Soft-APs are
/// 2.4 GHz only; a dual-band CYW43455 scan spends most of its dwell on 5 GHz.
const GHZ_24: &[u32] = &[
    2412, 2417, 2422, 2427, 2432, 2437, 2442, 2447, 2452, 2457, 2462, 2467, 2472,
];

/// `IFF_UP` in `/sys/class/net/<iface>/flags`.
const IFF_UP: u32 = 0x1;

/// Drain a netlink request stream and surface the first error.
async fn await_nl80211<S>(mut stream: S, what: &str) -> Result<(), DriverError>
where
    S: futures::stream::TryStream + Unpin,
    S::Error: std::fmt::Display,
{
    use futures::stream::TryStreamExt;
    loop {
        match stream.try_next().await {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(()),
            Err(e) => return Err(DriverError::Other(format!("{what}: {e}"))),
        }
    }
}

fn iface_is_up(name: &str) -> bool {
    let Ok(s) = std::fs::read_to_string(Path::new("/sys/class/net").join(name).join("flags"))
    else {
        return false;
    };
    u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).is_ok_and(|f| f & IFF_UP != 0)
}

fn iface_operstate(name: &str) -> String {
    std::fs::read_to_string(Path::new("/sys/class/net").join(name).join("operstate"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

/// `/sys/class/net/<name>/carrier`, which reads `1` once associated.
///
/// Unreadable (`None`) while the interface is administratively down.
fn iface_carrier(name: &str) -> Option<bool> {
    std::fs::read_to_string(Path::new("/sys/class/net").join(name).join("carrier"))
        .ok()
        .map(|s| s.trim() == "1")
}

/// `true` once the interface has carrier, i.e. association completed.
///
/// A wireless interface reports `operstate=down` and no carrier until it is
/// associated, so this is the check that tells a successful CONNECT apart
/// from one the firmware accepted and then abandoned.
fn iface_associated(name: &str) -> bool {
    iface_carrier(name) == Some(true) || iface_operstate(name) == "up"
}

/// Poll for carrier until `deadline` elapses. Returns `true` when associated.
async fn wait_associated(iface: &str, deadline: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if iface_associated(iface) {
            log::debug!(
                "connect: {iface} associated after {} ms",
                start.elapsed().as_millis()
            );
            return true;
        }
        if start.elapsed() >= deadline {
            return false;
        }
        tokio::time::sleep(ASSOCIATE_POLL).await;
    }
}

/// Issue one NL80211_CMD_CONNECT for an open (no PSK) AP.
///
/// Errors from the command are surfaced rather than drained: a rejected
/// CONNECT used to look like a successful association and only showed up
/// later as an HTTP failure.
async fn connect_once(
    handle: &wl_nl80211::Nl80211Handle,
    if_index: u32,
    ssid: &str,
    bssid: Option<[u8; 6]>,
) -> Result<(), DriverError> {
    use wl_nl80211::{Nl80211AuthType, Nl80211Connect};

    let mut builder = Nl80211Connect::new(if_index)
        .ssid(ssid)
        .auth_type(Nl80211AuthType::OpenSystem)
        .privacy(false);
    if let Some(mac) = bssid {
        builder = builder.mac(mac);
    }
    let stream = handle.connection().connect(builder.build()).execute().await;
    await_nl80211(stream, "nl80211 connect").await
}

fn log_rfkill() {
    match crate::rfkill::aggregate_state() {
        Ok(Some(s)) if s.blocked() => {
            log::warn!("scan: rfkill blocked (soft={} hard={})", s.soft, s.hard);
        }
        Ok(Some(_)) => log::debug!("scan: rfkill unblocked"),
        Ok(None) => log::debug!("scan: no rfkill switches"),
        Err(e) => log::debug!("scan: rfkill read: {e}"),
    }
}

async fn set_link_up(if_index: u32) -> Result<(), DriverError> {
    use rtnetlink::{new_connection, LinkUnspec};

    let (connection, handle, _) =
        new_connection().map_err(|e| DriverError::Other(format!("rtnetlink connection: {e}")))?;
    tokio::spawn(connection);
    let msg = LinkUnspec::new_with_index(if_index).up().build();
    handle
        .link()
        .set(msg)
        .execute()
        .await
        .map_err(|e| DriverError::Other(format!("link up: {e}")))
}

/// Remove `addr/prefix_len` from an interface, best-effort.
///
/// Looks the address up first because `RTM_DELADDR` needs the full message.
/// Failures are logged only: the caller is releasing the radio and must not
/// fail because of a cleanup step.
async fn remove_address(
    handle: &rtnetlink::Handle,
    if_index: u32,
    iface: &str,
    addr: std::net::Ipv4Addr,
    prefix_len: u8,
) {
    use futures::stream::TryStreamExt;

    let mut dump = handle
        .address()
        .get()
        .set_link_index_filter(if_index)
        .set_address_filter(std::net::IpAddr::V4(addr))
        .execute();
    loop {
        match dump.try_next().await {
            Ok(Some(msg)) => match handle.address().del(msg).execute().await {
                Ok(()) => log::debug!("released address {addr}/{prefix_len} from {iface}"),
                Err(e) => log::warn!("cannot remove {addr}/{prefix_len} from {iface}: {e}"),
            },
            Ok(None) => return,
            Err(e) => {
                log::warn!("cannot look up {addr} on {iface}: {e}");
                return;
            }
        }
    }
}

async fn trigger_scan(
    handle: &wl_nl80211::Nl80211Handle,
    if_index: u32,
    freqs: Option<&[u32]>,
) -> Result<(), DriverError> {
    let mut builder = wl_nl80211::Nl80211Scan::new(if_index);
    if let Some(freqs) = freqs {
        builder = builder.scan_frequencies(freqs.to_vec());
    }
    let attrs = builder.build();
    let stream = handle.scan().trigger(attrs).execute().await;
    await_nl80211(stream, "scan trigger").await
}

async fn dump_bss(
    handle: &wl_nl80211::Nl80211Handle,
    if_index: u32,
    max: usize,
) -> Result<Vec<ScanResult>, DriverError> {
    use futures::stream::TryStreamExt;

    let mut dump = handle.scan().dump(if_index).execute().await;
    let mut results = Vec::new();
    loop {
        match dump.try_next().await {
            Ok(Some(msg)) => {
                if results.len() >= max {
                    break;
                }
                if let Some(r) = parse_scan_attrs(&msg.payload.attributes) {
                    log::debug!(
                        "scan bss ssid={:?} bssid={:?} rssi={:?}",
                        r.ssid,
                        r.bssid,
                        r.rssi
                    );
                    results.push(r);
                }
            }
            Ok(None) => break,
            Err(e) => return Err(DriverError::Other(format!("scan dump: {e}"))),
        }
    }
    Ok(results)
}

async fn run_scan(iface: &str, if_index: u32, max: usize) -> Result<Vec<ScanResult>, DriverError> {
    use futures::stream::StreamExt;
    use wl_nl80211::{Nl80211Command, Nl80211Event, Nl80211MulticastGroup};

    log_rfkill();
    log::info!(
        "scan: iface={iface} ifindex={if_index} up={} operstate={}",
        iface_is_up(iface),
        iface_operstate(iface)
    );

    set_link_up(if_index).await?;
    log::info!(
        "scan: link up on {iface} (up={} operstate={})",
        iface_is_up(iface),
        iface_operstate(iface)
    );

    // Subscribe to scan events *before* TRIGGER_SCAN: the kernel can emit
    // NEW_SCAN_RESULTS before a late-joining socket sees it.
    let mut events = match wl_nl80211::new_multicast_connection(&[Nl80211MulticastGroup::Scan]) {
        Ok((conn, _, rx)) => {
            tokio::spawn(conn);
            Some(rx)
        }
        Err(e) => {
            log::warn!("scan: multicast subscribe failed ({e}); using timed wait");
            None
        }
    };

    let (connection, handle, _) = wl_nl80211::new_connection()
        .map_err(|e| DriverError::Other(format!("nl80211 connection: {e}")))?;
    tokio::spawn(connection);

    log::info!("scan: trigger active wildcard (2.4 GHz)");
    match trigger_scan(&handle, if_index, Some(GHZ_24)).await {
        Ok(()) => log::info!("scan: trigger ok (2.4 GHz)"),
        Err(e) => {
            log::warn!("scan: 2.4 GHz trigger failed ({e}); falling back to all bands");
            trigger_scan(&handle, if_index, None).await?;
            log::info!("scan: trigger ok (all bands)");
        }
    }

    if let Some(rx) = events.as_mut() {
        let wait = async {
            while let Some((msg, _)) = rx.next().await {
                match Nl80211Event::parse(msg) {
                    Some(Nl80211Event::NewScanResults) => {
                        log::debug!("scan: NEW_SCAN_RESULTS");
                        return Ok(());
                    }
                    Some(Nl80211Event::Unknown {
                        cmd: Nl80211Command::ScanAborted,
                    }) => {
                        log::warn!("scan: SCAN_ABORTED");
                        return Ok(());
                    }
                    Some(Nl80211Event::ScanStart) => {
                        log::debug!("scan: TRIGGER_SCAN event");
                    }
                    _ => {}
                }
            }
            Err(DriverError::Other(
                "nl80211 scan event stream closed before NEW_SCAN_RESULTS".into(),
            ))
        };
        match tokio::time::timeout(SCAN_DEADLINE, wait).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::warn!("scan: event wait: {e}; dumping anyway"),
            Err(_) => log::warn!("scan: timed out waiting for NEW_SCAN_RESULTS; dumping anyway"),
        }
    } else {
        tokio::time::sleep(SCAN_FALLBACK_WAIT).await;
    }

    let results = dump_bss(&handle, if_index, max).await?;
    log::info!("scan: {} BSS on {iface}", results.len());
    Ok(results)
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
        Ok(Self {
            iface,
            if_index,
            saved_conf: crate::netcfg::SavedConf::default(),
            assigned: None,
            policy: None,
        })
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
        let iface = self.iface.clone();
        Box::pin(async move { run_scan(&iface, if_index, max).await })
    }

    fn device(&self) -> Option<&str> {
        Some(&self.iface)
    }

    fn connect_open(&mut self, ssid: &str, bssid: Option<[u8; 6]>) -> RadioFut<'_, ()> {
        let if_index = self.if_index;
        let iface = self.iface.clone();
        let ssid = ssid.to_string();
        Box::pin(async move {
            // `release` puts the link down, so without this a second job
            // would try to associate on a down interface and silently fail.
            set_link_up(if_index).await?;
            log_rfkill();
            log::debug!(
                "connect: {ssid} on {iface} (up={} operstate={} carrier={:?})",
                iface_is_up(&iface),
                iface_operstate(&iface),
                iface_carrier(&iface)
            );

            let (connection, handle, _) = wl_nl80211::new_connection()
                .map_err(|e| DriverError::Other(format!("nl80211 connection: {e}")))?;
            tokio::spawn(connection);

            match connect_once(&handle, if_index, &ssid, bssid).await {
                Ok(()) => {
                    log::debug!("connect: CONNECT accepted for {ssid}");
                    if wait_associated(&iface, ASSOCIATE_DEADLINE).await {
                        tokio::time::sleep(ASSOCIATE_SETTLE).await;
                        return Ok(());
                    }
                    log::warn!("connect: {ssid} accepted but never gained carrier");
                }
                Err(e) => log::warn!("connect: CONNECT rejected for {ssid}: {e}"),
            }

            // The kernel drops its BSS cache when the link goes down, and
            // CONNECT needs the AP in that cache. Re-scan and try once more
            // so a stale cache does not fail the job.
            log::warn!(
                "connect: retrying {ssid} after a fresh scan (operstate={} carrier={:?})",
                iface_operstate(&iface),
                iface_carrier(&iface)
            );
            let found = run_scan(&iface, if_index, 64).await?;
            if !found
                .iter()
                .any(|r| r.ssid.as_deref() == Some(ssid.as_str()))
            {
                log::warn!("connect: {ssid} is not in the rescan results");
            }
            connect_once(&handle, if_index, &ssid, bssid).await?;
            if !wait_associated(&iface, ASSOCIATE_DEADLINE).await {
                log::warn!(
                    "connect: {ssid} still not associated (operstate={} carrier={:?})",
                    iface_operstate(&iface),
                    iface_carrier(&iface)
                );
                return Err(DriverError::AssociationTimedOut);
            }
            tokio::time::sleep(ASSOCIATE_SETTLE).await;
            Ok(())
        })
    }

    fn set_address(&mut self, addr: std::net::Ipv4Addr, prefix_len: u8) -> RadioFut<'_, ()> {
        let if_index = self.if_index;
        let iface = self.iface.clone();
        self.assigned = Some((addr, prefix_len));
        Box::pin(async move {
            use rtnetlink::new_connection;

            let (connection, handle, _) = new_connection()
                .map_err(|e| DriverError::Other(format!("rtnetlink connection: {e}")))?;
            tokio::spawn(connection);

            let result = handle
                .address()
                .add(if_index, std::net::IpAddr::V4(addr), prefix_len)
                .execute()
                .await
                .or_else(|e| {
                    let msg = e.to_string();
                    if msg.contains("exists")
                        || msg.contains("EEXIST")
                        || msg.contains("File exists")
                    {
                        log::debug!("address {addr}/{prefix_len} already on {iface}");
                        Ok(())
                    } else {
                        Err(DriverError::Other(format!("address add: {e}")))
                    }
                });
            if result.is_ok() {
                log::debug!(
                    "address {addr}/{prefix_len} on {iface} (up={} operstate={} accept_local={:?} rp_filter={:?})",
                    iface_is_up(&iface),
                    iface_operstate(&iface),
                    crate::netcfg::read_conf(&iface, "accept_local"),
                    crate::netcfg::read_conf(&iface, "rp_filter"),
                );
            }
            result
        })
    }

    fn link_up(&mut self) -> RadioFut<'_, ()> {
        let if_index = self.if_index;
        Box::pin(async move { set_link_up(if_index).await })
    }

    fn prepare_softap(
        &mut self,
        source: std::net::Ipv4Addr,
        host: std::net::Ipv4Addr,
    ) -> RadioFut<'_, ()> {
        let iface = self.iface.clone();
        self.saved_conf = crate::netcfg::prepare_softap(&iface);
        self.policy = Some(crate::netcfg::install_policy_route(source, host, &iface));
        Box::pin(async { Ok(()) })
    }

    fn release(&mut self) -> RadioFut<'_, ()> {
        let if_index = self.if_index;
        let iface = self.iface.clone();
        let assigned = self.assigned.take();
        let policy = self.policy.take();
        crate::netcfg::restore(&self.saved_conf);
        self.saved_conf = crate::netcfg::SavedConf::default();
        Box::pin(async move {
            if let Some(ref route) = policy {
                crate::netcfg::remove_policy_route(route);
            }
            use futures::stream::TryStreamExt;
            use wl_nl80211::Nl80211Disconnect;

            // Best-effort disconnect; report only hard failures.
            if let Ok((connection, handle, _)) = wl_nl80211::new_connection() {
                tokio::spawn(connection);
                let attrs = Nl80211Disconnect::new(if_index).build();
                let mut stream = handle.connection().disconnect(attrs).execute().await;
                let _ = stream.try_next().await;
            }

            use rtnetlink::{new_connection, LinkUnspec};
            if let Ok((connection, handle, _)) = new_connection() {
                tokio::spawn(connection);
                // The kernel does not always drop the address on disconnect,
                // and a leftover on-link route for the device subnet competes
                // with the hub's own LAN when the two collide.
                if let Some((addr, prefix_len)) = assigned {
                    remove_address(&handle, if_index, &iface, addr, prefix_len).await;
                }
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

    #[test]
    fn ghz24_is_channels_1_to_13() {
        assert_eq!(GHZ_24.len(), 13);
        assert_eq!(GHZ_24[0], 2412);
        assert_eq!(*GHZ_24.last().unwrap(), 2472);
        for pair in GHZ_24.windows(2) {
            assert_eq!(pair[1] - pair[0], 5);
        }
    }

    #[test]
    fn device_reports_bound_iface() {
        let Ok(first) = first_wireless_interface() else {
            return;
        };
        let radio = Nl80211Radio::with_interface(&first).expect("open");
        assert_eq!(radio.device(), Some(first.as_str()));
        assert_eq!(radio.iface(), first);
    }

    #[test]
    fn carrier_is_none_for_unknown_interface() {
        assert_eq!(iface_carrier("wp-no-such-iface"), None);
        assert!(!iface_associated("wp-no-such-iface"));
    }

    #[test]
    fn loopback_counts_as_associated() {
        if !Path::new("/sys/class/net/lo").exists() {
            return;
        }
        // `lo` is always operstate=unknown with carrier=1, so the carrier
        // branch is what has to match here.
        assert!(iface_associated("lo"));
    }

    #[tokio::test]
    async fn wait_associated_gives_up_on_a_missing_interface() {
        let start = std::time::Instant::now();
        let ok = wait_associated("wp-no-such-iface", std::time::Duration::from_millis(250)).await;
        assert!(!ok);
        assert!(start.elapsed() >= std::time::Duration::from_millis(250));
    }

    #[test]
    fn iface_is_up_loopback() {
        if !Path::new("/sys/class/net/lo").exists() {
            return;
        }
        assert!(iface_is_up("lo"));
        assert!(!iface_is_up("does-not-exist"));
    }
}
