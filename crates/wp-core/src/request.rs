//! Programming request domain types.

/// WiFi credentials to write to a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiCredentials<'a> {
    /// SSID.
    pub ssid: &'a str,
    /// PSK (never logged by the daemon).
    pub psk: Option<&'a str>,
}

/// wiThrottle server endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThrottleServer<'a> {
    /// Hostname or IP.
    pub host: &'a str,
    /// TCP port.
    pub port: u16,
    /// Discover via mDNS instead of a fixed host.
    pub automatic: bool,
}

/// One DCC vehicle slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterEntry<'a> {
    /// DCC address (1..=10239). `None` disables the slot.
    pub address: Option<u16>,
    /// True for a long address (>= 128), false for short.
    pub long_address: Option<bool>,
    /// Speed-step mode string (driver-specific vocabulary).
    pub mode: Option<&'a str>,
    /// Direction: 0 forward, 1 reverse, 2 do-not-change.
    pub direction: Option<u8>,
    /// Per-function mapping F0..Fmax.
    pub functions: Vec<FunctionMapping>,
}

/// A function key mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionMapping {
    /// Function index (0..=max_function_index).
    pub index: u8,
    /// Driver-specific mapping value.
    pub value: u8,
}

/// BigFred login credentials for devices that authenticate with login+PIN
/// (e.g. LongFred) rather than a 6-digit wiThrottle pairing code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BigfredCreds<'a> {
    /// BigFred login name.
    pub login: &'a str,
    /// BigFred PIN (never logged by the daemon).
    pub pin: &'a str,
}

/// The full programming request, borrowing caller-owned data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramRequest<'a> {
    /// Opaque device identity (e.g. a 6-digit BigFred pairing code for WiFred).
    pub identity: &'a str,
    /// WiFi network the device should join after programming.
    pub wifi: WifiCredentials<'a>,
    /// wiThrottle server the device should connect to.
    pub server: ThrottleServer<'a>,
    /// DCC vehicle list (capped by the driver's `max_roster_slots`).
    pub roster: Vec<RosterEntry<'a>>,
    /// Optional BigFred login+PIN (LongFred and similar).
    pub bigfred: Option<BigfredCreds<'a>>,
    /// Optional roster mode string (driver-specific, e.g. `"auto"` / `"static"`).
    pub roster_mode: Option<&'a str>,
}
