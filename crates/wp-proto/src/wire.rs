//! Request/response envelope and typed wire bodies.

// ---------------------------------------------------------------------------
// Envelopes
// ---------------------------------------------------------------------------

/// Top-level request envelope. `type` selects the method; `params` carries
/// the arguments as a **flat** object (not an internally tagged enum), matching
/// `docs/api.md` and the Go client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Method selector.
    pub kind: RequestKind,
    /// Method parameters.
    pub params: Option<Params>,
}

/// Top-level response envelope.
///
/// `result` is the inner body (array, object, or omitted), not a tagged
/// `{ "scan": ... }` wrapper — Go unmarshals it into a concrete struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// Method selector mirrored from the request.
    pub kind: RequestKind,
    /// Result on success.
    pub result: Option<crate::ResultBody>,
    /// Error on failure.
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
    #[serde(rename = "job.get")]
    JobGet,
    /// `job.watch`: stream job progress frames until terminal.
    #[serde(rename = "job.watch")]
    JobWatch,
    /// `job.cancel`: request cancellation of a running job.
    #[serde(rename = "job.cancel")]
    JobCancel,
    /// `identify`: blink the device LED so an operator can find it.
    Identify,
    /// `link.status`: report radio/link state.
    #[serde(rename = "link.status")]
    LinkStatus,
    /// `updateFirmware`: upload an app image over HTTP (Soft-AP or LAN).
    UpdateFirmware,
}

/// Method parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// USB serial via `espflash` (`--port` / `scan --mode usb`).
    Usb,
}

/// `scan` parameters.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanParams {
    /// Soft-AP radio scan (`ap`, default), LAN mDNS (`lan`), or USB serial (`usb`).
    #[serde(default)]
    pub mode: ReachMode,
}

/// `updateFirmware` parameters. The image stays on disk; the socket frame
/// only carries the path (1 MiB JSON limit).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFirmwareParams {
    /// Soft-AP (`ap`, default), layout LAN (`lan`), or USB `espflash` (`usb`).
    #[serde(default)]
    pub mode: ReachMode,
    /// Candidate from `scan`. Optional when [`Self::host`] is set in LAN mode
    /// or [`Self::port`] in USB mode.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub candidate: Option<CandidateRef>,
    /// Path to a LongFred image on the hub (`.app.bin`, merged `.bin`, or ELF).
    pub path: String,
    /// Explicit IPv4 for LAN mode (skips mDNS).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub host: Option<String>,
    /// USB serial device (e.g. `/dev/ttyACM0`). USB mode; skips `scan --mode usb`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub port: Option<String>,
    /// CSV partition table for ELF USB flashes (`espflash flash --partition-table`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub partition_table: Option<String>,
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

// ---------------------------------------------------------------------------
// Flat JSON envelopes (docs/api.md + Go client)
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
struct EnvelopeDto {
    #[serde(rename = "type")]
    kind: RequestKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorBody>,
}

fn params_to_value(params: &Params) -> Result<Option<serde_json::Value>, serde_json::Error> {
    match params {
        Params::None => Ok(None),
        Params::Program(p) => serde_json::to_value(p).map(Some),
        Params::Probe(p) => serde_json::to_value(p).map(Some),
        Params::Job(p) => serde_json::to_value(p).map(Some),
        Params::Identify(p) => serde_json::to_value(p).map(Some),
        Params::Scan(p) => serde_json::to_value(p).map(Some),
        Params::UpdateFirmware(p) => serde_json::to_value(p).map(Some),
    }
}

fn params_from_value(
    kind: RequestKind,
    value: Option<serde_json::Value>,
) -> Result<Option<Params>, serde_json::Error> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    match kind {
        RequestKind::Hello | RequestKind::LinkStatus => Ok(Some(Params::None)),
        RequestKind::Scan => Ok(Some(Params::Scan(serde_json::from_value(value)?))),
        RequestKind::Probe => Ok(Some(Params::Probe(serde_json::from_value(value)?))),
        RequestKind::Program => Ok(Some(Params::Program(serde_json::from_value(value)?))),
        RequestKind::JobGet | RequestKind::JobWatch | RequestKind::JobCancel => {
            Ok(Some(Params::Job(serde_json::from_value(value)?)))
        }
        RequestKind::Identify => Ok(Some(Params::Identify(serde_json::from_value(value)?))),
        RequestKind::UpdateFirmware => {
            Ok(Some(Params::UpdateFirmware(serde_json::from_value(value)?)))
        }
    }
}

fn result_to_value(
    result: &crate::ResultBody,
) -> Result<Option<serde_json::Value>, serde_json::Error> {
    use crate::ResultBody;
    match result {
        ResultBody::Hello(v) => serde_json::to_value(v).map(Some),
        ResultBody::Scan(v) => serde_json::to_value(v).map(Some),
        ResultBody::Probe(v) => serde_json::to_value(v).map(Some),
        ResultBody::Program(v) | ResultBody::UpdateFirmware(v) => serde_json::to_value(v).map(Some),
        ResultBody::Job(v) | ResultBody::JobCancelled(v) => serde_json::to_value(v).map(Some),
        ResultBody::JobWatch(v) => serde_json::to_value(v).map(Some),
        ResultBody::Identify => Ok(None),
        ResultBody::LinkStatus(v) => serde_json::to_value(v).map(Some),
    }
}

