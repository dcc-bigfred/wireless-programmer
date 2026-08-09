//! NewHeiko WiFred driver.
//!
//! Implements [`wp_core::DeviceDriver`] for the NewHeiko WiFred
//! (`https://github.com/newHeiko/wiFred`). In config mode the firmware calls
//! `initWiFiAP()`, raising an **open** AP named `wiFred-config<mac>` (the
//! MAC hex is not zero-padded, so we match on the prefix). The AP runs a web
//! server on port 80 with a built-in DHCP server at `192.168.4.1/24`.
//!
//! Configuration is read via `GET /api/getConfigXML` and written via a series
//! of `GET /index.html?...` requests whose query args are consumed by the
//! firmware's `server.arg()` handlers. The write order is load-bearing:
//! WiFi settings and restart are applied **last**, after everything else is
//! written and verified, so the device does not leave our AP mid-programming.

mod constants;
mod discovery;
mod xml;

use wp_core::{
    validate_common, DeviceCandidate, DeviceDriver, DriverCapabilities, DriverError, DriverId,
    IdentityFormat, NoProgress, Observation, Outcome, ProgressSink, ScanFilters, Transport,
};
use wp_link::percent_encode;

pub use constants::{
    Direction, FunctionInfo, CONFIG_AP_PORT, CONFIG_HOST, CONFIG_SOURCE_ADDR, MAX_FUNCTION,
    MAX_ROSTER_SLOTS, STRUCTURE_VERSION, WIFI_CONFIG_SSID_PREFIX,
};
pub use xml::{DeviceConfig, LocoConfig};

/// The WiFred driver.
#[derive(Debug, Default)]
pub struct WiFredDriver;

impl WiFredDriver {
    /// Construct a new driver instance.
    pub const fn new() -> Self {
        Self
    }
}

const ID: DriverId = DriverId::new("wifred");

impl DeviceDriver for WiFredDriver {
    fn id(&self) -> DriverId {
        ID
    }

    fn name(&self) -> &'static str {
        "NewHeiko WiFred"
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            max_roster_slots: MAX_ROSTER_SLOTS,
            max_function_index: MAX_FUNCTION,
            identity_format: IdentityFormat::Digits { len: 6 },
            supports_throttle_server: true,
            commissioning: wp_core::CommissioningKind::SoftAp,
        }
    }

    fn scan_filters(&self) -> ScanFilters {
        ScanFilters {
            ssid_prefixes: vec![WIFI_CONFIG_SSID_PREFIX.into()],
        }
    }

    fn identify(&self, obs: &Observation) -> Option<DeviceCandidate> {
        discovery::identify(obs)
    }

    fn validate(&self, req: &wp_core::ProgramRequest<'_>) -> Result<(), wp_core::ValidationError> {
        validate_common(&self.capabilities(), req)
    }

    async fn probe(&self, transport: Transport<'_>) -> Result<serde_json::Value, DriverError> {
        let client = http_client(transport)?;
        let body = client
            .get("/api/getConfigXML")
            .map_err(|e| DriverError::Http(e.to_string()))?;
        let cfg = xml::parse(&body).map_err(DriverError::Parse)?;
        check_structure_version(&cfg)?;
        serde_json::to_value(cfg).map_err(|e| DriverError::Parse(e.to_string()))
    }

    async fn program(
        &self,
        transport: Transport<'_>,
        req: &wp_core::ProgramRequest<'_>,
        progress: &mut dyn ProgressSink,
    ) -> Result<Outcome, DriverError> {
        let client = http_client(transport)?;

        // 1. Snapshot current state.
        progress.step("read");
        let body = client
            .get("/api/getConfigXML")
            .map_err(|e| DriverError::Http(e.to_string()))?;
        let before = xml::parse(&body).map_err(DriverError::Parse)?;
        check_structure_version(&before)?;

        // 2. Identity (throttleName).
        progress.step("identity");
        get(
            client,
            &format!("/index.html?throttleName={}", percent_encode(req.identity)),
        )?;

        // 3. Loco slots (address / mode / direction / long flag).
        progress.step("locos");
        for (slot, entry) in req.roster.iter().enumerate() {
            let n = (slot + 1) as u8;
            // -1 disables an unused slot.
            let addr: i16 = entry
                .address
                .and_then(|a| i16::try_from(a).ok())
                .unwrap_or(-1);
            let mut q = format!(
                "/index.html?loco={n}&loco.address={addr}&loco.mode={}",
                percent_encode(entry.mode.unwrap_or(""))
            );
            let dir = entry.direction.unwrap_or(Direction::DontChange as u8);
            q.push_str(&format!("&loco.direction={dir}"));
            if entry.long_address == Some(true) {
                q.push_str("&loco.longAddress=on");
            }
            get(client, &q)?;
        }

        // 4. Function maps per slot.
        progress.step("functions");
        for (slot, entry) in req.roster.iter().enumerate() {
            if entry.functions.is_empty() {
                continue;
            }
            let n = (slot + 1) as u8;
            let mut q = format!("/index.html?loco={n}");
            for fm in &entry.functions {
                q.push_str(&format!("&f{}={}", fm.index, fm.value));
            }
            get(client, &q)?;
        }

        // 5. wiThrottle server.
        progress.step("server");
        let mut q = format!(
            "/index.html?loco.serverName={}&loco.serverPort={}",
            percent_encode(req.server.host),
            req.server.port
        );
        if req.server.automatic {
            q.push_str("&loco.automatic=on");
        }
        get(client, &q)?;

        // 6. WiFi: remove stale entries for the target SSID, then add.
        //    Done last so the device does not drop our AP mid-write.
        progress.step("wifi");
        get(
            client,
            &format!("/index.html?remove={}", percent_encode(req.wifi.ssid)),
        )?;
        let mut q = format!("/index.html?wifiSSID={}", percent_encode(req.wifi.ssid));
        if let Some(psk) = req.wifi.psk {
            if !psk.is_empty() {
                q.push_str(&format!("&wifiKEY={}", percent_encode(psk)));
            }
        }
        get(client, &q)?;

        // 7. Verify.
        progress.step("verify");
        let body = client
            .get("/api/getConfigXML")
            .map_err(|e| DriverError::Http(e.to_string()))?;
        let after = xml::parse(&body).map_err(DriverError::Parse)?;
        let mismatches = verify(&after, req);
        if !mismatches.is_empty() {
            return Err(DriverError::VerificationFailed { mismatches });
        }

        // 8. Restart so WiFi settings take effect.
        progress.step("restart");
        get(client, "/restart.html")?;

        Ok(Outcome {
            restarted: true,
            mismatches: Vec::new(),
        })
    }
}

