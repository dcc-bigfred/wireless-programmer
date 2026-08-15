//! LongFred Soft-AP programming driver.
//!
//! Implements [`wp_core::DeviceDriver`] for LongFred throttles in programming
//! mode. The firmware raises an open Soft-AP named `longfred_prog_XXXXXX` with
//! a static address `192.168.0.1/24` and serves:
//!
//! - `GET  /api/v1/settings`
//! - `PUT  /api/v1/settings`
//! - `POST /api/v1/firmware`
//! - `POST /api/v1/programming-mode/off`
//!
//! Configuration is written as a single JSON PUT, verified with a GET, then
//! programming mode is cleared so the device leaves the Soft-AP.

mod constants;
mod discovery;
mod settings;

use wp_core::{
    validate_common, CommissioningNet, DeviceCandidate, DeviceDriver, DriverCapabilities,
    DriverError, DriverId, IdentityFormat, Observation, Outcome, ProgressSink, ScanFilters,
    Transport,
};

pub use constants::{
    CONFIG_AP_PORT, CONFIG_HOST, CONFIG_PREFIX_LEN, CONFIG_SOURCE, FIRMWARE_CONTENT_TYPE,
    FIRMWARE_PATH, MAX_FUNCTION, MAX_ROSTER_SLOTS, WIFI_CONFIG_SSID_PREFIX,
};
pub use discovery::identify;
pub use settings::{build_settings_put, format_roster_addr, verify};

use constants::{JSON_CONTENT_TYPE, PROGRAMMING_MODE_OFF_PATH, SETTINGS_PATH};

/// The LongFred driver.
#[derive(Debug, Default)]
pub struct LongFredDriver;

impl LongFredDriver {
    /// Construct a new driver instance.
    pub const fn new() -> Self {
        Self
    }
}

const ID: DriverId = DriverId::new("longfred");

impl DeviceDriver for LongFredDriver {
    fn id(&self) -> DriverId {
        ID
    }

    fn name(&self) -> &'static str {
        "LongFred"
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            max_roster_slots: MAX_ROSTER_SLOTS,
            max_function_index: MAX_FUNCTION,
            // Written as `wifi.hostname` (firmware max 16).
            identity_format: IdentityFormat::Alphanumeric { max_len: 16 },
            // LongFred authenticates to BigFred via login+PIN; the wiThrottle
            // server endpoint in ProgramRequest is unused but accepted so
            // callers can share a request shape with WiFred.
            supports_throttle_server: true,
            commissioning: wp_core::CommissioningKind::SoftAp,
            supports_firmware_update: true,
            commissioning_net: Some(CommissioningNet {
                host: CONFIG_HOST,
                port: CONFIG_AP_PORT,
                source: CONFIG_SOURCE,
                prefix: CONFIG_PREFIX_LEN,
            }),
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
            .get(SETTINGS_PATH)
            .map_err(|e| DriverError::Http(e.to_string()))?;
        serde_json::from_slice(&body).map_err(|e| DriverError::Parse(e.to_string()))
    }

    async fn program(
        &self,
        transport: Transport<'_>,
        req: &wp_core::ProgramRequest<'_>,
        progress: &mut dyn ProgressSink,
    ) -> Result<Outcome, DriverError> {
        let client = http_client(transport)?;

        progress.step("write");
        let put = build_settings_put(req);
        let put_bytes = serde_json::to_vec(&put).map_err(|e| DriverError::Other(e.to_string()))?;
        client
            .request("PUT", SETTINGS_PATH, Some((JSON_CONTENT_TYPE, &put_bytes)))
            .map_err(|e| DriverError::Http(e.to_string()))?;

        progress.step("verify");
        let body = client
            .get(SETTINGS_PATH)
            .map_err(|e| DriverError::Http(e.to_string()))?;
        let after: serde_json::Value =
            serde_json::from_slice(&body).map_err(|e| DriverError::Parse(e.to_string()))?;
        let mismatches = settings::verify(&after, req);
        if !mismatches.is_empty() {
            return Err(DriverError::VerificationFailed { mismatches });
        }

        progress.step("exit");
        client
            .request("POST", PROGRAMMING_MODE_OFF_PATH, None)
            .map_err(|e| DriverError::Http(e.to_string()))?;

        Ok(Outcome {
            restarted: true,
            mismatches: Vec::new(),
        })
    }
}

impl LongFredDriver {
    /// Stream an ESP32-C6 app image to `POST /api/v1/firmware`.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError`] when the HTTP POST fails.
    pub async fn update_firmware(
        &self,
        transport: Transport<'_>,
        image: &[u8],
        progress: &mut dyn ProgressSink,
    ) -> Result<Outcome, DriverError> {
        let client = http_client(transport)?;
        progress.step("write");
        progress.detail(&format!("{} bytes", image.len()));
        client
            .request("POST", FIRMWARE_PATH, Some((FIRMWARE_CONTENT_TYPE, image)))
            .map_err(|e| DriverError::Http(e.to_string()))?;
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
            "longfred driver requires an HTTP transport".into(),
        )),
    }
}

/// Soft-AP addressing helpers for callers that prefer constants over capabilities.
pub fn commissioning_net() -> CommissioningNet {
    CommissioningNet {
        host: CONFIG_HOST,
        port: CONFIG_AP_PORT,
        source: CONFIG_SOURCE,
        prefix: CONFIG_PREFIX_LEN,
    }
}
