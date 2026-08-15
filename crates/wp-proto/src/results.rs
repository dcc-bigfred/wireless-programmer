//! Result bodies and job/scan wire types.

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// Successful response bodies.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResultBody {
    /// `hello` response.
    Hello(HelloResult),
    /// `scan` response.
    Scan(Vec<CandidateWire>),
    /// `probe` response.
    Probe(DeviceInfoWire),
    /// `program` response.
    Program(ProgramResult),
    /// `updateFirmware` response (queued job id).
    UpdateFirmware(ProgramResult),
    /// `job.get` response.
    Job(JobSnapshot),
    /// `job.watch` stream frame.
    JobWatch(JobFrame),
    /// `job.cancel` response.
    JobCancelled(JobSnapshot),
    /// `identify` response.
    Identify,
    /// `link.status` response.
    LinkStatus(LinkStatusWire),
}

/// `hello` result.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloResult {
    /// Daemon version.
    pub version: String,
    /// Git commit, when built with `WIRELESS_PROGRAMMER_GIT_COMMIT`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Registered drivers and their capabilities.
    pub drivers: Vec<DriverInfoWire>,
}

/// Driver advertisement.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverInfoWire {
    /// Driver identifier (e.g. `"wifred"`).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Capabilities.
    pub capabilities: CapabilitiesWire,
}

/// Driver capabilities, mirrored from `wp_core::DriverCapabilities`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitiesWire {
    /// Maximum roster slots the device can store.
    pub max_roster_slots: u8,
    /// Highest function index the device understands.
    pub max_function_index: u8,
    /// Required format of the identity string.
    pub identity_format: IdentityFormatWire,
    /// Whether the device accepts a wiThrottle server endpoint.
    pub supports_throttle_server: bool,
    /// How the device is commissioned.
    pub commissioning: CommissioningKindWire,
    /// Whether the driver can upload firmware over HTTP.
    #[serde(default)]
    pub supports_firmware_update: bool,
    /// Soft-AP addressing for commissioning, when not using daemon defaults.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub commissioning_net: Option<CommissioningNetWire>,
}

/// On-link Soft-AP addressing advertised by a driver.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommissioningNetWire {
    /// Device Soft-AP address (dotted IPv4).
    pub host: String,
    /// HTTP port on the Soft-AP.
    pub port: u16,
    /// Address the hub should assign on the wireless interface.
    pub source: String,
    /// Prefix length for the on-link route.
    pub prefix: u8,
}

/// Identity format constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IdentityFormatWire {
    /// Exactly `len` decimal digits.
    Digits {
        /// Required digit count.
        len: u8,
    },
    /// Alphanumeric, max `max_len` characters.
    Alphanumeric {
        /// Maximum length.
        max_len: u8,
    },
    /// Free-form, no constraint.
    Any,
}

/// How a device is commissioned.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommissioningKindWire {
    /// Device raises its own WiFi AP.
    SoftAp,
    /// Device is already on the LAN (mDNS).
    Lan,
    /// Device is reached over a serial link.
    Serial,
}

/// A scan candidate.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateWire {
    /// Owning driver.
    pub driver: String,
    /// Stable candidate key.
    pub key: String,
    /// Human-readable label (e.g. SSID).
    pub label: String,
    /// Signal strength in dBm, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rssi: Option<i32>,
}

/// Device info read back from a probe.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfoWire {
    /// Driver identifier.
    pub driver: String,
    /// Stable candidate key.
    pub key: String,
    /// Firmware revision, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware_revision: Option<String>,
    /// Device-reported identity (e.g. `throttleName`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// Battery voltage in millivolts, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_mv: Option<u32>,
    /// Currently stored roster, when readable.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub roster: Vec<crate::RosterEntryWire>,
}

/// `program` result: a job has been queued.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramResult {
    /// Job identifier.
    pub job_id: String,
}

/// A point-in-time snapshot of a job.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    /// Job identifier.
    pub job_id: String,
    /// Current state.
    pub state: JobStateWire,
    /// Driver owning the job.
    pub driver: String,
    /// Candidate key.
    pub key: String,
    /// Human-readable detail, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A single streamed progress frame.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobFrame {
    /// Job identifier.
    pub job_id: String,
    /// State at this frame.
    pub state: JobStateWire,
    /// Step label, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// Progress 0..=100, when meaningful.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    /// Detail, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Job lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobStateWire {
    /// Waiting for the radio lock.
    Queued,
    /// Associating to the device.
    Joining,
    /// Reading device info.
    Probing,
    /// Writing configuration.
    Writing,
    /// Reading back to verify.
    Verifying,
    /// Asking the device to restart.
    Restarting,
    /// Finished successfully.
    Done,
    /// Failed; see `detail`.
    Failed,
    /// Cancelled by the caller.
    Cancelled,
}

impl JobStateWire {
    /// Whether this state is terminal (`done`, `failed`, or `cancelled`).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    /// The wire name (matches the serde `camelCase` tag).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Joining => "joining",
            Self::Probing => "probing",
            Self::Writing => "writing",
            Self::Verifying => "verifying",
            Self::Restarting => "restarting",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for JobStateWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Radio/link status.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkStatusWire {
    /// Whether the radio is currently held by a job.
    pub busy: bool,
    /// Wireless interface name, when one is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    /// Whether rfkill blocks the radio.
    pub rfkill_blocked: bool,
}