/// Blink the device LED so an operator can find the physical throttle.
///
/// Maps to `GET /flashred.html?count=N`.
pub fn identify_request(count: Option<u32>) -> String {
    match count {
        Some(n) => format!("/flashred.html?count={n}"),
        None => "/flashred.html".into(),
    }
}

/// Extract the HTTP client from a [`Transport`].
fn http_client(transport: Transport<'_>) -> Result<&mut dyn wp_core::HttpClient, DriverError> {
    match transport {
        Transport::Http(c) => Ok(c),
        Transport::Bytes(_) => Err(DriverError::Other(
            "wifred driver requires an HTTP transport".into(),
        )),
    }
}

/// Issue a GET and translate I/O errors.
fn get(client: &mut dyn wp_core::HttpClient, path: &str) -> Result<(), DriverError> {
    client
        .get(path)
        .map_err(|e| DriverError::Http(e.to_string()))?;
    Ok(())
}

/// Reject an unsupported structure version.
fn check_structure_version(cfg: &DeviceConfig) -> Result<(), DriverError> {
    match &cfg.structure_version {
        Some(v) if v == STRUCTURE_VERSION => Ok(()),
        Some(v) => Err(DriverError::UnsupportedStructureVersion(v.clone())),
        None => Err(DriverError::Parse(
            "missing structureVersion in device response".into(),
        )),
    }
}

/// Compare the read-back config against the request, returning mismatch field names.
fn verify(cfg: &DeviceConfig, req: &wp_core::ProgramRequest<'_>) -> Vec<String> {
    let mut mismatches = Vec::new();
    if cfg.throttle_name.as_deref() != Some(req.identity) {
        mismatches.push("throttleName".into());
    }
    if cfg.loco_server.as_ref().map(|s| s.name.as_str()) != Some(req.server.host)
        || cfg.loco_server.as_ref().map(|s| s.port).unwrap_or(0) != req.server.port
    {
        mismatches.push("locoServer".into());
    }
    let has_ssid = cfg
        .networks
        .iter()
        .any(|n| n.ssid == req.wifi.ssid && n.enabled);
    if !has_ssid {
        mismatches.push("wifi".into());
    }
    for (slot, entry) in req.roster.iter().enumerate() {
        let want_addr = entry.address.map(i64::from).unwrap_or(-1);
        let got = cfg.locos.get(slot);
        match got {
            Some(g) if g.address == want_addr => {}
            _ => mismatches.push(format!("loco[{}].address", slot + 1)),
        }
    }
    mismatches
}

// Silence the default-progress import warning when only the trait is used.
#[allow(dead_code)]
fn _no_progress() -> NoProgress {
    NoProgress
}
