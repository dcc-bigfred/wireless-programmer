//! Driver capabilities and commissioning model.

use wp_proto::{CapabilitiesWire, CommissioningKindWire, IdentityFormatWire};

/// Stable identifier for a driver implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DriverId(pub &'static str);

impl DriverId {
    /// Construct a driver id from a static string.
    pub const fn new(s: &'static str) -> Self {
        Self(s)
    }
}

/// How a device is reached for commissioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommissioningKind {
    /// Device raises its own WiFi AP.
    SoftAp,
    /// Device is already on the LAN (mDNS).
    Lan,
    /// Device is reached over a serial link.
    Serial,
}

impl From<CommissioningKind> for CommissioningKindWire {
    fn from(kind: CommissioningKind) -> Self {
        match kind {
            CommissioningKind::SoftAp => CommissioningKindWire::SoftAp,
            CommissioningKind::Lan => CommissioningKindWire::Lan,
            CommissioningKind::Serial => CommissioningKindWire::Serial,
        }
    }
}

/// Required format of the device identity string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityFormat {
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

impl IdentityFormat {
    /// Returns `true` when `value` satisfies this format.
    pub fn matches(self, value: &str) -> bool {
        match self {
            IdentityFormat::Digits { len } => {
                let want = usize::from(len);
                value.len() == want && value.chars().all(|c| c.is_ascii_digit())
            }
            IdentityFormat::Alphanumeric { max_len } => {
                let limit = usize::from(max_len);
                !value.is_empty()
                    && value.len() <= limit
                    && value.chars().all(|c| c.is_ascii_alphanumeric())
            }
            IdentityFormat::Any => true,
        }
    }
}

impl From<IdentityFormat> for IdentityFormatWire {
    fn from(fmt: IdentityFormat) -> Self {
        match fmt {
            IdentityFormat::Digits { len } => IdentityFormatWire::Digits { len },
            IdentityFormat::Alphanumeric { max_len } => {
                IdentityFormatWire::Alphanumeric { max_len }
            }
            IdentityFormat::Any => IdentityFormatWire::Any,
        }
    }
}

/// What a driver can do, advertised to callers via `hello`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverCapabilities {
    /// Maximum roster slots the device can store.
    pub max_roster_slots: u8,
    /// Highest function index the device understands.
    pub max_function_index: u8,
    /// Required format of the identity string.
    pub identity_format: IdentityFormat,
    /// Whether the device accepts a wiThrottle server endpoint.
    pub supports_throttle_server: bool,
    /// How the device is commissioned.
    pub commissioning: CommissioningKind,
}

impl From<DriverCapabilities> for CapabilitiesWire {
    fn from(c: DriverCapabilities) -> Self {
        CapabilitiesWire {
            max_roster_slots: c.max_roster_slots,
            max_function_index: c.max_function_index,
            identity_format: c.identity_format.into(),
            supports_throttle_server: c.supports_throttle_server,
            commissioning: c.commissioning.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_format_rejects_non_digit_and_wrong_length() {
        let fmt = IdentityFormat::Digits { len: 6 };
        assert!(fmt.matches("122145"));
        assert!(!fmt.matches("12214"));
        assert!(!fmt.matches("1221456"));
        assert!(!fmt.matches("12a145"));
        assert!(!fmt.matches(""));
    }

    #[test]
    fn alphanumeric_format_enforces_length_and_charset() {
        let fmt = IdentityFormat::Alphanumeric { max_len: 8 };
        assert!(fmt.matches("abc123"));
        assert!(fmt.matches("ABCDEFGH"));
        assert!(!fmt.matches("ABCDEFGHI"));
        assert!(!fmt.matches("ab-cd"));
    }

    #[test]
    fn any_format_accepts_everything() {
        let fmt = IdentityFormat::Any;
        assert!(fmt.matches(""));
        assert!(fmt.matches("anything goes? no: still matches"));
    }
}
