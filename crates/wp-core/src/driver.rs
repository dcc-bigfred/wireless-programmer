//! The `DeviceDriver` trait and supporting types.

use std::collections::BTreeSet;
use std::fmt;

use crate::{DriverCapabilities, IdentityFormat, ProgramRequest, ValidationError};

/// A borrowed transport handed to a driver after the daemon established
/// reachability. A driver never opens its own sockets (guidelines §1.2).
pub enum Transport<'a> {
    /// An HTTP client (e.g. for a WiFi config page).
    Http(&'a mut dyn crate::HttpClient),
    /// A byte stream (e.g. for a serial device).
    Bytes(&'a mut dyn crate::ByteStream),
}

impl fmt::Debug for Transport<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Transport::Http(_) => f.debug_tuple("Http").field(&"<http>").finish(),
            Transport::Bytes(_) => f.debug_tuple("Bytes").field(&"<bytes>").finish(),
        }
    }
}

/// A raw radio observation (one scan result) before a driver claims it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// SSID, when present.
    pub ssid: Option<String>,
    /// BSSID, when present.
    pub bssid: Option<String>,
    /// Signal strength in dBm, when known.
    pub rssi: Option<i32>,
    /// Driver-extended fields, serialised as JSON.
    pub extra: serde_json::Value,
}

/// A driver-claimed candidate, stable for the scan session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCandidate {
    /// Owning driver id.
    pub driver: String,
    /// Stable candidate key (e.g. BSSID).
    pub key: String,
    /// Human-readable label.
    pub label: String,
    /// Signal strength in dBm, when known.
    pub rssi: Option<i32>,
}

/// Filters a driver applies to raw scan observations.
#[derive(Debug, Clone, Default)]
pub struct ScanFilters {
    /// SSID prefixes the driver claims.
    pub ssid_prefixes: Vec<String>,
}

/// Sink for progress updates during a programming job.
pub trait ProgressSink: Send {
    /// Report a step transition.
    fn step(&mut self, step: &str);
    /// Report progress 0..=100, when meaningful.
    fn progress(&mut self, progress: u8);
    /// Report a human-readable detail line.
    fn detail(&mut self, detail: &str);
}

/// A no-op sink for drivers that do not report progress.
#[derive(Debug, Default)]
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn step(&mut self, _step: &str) {}
    fn progress(&mut self, _progress: u8) {}
    fn detail(&mut self, _detail: &str) {}
}

/// The outcome of a successful programming run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Whether the device was asked to restart.
    pub restarted: bool,
    /// Fields that mismatched during verification, if any.
    pub mismatches: Vec<String>,
}

/// The contract every device family implements.
pub trait DeviceDriver {
    /// Stable driver identifier.
    fn id(&self) -> crate::DriverId;

    /// Human-readable name.
    fn name(&self) -> &'static str;

    /// Capabilities advertised via `hello`.
    fn capabilities(&self) -> DriverCapabilities;

    /// Filters applied to raw scan observations.
    fn scan_filters(&self) -> ScanFilters;

    /// Claim a raw observation as a candidate, if it belongs to this driver.
    fn identify(&self, obs: &Observation) -> Option<DeviceCandidate>;

    /// Validate a request against capabilities **before** any radio work.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError`] for caller-controlled input that does not
    /// satisfy the device's constraints.
    fn validate(&self, req: &ProgramRequest<'_>) -> Result<(), ValidationError>;

    /// Read device info over the supplied transport.
    ///
    /// # Errors
    ///
    /// Returns [`crate::DriverError`] on runtime failure.
    fn probe(
        &self,
        transport: Transport<'_>,
    ) -> impl std::future::Future<Output = Result<serde_json::Value, crate::DriverError>>;

    /// Program the device over the supplied transport.
    ///
    /// # Errors
    ///
    /// Returns [`crate::DriverError`] on runtime failure.
    fn program(
        &self,
        transport: Transport<'_>,
        req: &ProgramRequest<'_>,
        progress: &mut dyn ProgressSink,
    ) -> impl std::future::Future<Output = Result<Outcome, crate::DriverError>>;
}

/// Shared validation used by drivers: capacity, identity format, address
/// range, function index range, SSID non-empty, server support.
pub fn validate_common(
    caps: &DriverCapabilities,
    req: &ProgramRequest<'_>,
) -> Result<(), ValidationError> {
    if !caps.identity_format.matches(req.identity) {
        return Err(ValidationError::IdentityFormat);
    }
    if req.wifi.ssid.is_empty() {
        return Err(ValidationError::EmptySsid);
    }
    if !caps.supports_throttle_server {
        return Err(ValidationError::ServerUnsupported);
    }
    if req.roster.len() > usize::from(caps.max_roster_slots) {
        return Err(ValidationError::CapacityExceeded {
            capacity: usize::from(caps.max_roster_slots),
            requested: req.roster.len(),
        });
    }
    let mut seen = BTreeSet::new();
    for (slot, entry) in req.roster.iter().enumerate() {
        if let Some(addr) = entry.address {
            if !(1..=10239).contains(&addr) {
                return Err(ValidationError::AddressOutOfRange { addr });
            }
        }
        for fm in &entry.functions {
            if fm.index > caps.max_function_index {
                return Err(ValidationError::FunctionIndexOutOfRange {
                    index: fm.index,
                    max: caps.max_function_index,
                });
            }
            if !seen.insert((slot, fm.index)) {
                return Err(ValidationError::RosterEntry {
                    slot,
                    reason: "duplicate function index",
                });
            }
        }
    }
    Ok(())
}

// Keep the IdentityFormat import used by the public re-export path.
const _: fn(IdentityFormat) = |f| {
    let _ = f.matches("");
};
