//! Driver capabilities and commissioning model.

use std::net::Ipv4Addr;

use wp_proto::{
    CapabilitiesWire, CommissioningKindWire, CommissioningNetWire, IdentityFormatWire, ReachMode,
};

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

/// On-link Soft-AP addressing for commissioning.
///
/// When present on [`DriverCapabilities`], the daemon should bind the wireless
/// interface to `source/prefix` and talk to `host:port`. When absent, the
/// daemon keeps its historical defaults (`192.168.4.1` / `192.168.4.2/24`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommissioningNet {
    /// Device Soft-AP address (e.g. `192.168.4.1`).
    pub host: Ipv4Addr,
    /// HTTP port on the Soft-AP (typically 80).
    pub port: u16,
    /// Address the hub assigns on the wireless interface (e.g. `192.168.4.2`).
    pub source: Ipv4Addr,
    /// Prefix length of the assigned address (typically 24). The kernel
    /// prefix route is suppressed; only a `/32` to [`Self::host`] is
    /// installed.
    pub prefix: u8,
}

impl From<CommissioningNet> for CommissioningNetWire {
    fn from(n: CommissioningNet) -> Self {
        CommissioningNetWire {
            host: n.host.to_string(),
            port: n.port,
            source: n.source.to_string(),
            prefix: n.prefix,
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

/// Which `updateFirmware` reach paths a driver accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FirmwareModes {
    /// Soft-AP HTTP upload.
    pub ap: bool,
    /// Layout LAN HTTP upload.
    pub lan: bool,
    /// USB serial (`espflash`).
    pub usb: bool,
}

impl FirmwareModes {
    /// Soft-AP only (e.g. RailBOX file-browser).
    pub const AP: Self = Self {
        ap: true,
        lan: false,
        usb: false,
    };

    /// Soft-AP, LAN HTTP OTA, and USB (e.g. LongFred).
    pub const AP_LAN_USB: Self = Self {
        ap: true,
        lan: true,
        usb: true,
    };

    /// Whether `mode` is an accepted firmware path. `Z21` is never a firmware
    /// path; drivers that do not support a mode simply leave it `false`.
    #[must_use]
    pub fn allows(self, mode: ReachMode) -> bool {
        match mode {
            ReachMode::Ap => self.ap,
            ReachMode::Lan => self.lan,
            ReachMode::Usb => self.usb,
            ReachMode::Z21 => false,
        }
    }

    fn to_reach_modes(self) -> Vec<ReachMode> {
        let mut modes = Vec::new();
        if self.ap {
            modes.push(ReachMode::Ap);
        }
        if self.lan {
            modes.push(ReachMode::Lan);
        }
        if self.usb {
            modes.push(ReachMode::Usb);
        }
        modes
    }
}

/// Firmware-upload policy. Present when the driver supports `updateFirmware`.
///
/// Shared job code reads these fields instead of matching on a driver id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareCapabilities {
    /// Maximum image size loaded into RAM for HTTP upload.
    pub max_bytes: u64,
    /// Human-readable cap used in error strings (e.g. `"5 MiB"`).
    pub max_bytes_label: &'static str,
    /// Accepted `updateFirmware` reach paths.
    pub modes: FirmwareModes,
    /// HTTP images must be an ESP `.app.bin` (magic `0xE9`).
    pub require_esp_app_bin: bool,
    /// TCP RST/EOF after a full request write is success (device reboot).
    pub success_on_reset_after_write: bool,
}

impl FirmwareCapabilities {
    /// Whether `mode` is an accepted firmware path.
    #[must_use]
    pub fn allows(self, mode: ReachMode) -> bool {
        self.modes.allows(mode)
    }

    /// Error detail when the image is larger than [`Self::max_bytes`].
    #[must_use]
    pub fn too_large_detail(self) -> String {
        format!("firmware image exceeds {}", self.max_bytes_label)
    }

