//! Digitrax FRED (wired LocoNet throttle) via Z21 `LAN_LOCONET_DISPATCH_ADDR`.
//!
//! The FRED itself has no Wi‑Fi page. Programming is DISPATCH_PUT of one DCC
//! address on a Z21-LAN command station that is LocoNet master. The daemon
//! owns the UDP socket; this driver only validates the roster.

use wp_core::{
    DeviceCandidate, DeviceDriver, DriverCapabilities, DriverError, DriverId, IdentityFormat,
    Observation, Outcome, ProgressSink, ScanFilters, Transport, ValidationError,
};

/// Digitrax FRED driver.
#[derive(Debug, Default)]
pub struct FredDriver;

impl FredDriver {
    /// Construct a new driver instance.
    pub const fn new() -> Self {
        Self
    }
}

const ID: DriverId = DriverId::new("fred");

impl DeviceDriver for FredDriver {
    fn id(&self) -> DriverId {
        ID
    }

    fn name(&self) -> &'static str {
        "Digitrax FRED"
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            max_roster_slots: 1,
            max_function_index: 0,
            identity_format: IdentityFormat::Any,
            supports_throttle_server: false,
            commissioning: wp_core::CommissioningKind::Lan,
            supports_firmware_update: false,
            commissioning_net: None,
        }
    }

    fn scan_filters(&self) -> ScanFilters {
        ScanFilters::default()
    }

    fn identify(&self, _obs: &Observation) -> Option<DeviceCandidate> {
        None
    }

    fn validate(&self, req: &wp_core::ProgramRequest<'_>) -> Result<(), ValidationError> {
        if req.roster.len() != 1 {
            return Err(ValidationError::CapacityExceeded {
                capacity: 1,
                requested: req.roster.len(),
            });
        }
        match req.roster[0].address {
            Some(addr) if (1..=10239).contains(&addr) => Ok(()),
            Some(addr) => Err(ValidationError::AddressOutOfRange { addr }),
            None => Err(ValidationError::RosterEntry {
                slot: 0,
                reason: "missing address",
            }),
        }
    }

    async fn probe(&self, _transport: Transport<'_>) -> Result<serde_json::Value, DriverError> {
        Ok(serde_json::json!({ "driver": "fred" }))
    }

    async fn program(
        &self,
        _transport: Transport<'_>,
        _req: &wp_core::ProgramRequest<'_>,
        _progress: &mut dyn ProgressSink,
    ) -> Result<Outcome, DriverError> {
        Err(DriverError::Other(
            "fred programming uses Z21 UDP, not HTTP/serial".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wp_core::{ProgramRequest, RosterEntry, ThrottleServer, WifiCredentials};

    fn req(addr: Option<u16>) -> ProgramRequest<'static> {
        ProgramRequest {
            identity: "",
            wifi: WifiCredentials {
                ssid: "",
                psk: None,
            },
            server: ThrottleServer {
                host: "",
                port: 0,
                automatic: false,
            },
            roster: vec![RosterEntry {
                address: addr,
                long_address: None,
                mode: None,
                direction: None,
                functions: Vec::new(),
            }],
            bigfred: None,
            roster_mode: None,
        }
    }

    #[test]
    fn validate_requires_one_in_range_address() {
        let d = FredDriver::new();
        assert!(d.validate(&req(Some(42))).is_ok());
        assert!(matches!(
            d.validate(&req(Some(0))),
            Err(ValidationError::AddressOutOfRange { addr: 0 })
        ));
        assert!(matches!(
            d.validate(&req(None)),
            Err(ValidationError::RosterEntry { slot: 0, .. })
        ));
        let empty = ProgramRequest {
            roster: Vec::new(),
            ..req(Some(1))
        };
        assert!(matches!(
            d.validate(&empty),
            Err(ValidationError::CapacityExceeded {
                capacity: 1,
                requested: 0
            })
        ));
    }
}
