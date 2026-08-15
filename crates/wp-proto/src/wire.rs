//! Request/response envelope and typed wire bodies.

// ---------------------------------------------------------------------------
// Envelopes
// ---------------------------------------------------------------------------

/// Top-level request envelope. `type` selects the method; `params` carries
/// the arguments.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// Method selector.
    #[serde(rename = "type")]
    pub kind: RequestKind,
    /// Method parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Params>,
}

/// Top-level response envelope.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// Method selector mirrored from the request.
    #[serde(rename = "type")]
    pub kind: RequestKind,
    /// Result on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<crate::ResultBody>,
    /// Error on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

/// A typed error returned over the wire.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorBody {
    /// Machine-readable error code.
    pub code: String,
    /// Human-readable message.
    pub message: String,
}

impl ErrorBody {
    /// Construct an error body.
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Supported request methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RequestKind {
    /// `hello`: exchange version + driver capabilities.
    Hello,
    /// `scan`: enumerate candidate devices on the radio.
    Scan,
    /// `probe`: read a single candidate's device info.
    Probe,
    /// `program`: start a programming job, returns `job_id`.
    Program,
    /// `job.get`: snapshot a job's state.
    JobGet,
    /// `job.watch`: stream job progress frames until terminal.
    JobWatch,
    /// `job.cancel`: request cancellation of a running job.
    JobCancel,
    /// `identify`: blink the device LED so an operator can find it.
    Identify,
    /// `link.status`: report radio/link state.
    LinkStatus,
    /// `updateFirmware`: upload an app image over HTTP (Soft-AP or LAN).
    UpdateFirmware,
}

/// Method parameters.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Params {
    /// Arguments for [`RequestKind::Program`].
    Program(ProgramParams),
    /// Arguments for [`RequestKind::Probe`].
    Probe(ProbeParams),
    /// Arguments for [`RequestKind::JobGet`] / [`RequestKind::JobWatch`] /
    /// [`RequestKind::JobCancel`].
    Job(JobParams),
    /// Arguments for [`RequestKind::Identify`].
    Identify(IdentifyParams),
    /// Arguments for [`RequestKind::Scan`] (optional; omitted means Soft-AP).
    Scan(ScanParams),
    /// Arguments for [`RequestKind::UpdateFirmware`].
    UpdateFirmware(UpdateFirmwareParams),
    /// No parameters.
    None,
}

// ---------------------------------------------------------------------------
// Method parameters
// ---------------------------------------------------------------------------

/// `program` parameters.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramParams {
    /// Candidate to program, as returned by `scan`.
    pub candidate: CandidateRef,
    /// What to write.
    pub request: ProgramRequestWire,
}

/// `probe` parameters.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeParams {
    /// Candidate to probe.
    pub candidate: CandidateRef,
}

/// `job.*` parameters.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobParams {
    /// Job identifier returned by `program`.
    pub job_id: String,
}

/// `identify` parameters.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentifyParams {
    /// Candidate to blink.
    pub candidate: CandidateRef,
    /// Number of blinks (driver default applies when omitted).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

/// How to reach a LongFred for firmware or scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReachMode {
    /// Soft-AP programming network (radio scan / join).
    #[default]
    Ap,
    /// Device already on the layout LAN (mDNS / `--host`).
    Lan,
}

/// `scan` parameters.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanParams {
    /// Soft-AP radio scan (`ap`, default) or LAN mDNS (`lan`).
    #[serde(default)]
    pub mode: ReachMode,
}

/// `updateFirmware` parameters. The image stays on disk; the socket frame
/// only carries the path (1 MiB JSON limit).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFirmwareParams {
    /// Soft-AP (`ap`, default) or layout LAN (`lan`).
    #[serde(default)]
    pub mode: ReachMode,
    /// Candidate from `scan`. Optional when [`Self::host`] is set in LAN mode.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub candidate: Option<CandidateRef>,
    /// Path to an ESP32-C6 `.app.bin` on the hub.
    pub path: String,
    /// Explicit IPv4 for LAN mode (skips mDNS).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub host: Option<String>,
}

/// A reference to a scan result, stable for the lifetime of a scan session.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateRef {
    /// Driver identifier (e.g. `"wifred"`).
    pub driver: String,
    /// Opaque driver-specific candidate key (e.g. BSSID for WiFred).
    pub key: String,
}

/// The full programming request, in wire form.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramRequestWire {
    /// Opaque device identity (e.g. a 6-digit BigFred pairing code for WiFred).
    pub identity: String,
    /// WiFi network the device should join after programming.
    pub wifi: WifiCredentialsWire,
    /// wiThrottle server the device should connect to.
    pub server: ThrottleServerWire,
    /// DCC vehicle list (capped by the driver's `max_roster_slots`).
    pub roster: Vec<RosterEntryWire>,
    /// Optional BigFred login+PIN (LongFred and similar).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bigfred: Option<BigfredCredsWire>,
    /// Optional roster mode string (e.g. `"auto"` / `"static"` for LongFred).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub roster_mode: Option<String>,
}

/// BigFred login credentials on the wire.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BigfredCredsWire {
    /// BigFred login name.
    pub login: String,
    /// BigFred PIN (never logged by the daemon).
    pub pin: String,
}

/// WiFi credentials.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WifiCredentialsWire {
    /// SSID.
    pub ssid: String,
    /// PSK (never logged by the daemon).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub psk: Option<String>,
}

/// wiThrottle server endpoint.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThrottleServerWire {
    /// Hostname or IP.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Discover the server via mDNS instead of using a fixed host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automatic: Option<bool>,
}

/// One DCC vehicle slot.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RosterEntryWire {
    /// DCC address (1..=10239). `None` disables the slot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<u16>,
    /// True for a long address (>= 128), false for short.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_address: Option<bool>,
    /// Speed-step mode string (driver-specific vocabulary).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Direction: 0 forward, 1 reverse, 2 do-not-change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<u8>,
    /// Per-function mapping F0..Fmax (driver-specific enum value).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub functions: Vec<FunctionMappingWire>,
}

/// A function key mapping.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionMappingWire {
    /// Function index (0..=max_function_index).
    pub index: u8,
    /// Driver-specific mapping value.
    pub value: u8,
}
