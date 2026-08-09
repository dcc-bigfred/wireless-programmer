//! Typed errors for driver validation and runtime failures.

use thiserror::Error;

/// Validation failure raised before any radio work.
///
/// These are caller-controlled inputs, so they are returned as typed errors
/// rather than panics (guidelines §4.5).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    /// The identity string does not match the driver's required format.
    #[error("identity does not match required format")]
    IdentityFormat,
    /// The roster exceeds the device's slot capacity.
    #[error("roster of {requested} entries exceeds {capacity} slots")]
    CapacityExceeded {
        /// Device capacity.
        capacity: usize,
        /// Requested entry count.
        requested: usize,
    },
    /// A function index is out of range for the device.
    #[error("function index {index} exceeds maximum {max}")]
    FunctionIndexOutOfRange {
        /// Offending index.
        index: u8,
        /// Maximum supported index.
        max: u8,
    },
    /// A DCC address is out of the valid range.
    #[error("dcc address {addr} out of range 1..=10239")]
    AddressOutOfRange {
        /// Offending address.
        addr: u16,
    },
    /// The WiFi SSID is empty.
    #[error("wifi ssid must not be empty")]
    EmptySsid,
    /// The driver does not support a wiThrottle server but one was supplied.
    #[error("driver does not support a throttle server")]
    ServerUnsupported,
    /// A roster entry is otherwise malformed.
    #[error("roster entry at index {slot} is invalid: {reason}")]
    RosterEntry {
        /// Slot index.
        slot: usize,
        /// Reason.
        reason: &'static str,
    },
}

/// Runtime failure during probing or programming.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DriverError {
    /// The radio is blocked by rfkill.
    #[error("radio blocked by rfkill")]
    RadioBlocked,
    /// No wireless interface is available.
    #[error("no wireless interface available")]
    NoInterface,
    /// Association to the device AP timed out.
    #[error("association timed out")]
    AssociationTimedOut,
    /// The device could not be reached at its expected address.
    #[error("device unreachable: {probed}")]
    DeviceUnreachable {
        /// The address that was probed.
        probed: String,
    },
    /// An HTTP request to the device failed.
    #[error("http error: {0}")]
    Http(String),
    /// The device returned a response that could not be parsed.
    #[error("device response parse error: {0}")]
    Parse(String),
    /// The device reported an unsupported structure version.
    #[error("unsupported device structure version: {0}")]
    UnsupportedStructureVersion(String),
    /// Verification found mismatches after writing.
    #[error("verification failed: {mismatches:?}")]
    VerificationFailed {
        /// The field names that did not match.
        mismatches: Vec<String>,
    },
    /// The job was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// A deadline elapsed before the operation completed.
    #[error("deadline elapsed: {0}")]
    DeadlineElapsed(&'static str),
    /// Another driver-specific failure.
    #[error("{0}")]
    Other(String),
}
