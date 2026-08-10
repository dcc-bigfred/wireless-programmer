//! LongFred scan/discovery.

use wp_core::{DeviceCandidate, Observation};

use crate::longfred::constants::WIFI_CONFIG_SSID_PREFIX;

/// Claim a raw scan observation as a LongFred candidate.
///
/// The programming Soft-AP SSID is `longfred_prog_XXXXXX` (6 hex chars from
/// the MAC). Match on the prefix; the BSSID is the stable candidate key.
pub fn identify(obs: &Observation) -> Option<DeviceCandidate> {
    let ssid = obs.ssid.as_ref()?;
    if !ssid.starts_with(WIFI_CONFIG_SSID_PREFIX) {
        return None;
    }
    let key = obs.bssid.clone().unwrap_or_else(|| ssid.clone());
    Some(DeviceCandidate {
        driver: "longfred".into(),
        key,
        label: ssid.clone(),
        rssi: obs.rssi,
    })
}
