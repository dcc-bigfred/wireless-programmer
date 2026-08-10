//! Settings JSON helpers for the LongFred driver.

use serde_json::json;
use wp_core::{BigfredCreds, ProgramRequest, RosterEntry, ThrottleServer, WifiCredentials};
use wp_drivers::longfred::{build_settings_put, format_roster_addr, verify};

fn base_req<'a>() -> ProgramRequest<'a> {
    ProgramRequest {
        identity: "pilot1",
        wifi: WifiCredentials {
            ssid: "club-wifi",
            psk: Some("secret"),
        },
        server: ThrottleServer {
            host: "unused.local",
            port: 12090,
            automatic: false,
        },
        roster: vec![
            RosterEntry {
                address: Some(3),
                long_address: Some(false),
                mode: None,
                direction: None,
                functions: Vec::new(),
            },
            RosterEntry {
                address: Some(128),
                long_address: Some(true),
                mode: None,
                direction: None,
                functions: Vec::new(),
            },
        ],
        bigfred: Some(BigfredCreds {
            login: "ops",
            pin: "1234",
        }),
        roster_mode: Some("static"),
    }
}

#[test]
fn build_settings_put_shape() {
    let body = build_settings_put(&base_req());
    assert_eq!(body["wifi"]["ssid"], "club-wifi");
    assert_eq!(body["wifi"]["password"], "secret");
    assert_eq!(body["wifi"]["hostname"], "pilot1");
    assert_eq!(body["bigfred"]["login"], "ops");
    assert_eq!(body["bigfred"]["pin"], "1234");
    assert_eq!(body["roster_mode"], "static");
    assert_eq!(body["roster"][0]["addr"], "S3");
    assert_eq!(body["roster"][1]["addr"], "L128");
}

#[test]
fn format_roster_addr_defaults_long_at_128() {
    assert_eq!(format_roster_addr(42, None).as_deref(), Some("S42"));
    assert_eq!(format_roster_addr(128, None).as_deref(), Some("L128"));
    assert_eq!(format_roster_addr(10, Some(true)).as_deref(), Some("L10"));
}

#[test]
fn verify_accepts_matching_settings() {
    let settings = json!({
        "device": { "name": "Pilot", "id": 4242, "variant": "longfred-standard" },
        "wifi": { "hostname": "pilot1", "networks": ["club-wifi"] },
        "bigfred": { "login": "ops", "pin_set": true },
        "roster": {
            "mode": "static",
            "entries": [ { "addr": "S3" }, { "addr": "L128" } ]
        },
        "programming_mode": true
    });
    assert!(verify(&settings, &base_req()).is_empty());
}

#[test]
fn verify_reports_wifi_mismatch() {
    let settings = json!({
        "wifi": { "hostname": "pilot1", "networks": ["other"] },
        "bigfred": { "login": "ops" },
        "roster": { "mode": "static", "entries": [] }
    });
    let m = verify(&settings, &base_req());
    assert!(m.contains(&"wifi".into()), "{m:?}");
}

#[test]
fn verify_reports_bigfred_login_mismatch() {
    let settings = json!({
        "wifi": { "hostname": "pilot1", "networks": ["club-wifi"] },
        "bigfred": { "login": "wrong" },
        "roster": { "mode": "static", "entries": [
            { "addr": "S3" }, { "addr": "L128" }
        ] }
    });
    let m = verify(&settings, &base_req());
    assert!(
        m.contains(&"bigfred.login".into()),
        "expected bigfred.login mismatch, got {m:?}"
    );
}

#[test]
fn verify_reports_roster_mode_mismatch() {
    let settings = json!({
        "wifi": { "hostname": "pilot1", "networks": ["club-wifi"] },
        "bigfred": { "login": "ops" },
        "roster": { "mode": "auto", "entries": [
            { "addr": "S3" }, { "addr": "L128" }
        ] }
    });
    let m = verify(&settings, &base_req());
    assert!(
        m.contains(&"roster.mode".into()),
        "expected roster.mode mismatch, got {m:?}"
    );
}