fn result_from_value(
    kind: RequestKind,
    value: Option<serde_json::Value>,
) -> Result<Option<crate::ResultBody>, serde_json::Error> {
    use crate::ResultBody;
    let Some(value) = value else {
        return Ok(match kind {
            RequestKind::Identify => Some(ResultBody::Identify),
            _ => None,
        });
    };
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(match kind {
        RequestKind::Hello => ResultBody::Hello(serde_json::from_value(value)?),
        RequestKind::Scan => ResultBody::Scan(serde_json::from_value(value)?),
        RequestKind::Probe => ResultBody::Probe(serde_json::from_value(value)?),
        RequestKind::Program => ResultBody::Program(serde_json::from_value(value)?),
        RequestKind::UpdateFirmware => ResultBody::UpdateFirmware(serde_json::from_value(value)?),
        RequestKind::JobGet => ResultBody::Job(serde_json::from_value(value)?),
        RequestKind::JobWatch => ResultBody::JobWatch(serde_json::from_value(value)?),
        RequestKind::JobCancel => ResultBody::JobCancelled(serde_json::from_value(value)?),
        RequestKind::Identify => ResultBody::Identify,
        RequestKind::LinkStatus => ResultBody::LinkStatus(serde_json::from_value(value)?),
    }))
}

impl serde::Serialize for Request {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let params = match &self.params {
            None => None,
            Some(p) => params_to_value(p).map_err(serde::ser::Error::custom)?,
        };
        EnvelopeDto {
            kind: self.kind,
            params,
            result: None,
            error: None,
        }
        .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Request {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let dto = EnvelopeDto::deserialize(deserializer)?;
        Ok(Self {
            kind: dto.kind,
            params: params_from_value(dto.kind, dto.params).map_err(serde::de::Error::custom)?,
        })
    }
}

impl serde::Serialize for Response {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let result = match &self.result {
            None => None,
            Some(r) => result_to_value(r).map_err(serde::ser::Error::custom)?,
        };
        EnvelopeDto {
            kind: self.kind,
            params: None,
            result,
            error: self.error.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Response {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let dto = EnvelopeDto::deserialize(deserializer)?;
        Ok(Self {
            kind: dto.kind,
            result: result_from_value(dto.kind, dto.result).map_err(serde::de::Error::custom)?,
            error: dto.error,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HelloResult, ProgramResult, ResultBody};

    #[test]
    fn update_firmware_params_are_flat() {
        let req = Request {
            kind: RequestKind::UpdateFirmware,
            params: Some(Params::UpdateFirmware(UpdateFirmwareParams {
                mode: ReachMode::Ap,
                candidate: Some(CandidateRef {
                    driver: "longfred".into(),
                    key: "aa:bb".into(),
                }),
                path: "/tmp/x.app.bin".into(),
                host: None,
                port: None,
                partition_table: None,
            })),
        };
        let json = serde_json::to_value(&req).expect("ser");
        assert_eq!(json["type"], "updateFirmware");
        assert_eq!(json["params"]["mode"], "ap");
        assert_eq!(json["params"]["path"], "/tmp/x.app.bin");
        assert_eq!(json["params"]["candidate"]["driver"], "longfred");
        assert!(json["params"].get("updateFirmware").is_none());

        let back: Request = serde_json::from_value(json).expect("de");
        assert_eq!(back.kind, RequestKind::UpdateFirmware);
        match back.params {
            Some(Params::UpdateFirmware(p)) => assert_eq!(p.path, "/tmp/x.app.bin"),
            other => panic!("expected UpdateFirmware, got {other:?}"),
        }
    }

    #[test]
    fn go_dotted_method_names_round_trip() {
        let req: Request =
            serde_json::from_str(r#"{"type":"job.get","params":{"jobId":"job-1"}}"#).expect("de");
        assert_eq!(req.kind, RequestKind::JobGet);
        assert_eq!(serde_json::to_value(&req).unwrap()["type"], "job.get");

        let watch: Request =
            serde_json::from_str(r#"{"type":"job.watch","params":{"jobId":"job-1"}}"#).unwrap();
        assert_eq!(watch.kind, RequestKind::JobWatch);

        let link: Request = serde_json::from_str(r#"{"type":"link.status"}"#).unwrap();
        assert_eq!(link.kind, RequestKind::LinkStatus);
    }

    #[test]
    fn scan_result_is_a_bare_array() {
        let resp = Response {
            kind: RequestKind::Scan,
            result: Some(ResultBody::Scan(Vec::new())),
            error: None,
        };
        let json = serde_json::to_value(&resp).expect("ser");
        assert_eq!(json["type"], "scan");
        assert!(json["result"].is_array());
        assert!(json["result"].get("scan").is_none());
    }

    #[test]
    fn hello_result_is_a_bare_object() {
        let resp = Response {
            kind: RequestKind::Hello,
            result: Some(ResultBody::Hello(HelloResult {
                version: "0.1.0".into(),
                commit: None,
                drivers: Vec::new(),
            })),
            error: None,
        };
        let json = serde_json::to_value(&resp).expect("ser");
        assert_eq!(json["result"]["version"], "0.1.0");
        assert!(json["result"].get("hello").is_none());
    }

    #[test]
    fn program_result_job_id_is_at_result_root() {
        let resp = Response {
            kind: RequestKind::Program,
            result: Some(ResultBody::Program(ProgramResult {
                job_id: "job-1".into(),
            })),
            error: None,
        };
        let json = serde_json::to_value(&resp).expect("ser");
        assert_eq!(json["result"]["jobId"], "job-1");
    }

    #[test]
    fn go_flat_scan_lan_params() {
        let req: Request =
            serde_json::from_str(r#"{"type":"scan","params":{"mode":"lan"}}"#).unwrap();
        match req.params {
            Some(Params::Scan(p)) => assert_eq!(p.mode, ReachMode::Lan),
            other => panic!("{other:?}"),
        }
    }
}
