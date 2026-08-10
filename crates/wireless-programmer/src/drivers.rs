//! Driver registry and enum dispatch.
//!
//! The driver set is closed at compile time, so dispatch uses an enum
//! (guidelines §8.2) rather than `Box<dyn DeviceDriver>`.

use wp_core::{DeviceCandidate, DeviceDriver, DriverCapabilities, Observation};
use wp_drivers::{LongFredDriver, WiFredDriver};

/// All registered drivers.
#[derive(Debug, Clone, Copy)]
pub enum Driver {
    /// NewHeiko WiFred.
    WiFred,
    /// LongFred Soft-AP programming.
    LongFred,
}

impl Driver {
    /// The driver's stable id string.
    pub fn id_str(self) -> &'static str {
        match self {
            Driver::WiFred => "wifred",
            Driver::LongFred => "longfred",
        }
    }

    /// Human-readable name.
    pub fn name(self) -> &'static str {
        match self {
            Driver::WiFred => "NewHeiko WiFred",
            Driver::LongFred => "LongFred",
        }
    }
}

/// A registry of all drivers, owning their instances.
#[derive(Debug)]
pub struct DriverRegistry {
    wifred: WiFredDriver,
    longfred: LongFredDriver,
}

impl DriverRegistry {
    /// Construct a registry with all built-in drivers.
    pub fn new() -> Self {
        Self {
            wifred: WiFredDriver::new(),
            longfred: LongFredDriver::new(),
        }
    }

    /// Iterate over (driver tag, capabilities) for `hello`.
    pub fn drivers(&self) -> Vec<(Driver, DriverCapabilities)> {
        vec![
            (Driver::WiFred, self.wifred.capabilities()),
            (Driver::LongFred, self.longfred.capabilities()),
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
        match candidate.driver.as_str() {
            "wifred" => Some(Driver::WiFred),
            "longfred" => Some(Driver::LongFred),
            _ => None,
        }
    }

    /// Claim a raw observation against every driver.
    pub fn identify(&self, obs: &Observation) -> Option<DeviceCandidate> {
        self.longfred
            .identify(obs)
            .or_else(|| self.wifred.identify(obs))
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
