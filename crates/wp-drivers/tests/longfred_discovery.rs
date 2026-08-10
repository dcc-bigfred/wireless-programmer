//! LongFred Soft-AP discovery / identify tests.

use wp_core::Observation;
use wp_drivers::longfred::identify;

#[test]
fn identify_matches_prefix() {
    let obs = Observation {
        ssid: Some("longfred_prog_a1b2c3".into()),
        bssid: Some("aa:bb:cc:dd:ee:ff".into()),
        rssi: Some(-42),
        extra: serde_json::Value::Null,
    };
    let c = identify(&obs).expect("claimed");
    assert_eq!(c.driver, "longfred");
    assert_eq!(c.key, "aa:bb:cc:dd:ee:ff");
    assert_eq!(c.label, "longfred_prog_a1b2c3");
    assert_eq!(c.rssi, Some(-42));
}

#[test]
fn identify_rejects_unrelated_ssid() {
    let obs = Observation {
        ssid: Some("wiFred-configa1b2".into()),
        bssid: None,
        rssi: None,
        extra: serde_json::Value::Null,
    };
    assert!(identify(&obs).is_none());
}

#[test]
fn identify_falls_back_to_ssid_when_no_bssid() {
    let obs = Observation {
        ssid: Some("longfred_prog_ffffff".into()),
        bssid: None,
        rssi: None,
        extra: serde_json::Value::Null,
    };
    let c = identify(&obs).expect("claimed");
    assert_eq!(c.key, "longfred_prog_ffffff");
}

#[test]
fn identify_rejects_missing_ssid() {
    let obs = Observation {
        ssid: None,
        bssid: Some("aa:bb:cc:dd:ee:ff".into()),
        rssi: None,
        extra: serde_json::Value::Null,
    };
    assert!(identify(&obs).is_none());
}
