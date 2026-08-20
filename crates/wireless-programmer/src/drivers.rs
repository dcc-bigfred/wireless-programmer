//! Driver registry and enum dispatch.
//!
//! The driver set is closed at compile time, so dispatch uses an enum
//! (guidelines §8.2) rather than `Box<dyn DeviceDriver>`.

use std::net::Ipv4Addr;

use wp_core::{
    CommissioningNet, DeviceCandidate, DeviceDriver, DriverCapabilities, DriverError, Observation,
    Outcome, ProgramRequest, ProgressSink, Transport,
};
use wp_drivers::{FredDriver, LongFredDriver, WiFredDriver};

/// All registered drivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    /// NewHeiko WiFred.
    WiFred,
    /// LongFred Soft-AP programming.
    LongFred,
    /// Digitrax FRED via Z21 LAN LocoNet dispatch.
    Fred,
}

impl Driver {
    /// The driver's stable id string.
    pub fn id_str(self) -> &'static str {
        match self {
            Driver::WiFred => "wifred",
            Driver::LongFred => "longfred",
            Driver::Fred => "fred",
        }
    }

    /// Human-readable name.
    pub fn name(self) -> &'static str {
        match self {
            Driver::WiFred => "NewHeiko WiFred",
            Driver::LongFred => "LongFred",
            Driver::Fred => "Digitrax FRED",
        }
    }

    /// Soft-AP addressing for commissioning.
    pub fn commissioning_net(self) -> CommissioningNet {
        match self {
            Driver::WiFred => CommissioningNet {
                host: Ipv4Addr::new(192, 168, 4, 1),
                port: 80,
                source: Ipv4Addr::new(192, 168, 4, 2),
                prefix: 24,
            },
            Driver::LongFred => wp_drivers::longfred::commissioning_net(),
            Driver::Fred => CommissioningNet {
                host: Ipv4Addr::UNSPECIFIED,
                port: 0,
                source: Ipv4Addr::UNSPECIFIED,
                prefix: 0,
            },
        }
    }

    /// Parse a driver id string.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "wifred" => Some(Driver::WiFred),
            "longfred" => Some(Driver::LongFred),
            "fred" => Some(Driver::Fred),
            _ => None,
        }
    }
}

/// A registry of all drivers, owning their instances.
#[derive(Debug)]
pub struct DriverRegistry {
    wifred: WiFredDriver,
    longfred: LongFredDriver,
    fred: FredDriver,
}

impl DriverRegistry {
    /// Construct a registry with all built-in drivers.
    pub fn new() -> Self {
        Self {
            wifred: WiFredDriver::new(),
            longfred: LongFredDriver::new(),
            fred: FredDriver::new(),
        }
    }

    /// Iterate over (driver tag, capabilities) for `hello`.
    pub fn drivers(&self) -> Vec<(Driver, DriverCapabilities)> {
        vec![
            (Driver::WiFred, self.wifred.capabilities()),
            (Driver::LongFred, self.longfred.capabilities()),
            (Driver::Fred, self.fred.capabilities()),
        ]
    }

    /// Build the `hello` result's driver list.
    pub fn driver_infos(&self) -> Vec<wp_proto::DriverInfoWire> {
        self.drivers()
            .into_iter()
            .map(|(d, caps)| wp_proto::DriverInfoWire {
                id: d.id_str().into(),
                name: d.name().into(),
                capabilities: caps.into(),
            })
            .collect()
    }

    /// Find the driver owning a candidate.
    pub fn driver_for(&self, candidate: &wp_proto::CandidateRef) -> Option<Driver> {
        Driver::from_id(candidate.driver.as_str())
    }

    /// Claim a raw observation against every driver.
    pub fn identify(&self, obs: &Observation) -> Option<DeviceCandidate> {
        self.longfred
            .identify(obs)
            .or_else(|| self.wifred.identify(obs))
    }

    /// Validate a request against the driver's capabilities.
    pub fn validate(
        &self,
        driver: Driver,
        req: &ProgramRequest<'_>,
    ) -> Result<(), wp_core::ValidationError> {
        match driver {
            Driver::WiFred => self.wifred.validate(req),
            Driver::LongFred => self.longfred.validate(req),
            Driver::Fred => self.fred.validate(req),
        }
    }

    /// Probe a device over the supplied transport.
    pub async fn probe(
        &self,
        driver: Driver,
        transport: Transport<'_>,
    ) -> Result<serde_json::Value, DriverError> {
        match driver {
            Driver::WiFred => self.wifred.probe(transport).await,
            Driver::LongFred => self.longfred.probe(transport).await,
            Driver::Fred => self.fred.probe(transport).await,
        }
    }

    /// Program a device over the supplied transport.
    pub async fn program(
        &self,
        driver: Driver,
        transport: Transport<'_>,
        req: &ProgramRequest<'_>,
        progress: &mut dyn ProgressSink,
    ) -> Result<Outcome, DriverError> {
        match driver {
            Driver::WiFred => self.wifred.program(transport, req, progress).await,
            Driver::LongFred => self.longfred.program(transport, req, progress).await,
            Driver::Fred => self.fred.program(transport, req, progress).await,
        }
    }

    /// Whether this driver can upload firmware over HTTP.
    pub fn supports_firmware_update(&self, driver: Driver) -> bool {
        match driver {
            Driver::WiFred => self.wifred.capabilities().supports_firmware_update,
            Driver::LongFred => self.longfred.capabilities().supports_firmware_update,
            Driver::Fred => false,
        }
    }

    /// Upload firmware over the supplied transport.
    pub async fn update_firmware(
        &self,
        driver: Driver,
        transport: Transport<'_>,
        image: &[u8],
        progress: &mut dyn ProgressSink,
    ) -> Result<Outcome, DriverError> {
        match driver {
            Driver::WiFred => Err(DriverError::Other(
                "firmware update is not supported".into(),
            )),
            Driver::LongFred => {
                self.longfred
                    .update_firmware(transport, image, progress)
                    .await
            }
            Driver::Fred => Err(DriverError::Other(
                "firmware update is not supported".into(),
            )),
        }
    }

    /// Blink a device LED (WiFred Soft-AP only).
    pub async fn blink(
        &self,
        driver: Driver,
        transport: Transport<'_>,
        count: Option<u32>,
    ) -> Result<(), DriverError> {
        match driver {
            Driver::WiFred => {
                let client = match transport {
                    Transport::Http(c) => c,
                    Transport::Bytes(_) => {
                        return Err(DriverError::Other(
                            "wifred identify requires an HTTP transport".into(),
                        ));
                    }
                };
                client
                    .request("GET", &wp_drivers::wifred::identify_request(count), None)
                    .map_err(|e| DriverError::Http(e.to_string()))?;
                Ok(())
            }
            Driver::LongFred => Err(DriverError::Other(
                "LongFred has no LED identify in programming mode".into(),
            )),
            Driver::Fred => Err(DriverError::Other(
                "FRED has no LED identify over Z21 LAN".into(),
            )),
        }
    }

    /// Borrow the WiFred driver.
    pub fn wifred(&self) -> &WiFredDriver {
        &self.wifred
    }

    /// Borrow the LongFred driver.
    pub fn longfred(&self) -> &LongFredDriver {
        &self.longfred
    }
}

impl Default for DriverRegistry {
    fn default() -> Self {
        Self::new()
    }
}
