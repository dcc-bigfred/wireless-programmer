//! WiFred scan/discovery.

#![allow(dead_code)]

use wp_core::{DeviceCandidate, Observation};
use wp_link::ScanResult;

use crate::wifred::constants::WIFI_CONFIG_SSID_PREFIX;

/// Claim a raw scan observation as a WiFred candidate.
///
/// The config AP SSID is `wiFred-config<mac>` (unpadded hex), so we match on
/// the prefix. The BSSID is the stable candidate key (the firmware's
/// `<macAdress>` is built from the STA MAC with the same unpadded hex and is
/// therefore not a reliable unique id).
pub fn identify(obs: &Observation) -> Option<DeviceCandidate> {
    let ssid = obs.ssid.as_ref()?;
    if !ssid.starts_with(WIFI_CONFIG_SSID_PREFIX) {
        return None;
    }
    let key = obs.bssid.clone().unwrap_or_else(|| ssid.clone());
    Some(DeviceCandidate {
        driver: "wifred".into(),
        key,
        label: ssid.clone(),
        rssi: obs.rssi,
    })
}

/// Convert a [`wp_link::ScanResult`] into an [`Observation`] for the driver's
/// `identify` step.
pub fn observation_from_scan(s: &ScanResult) -> Observation {
    Observation {
        ssid: s.ssid.clone(),
        bssid: s.bssid.clone(),
        rssi: s.rssi,
        extra: serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identify_matches_prefix() {
        let obs = Observation {
            ssid: Some("wiFred-configa1b2".into()),
            bssid: Some("aa:bb:cc:dd:ee:ff".into()),
            rssi: Some(-55),
            extra: serde_json::Value::Null,
        };
        let c = identify(&obs).expect("claimed");
        assert_eq!(c.driver, "wifred");
        assert_eq!(c.key, "aa:bb:cc:dd:ee:ff");
        assert_eq!(c.label, "wiFred-configa1b2");
        assert_eq!(c.rssi, Some(-55));
    }

    #[test]
    fn identify_rejects_unrelated_ssid() {
        let obs = Observation {
            ssid: Some("home-wifi".into()),
            bssid: None,
            rssi: None,
            extra: serde_json::Value::Null,
        };
        assert!(identify(&obs).is_none());
    }

    #[test]
    fn identify_falls_back_to_ssid_when_no_bssid() {
        let obs = Observation {
            ssid: Some("wiFred-configf0".into()),
            bssid: None,
            rssi: None,
            extra: serde_json::Value::Null,
        };
        let c = identify(&obs).expect("claimed");
        assert_eq!(c.key, "wiFred-configf0");
    }

    #[test]
    fn identify_rejects_missing_ssid() {
        let obs = Observation {
            ssid: None,
            bssid: None,
            rssi: None,
            extra: serde_json::Value::Null,
        };
        assert!(identify(&obs).is_none());
    }
}
