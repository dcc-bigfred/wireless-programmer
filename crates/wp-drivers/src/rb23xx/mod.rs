//! RailBOX RB23xx Soft-AP firmware driver.
//!
//! Implements [`wp_core::DeviceDriver`] for RailBOX RB2300 / RB2310 sound
//! decoders. With F28 on, the decoder raises a WPA2-PSK Soft-AP named
//! `RB2300_XXXXX` (password [`constants::SOFTAP_PSK`]) at `192.168.4.1/24`
//! and serves a file-browser:
//!
//! - `GET  /?p=/`
//! - `POST /upload?p=/{filename}` (`Content-Type: multipart/form-data`, raw body)
//!
//! Firmware `.bin` is uploaded to the decoder root (not a sound-pack slot).
//! The decoder reboots when the image is stored, so a TCP RST after a full
//! write is success. Roster programming is not supported over this HTTP API.

mod constants;
mod discovery;

use wp_core::{
    CommissioningNet, DeviceCandidate, DeviceDriver, DriverCapabilities, DriverError, DriverId,
    FirmwareCapabilities, FirmwareModes, IdentityFormat, Observation, Outcome, ProgressSink,
    ScanFilters, Transport,
};
use wp_link::percent_encode;

pub use constants::{
    CONFIG_AP_PORT, CONFIG_HOST, CONFIG_PREFIX_LEN, CONFIG_SOURCE, MAX_FIRMWARE_BYTES,
    MAX_FUNCTION, MAX_ROSTER_SLOTS, SOFTAP_PSK, UPLOAD_CONTENT_TYPE, WIFI_CONFIG_SSID_PREFIXES,
};
pub use discovery::identify;

use constants::LIST_PATH;

/// The RB23xx driver.
#[derive(Debug, Default)]
pub struct Rb23xxDriver;

impl Rb23xxDriver {
    /// Construct a new driver instance.
    pub const fn new() -> Self {
        Self
    }
}

const ID: DriverId = DriverId::new("rb23xx");

impl DeviceDriver for Rb23xxDriver {
    fn id(&self) -> DriverId {
        ID
    }

    fn name(&self) -> &'static str {
        "RailBOX RB23xx"
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            max_roster_slots: MAX_ROSTER_SLOTS,
            max_function_index: MAX_FUNCTION,
            identity_format: IdentityFormat::Any,
            supports_throttle_server: false,
            commissioning: wp_core::CommissioningKind::SoftAp,
            commissioning_net: Some(CommissioningNet {
                host: CONFIG_HOST,
                port: CONFIG_AP_PORT,
                source: CONFIG_SOURCE,
                prefix: CONFIG_PREFIX_LEN,
            }),
            softap_psk: Some(SOFTAP_PSK),
            firmware: Some(FirmwareCapabilities {
                max_bytes: MAX_FIRMWARE_BYTES,
                max_bytes_label: "5 MiB",
                modes: FirmwareModes::AP,
                require_esp_app_bin: false,
                success_on_reset_after_write: true,
            }),
        }
    }

    fn scan_filters(&self) -> ScanFilters {
        ScanFilters {
            ssid_prefixes: WIFI_CONFIG_SSID_PREFIXES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }

    fn identify(&self, obs: &Observation) -> Option<DeviceCandidate> {
        discovery::identify(obs)
    }

    fn validate(&self, _req: &wp_core::ProgramRequest<'_>) -> Result<(), wp_core::ValidationError> {
        Ok(())
    }

    async fn probe(&self, transport: Transport<'_>) -> Result<serde_json::Value, DriverError> {
        let client = http_client(transport)?;
        let body = client
            .get(LIST_PATH)
            .map_err(|e| DriverError::Http(e.to_string()))?;
        Ok(serde_json::json!({
            "ok": true,
            "bytes": body.len(),
        }))
    }

    async fn program(
        &self,
        _transport: Transport<'_>,
        _req: &wp_core::ProgramRequest<'_>,
        _progress: &mut dyn ProgressSink,
    ) -> Result<Outcome, DriverError> {
        Err(DriverError::Other(
            "rb23xx does not support program; use updateFirmware to upload a .bin".into(),
        ))
    }

    async fn update_firmware(
        &self,
        transport: Transport<'_>,
        image: &[u8],
        filename: &str,
        progress: &mut dyn ProgressSink,
    ) -> Result<Outcome, DriverError> {
        let client = http_client(transport)?;
        let name = std::path::Path::new(filename)
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .unwrap_or("firmware.bin");
        let path = format!("/upload?p=/{}", percent_encode(name));
        progress.step("write");
        progress.detail(&format!("{} bytes", image.len()));
        client
            .request("POST", &path, Some((UPLOAD_CONTENT_TYPE, image)))
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::Interrupted {
                    DriverError::Cancelled
                } else {
                    DriverError::Http(e.to_string())
                }
            })?;
        progress.step("restart");
        Ok(Outcome {
            restarted: true,
            mismatches: Vec::new(),
        })
    }
}

/// Extract the HTTP client from a [`Transport`].
fn http_client(transport: Transport<'_>) -> Result<&mut dyn wp_core::HttpClient, DriverError> {
    match transport {
        Transport::Http(c) => Ok(c),
        Transport::Bytes(_) => Err(DriverError::Other(
            "rb23xx driver requires an HTTP transport".into(),
        )),
    }
}

/// Soft-AP addressing helpers for callers that prefer constants over capabilities.
#[must_use]
pub fn commissioning_net() -> CommissioningNet {
    CommissioningNet {
        host: CONFIG_HOST,
        port: CONFIG_AP_PORT,
        source: CONFIG_SOURCE,
        prefix: CONFIG_PREFIX_LEN,
    }
}
