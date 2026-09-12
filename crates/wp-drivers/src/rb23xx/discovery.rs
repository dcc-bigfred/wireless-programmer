//! RailBOX RB23xx scan/discovery.

use wp_core::{DeviceCandidate, Observation};

use crate::rb23xx::constants::WIFI_CONFIG_SSID_PREFIXES;

/// Claim a raw scan observation as an RB23xx candidate.
///
/// The decoder Soft-AP SSID is `RB2300_XXXXX` (also `RB2310_` / `RB2302_`).
/// Match on those prefixes; the BSSID is the stable candidate key.
pub fn identify(obs: &Observation) -> Option<DeviceCandidate> {
    let ssid = obs.ssid.as_ref()?;
    if !WIFI_CONFIG_SSID_PREFIXES
        .iter()
        .any(|prefix| ssid.starts_with(prefix))
    {
        return None;
    }
    let key = obs.bssid.clone().unwrap_or_else(|| ssid.clone());
    Some(DeviceCandidate {
        driver: "rb23xx".into(),
        key,
        label: ssid.clone(),
        rssi: obs.rssi,
    })
}
