//! Core device abstraction, capabilities and typed errors for
//! `wireless-programmer`.
//!
//! The design principle (see `CODING-GUIDELINES.md` §1.1, §8.2): a driver never
//! owns transport. The daemon establishes reachability, then hands the driver
//! a [`Transport`]. A future NFC or USB-serial throttle plugs in by declaring
//! a different [`CommissioningKind`] — driver code does not change shape.
//!
//! Driver dispatch uses an enum, not `Box<dyn DeviceDriver>`, because the
//! driver set is closed at compile time (guidelines §8.2).

#![forbid(unsafe_code)]

mod capabilities;
mod driver;
mod error;
mod request;
mod transport;

pub use capabilities::{
    CommissioningKind, CommissioningNet, DriverCapabilities, DriverId, IdentityFormat,
};
pub use driver::{
    validate_common, DeviceCandidate, DeviceDriver, NoProgress, Observation, Outcome, ProgressSink,
    ScanFilters, Transport,
};
pub use error::{DriverError, ValidationError};
pub use request::{
    BigfredCreds, FunctionMapping, ProgramRequest, RosterEntry, ThrottleServer, WifiCredentials,
};
pub use transport::{ByteStream, HttpClient};
