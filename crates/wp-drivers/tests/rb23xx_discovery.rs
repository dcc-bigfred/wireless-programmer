//! RB23xx Soft-AP discovery / identify tests.

use wp_core::Observation;
use wp_drivers::rb23xx::identify;

#[test]
fn identify_matches_rb2300_prefix() {
    let obs = Observation {
        ssid: Some("RB2300_A1B2C".into()),
        bssid: Some("aa:bb:cc:dd:ee:ff".into()),
        rssi: Some(-42),
        extra: serde_json::Value::Null,
    };
    let c = identify(&obs).expect("claimed");
    assert_eq!(c.driver, "rb23xx");
    assert_eq!(c.key, "aa:bb:cc:dd:ee:ff");
    assert_eq!(c.label, "RB2300_A1B2C");
    assert_eq!(c.rssi, Some(-42));
}

#[test]
fn identify_matches_rb2310_and_rb2302() {
    for ssid in ["RB2310_FFFFF", "RB2302_12345"] {
        let obs = Observation {
            ssid: Some(ssid.into()),
            bssid: None,
            rssi: None,
            extra: serde_json::Value::Null,
        };
        let c = identify(&obs).expect(ssid);
        assert_eq!(c.driver, "rb23xx");
        assert_eq!(c.key, ssid);
    }
}

#[test]
fn identify_rejects_unrelated_ssid() {
    let obs = Observation {
        ssid: Some("longfred_prog_a1b2c3".into()),
        bssid: None,
        rssi: None,
        extra: serde_json::Value::Null,
    };
    assert!(identify(&obs).is_none());
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