    /// Error detail when the requested reach path is not in [`Self::modes`].
    #[must_use]
    pub fn mode_rejected_detail(self) -> &'static str {
        if self.modes == FirmwareModes::AP {
            "firmware update is Soft-AP only"
        } else {
            "firmware update is not supported in this mode"
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
    /// Soft-AP addressing for commissioning, when the driver does not use the
    /// daemon's historical `192.168.4.x` defaults.
    pub commissioning_net: Option<CommissioningNet>,
    /// WPA2-PSK passphrase for the device Soft-AP, when it is not open.
    /// These are publicly documented factory default passwords for a
    /// temporary commissioning AP, not secrets.
    pub softap_psk: Option<&'static str>,
    /// Firmware-upload policy, when the driver supports `updateFirmware`.
    pub firmware: Option<FirmwareCapabilities>,
}

impl From<DriverCapabilities> for CapabilitiesWire {
    fn from(c: DriverCapabilities) -> Self {
        CapabilitiesWire {
            max_roster_slots: c.max_roster_slots,
            max_function_index: c.max_function_index,
            identity_format: c.identity_format.into(),
            supports_throttle_server: c.supports_throttle_server,
            commissioning: c.commissioning.into(),
            supports_firmware_update: c.firmware.is_some(),
            commissioning_net: c.commissioning_net.map(Into::into),
            max_firmware_bytes: c.firmware.map(|f| f.max_bytes),
            firmware_modes: c
                .firmware
                .map(|f| f.modes.to_reach_modes())
                .unwrap_or_default(),
            firmware_require_esp_app_bin: c.firmware.is_some_and(|f| f.require_esp_app_bin),
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

    fn firmware_caps() -> FirmwareCapabilities {
        FirmwareCapabilities {
            max_bytes: 5 * 1024 * 1024,
            max_bytes_label: "5 MiB",
            modes: FirmwareModes::AP,
            require_esp_app_bin: false,
            success_on_reset_after_write: true,
        }
    }

    #[test]
    fn firmware_modes_ap_only_rejects_lan_and_usb() {
        use wp_proto::ReachMode;
        let modes = FirmwareModes::AP;
        assert!(modes.allows(ReachMode::Ap));
        assert!(!modes.allows(ReachMode::Lan));
        assert!(!modes.allows(ReachMode::Usb));
        assert!(!modes.allows(ReachMode::Z21));
    }

    #[test]
    fn firmware_caps_drive_error_details_and_hello_wire() {
        let fw = firmware_caps();
        assert_eq!(fw.too_large_detail(), "firmware image exceeds 5 MiB");
        assert_eq!(fw.mode_rejected_detail(), "firmware update is Soft-AP only");

        let wire = CapabilitiesWire::from(DriverCapabilities {
            max_roster_slots: 0,
            max_function_index: 0,
            identity_format: IdentityFormat::Any,
            supports_throttle_server: false,
            commissioning: CommissioningKind::SoftAp,
            commissioning_net: None,
            softap_psk: Some("00000000"),
            firmware: Some(fw),
        });
        assert!(wire.supports_firmware_update);
        assert_eq!(wire.max_firmware_bytes, Some(fw.max_bytes));
        assert_eq!(wire.firmware_modes, vec![ReachMode::Ap]);
        assert!(!wire.firmware_require_esp_app_bin);
    }

    #[test]
    fn hello_omits_firmware_policy_when_unsupported() {
        let wire = CapabilitiesWire::from(DriverCapabilities {
            max_roster_slots: 4,
            max_function_index: 16,
            identity_format: IdentityFormat::Digits { len: 6 },
            supports_throttle_server: true,
            commissioning: CommissioningKind::SoftAp,
            commissioning_net: None,
            softap_psk: None,
            firmware: None,
        });
        assert!(!wire.supports_firmware_update);
        assert_eq!(wire.max_firmware_bytes, None);
        assert!(wire.firmware_modes.is_empty());
        assert!(!wire.firmware_require_esp_app_bin);
    }
}
