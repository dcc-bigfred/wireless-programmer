//! LongFred `/api/v1/settings` JSON helpers.

use serde_json::{json, Value};
use wp_core::ProgramRequest;

/// Build the JSON body for `PUT /api/v1/settings`.
///
/// Maps the domain [`ProgramRequest`] onto LongFred's provisioning DTO:
/// - `wifi.ssid` / `wifi.password` from the request WiFi credentials
/// - `wifi.hostname` from `identity` when non-empty
/// - `bigfred` from optional BigFred credentials
/// - `roster_mode` and `roster` (addresses as `S`/`L` + digits)
pub fn build_settings_put(req: &ProgramRequest<'_>) -> Value {
    let mut body = serde_json::Map::new();

    let mut wifi = serde_json::Map::new();
    wifi.insert("ssid".into(), json!(req.wifi.ssid));
    if let Some(psk) = req.wifi.psk {
        wifi.insert("password".into(), json!(psk));
    }
    if !req.identity.is_empty() {
        wifi.insert("hostname".into(), json!(req.identity));
    }
    body.insert("wifi".into(), Value::Object(wifi));

    if let Some(bf) = req.bigfred {
        body.insert(
            "bigfred".into(),
            json!({
                "login": bf.login,
                "pin": bf.pin,
            }),
        );
    }

    if let Some(mode) = req.roster_mode {
        body.insert("roster_mode".into(), json!(mode));
    }

    let roster: Vec<Value> = req
        .roster
        .iter()
        .filter_map(|entry| {
            let addr = format_roster_addr(entry.address?, entry.long_address)?;
            Some(json!({ "addr": addr }))
        })
        .collect();
    if !roster.is_empty() {
        body.insert("roster".into(), Value::Array(roster));
    }

    Value::Object(body)
}

/// Format a DCC address the way LongFred's static roster expects (`S42`, `L128`).
pub fn format_roster_addr(address: u16, long_address: Option<bool>) -> Option<String> {
    if address == 0 || address > 10239 {
        return None;
    }
    let long = long_address.unwrap_or(address >= 128);
    let prefix = if long { 'L' } else { 'S' };
    Some(format!("{prefix}{address}"))
}

/// Compare a GET settings payload against the request; return mismatch fields.
pub fn verify(settings: &Value, req: &ProgramRequest<'_>) -> Vec<String> {
    let mut mismatches = Vec::new();

    let networks = settings
        .pointer("/wifi/networks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_ssid = networks.iter().any(|n| n.as_str() == Some(req.wifi.ssid));
    if !has_ssid {
        mismatches.push("wifi".into());
    }

    if !req.identity.is_empty() {
        let hostname = settings
            .pointer("/wifi/hostname")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if hostname != req.identity {
            mismatches.push("wifi.hostname".into());
        }
    }

    if let Some(bf) = req.bigfred {
        let login = settings
            .pointer("/bigfred/login")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if login != bf.login {
            mismatches.push("bigfred.login".into());
        }
    }

    if let Some(mode) = req.roster_mode {
        let got = settings
            .pointer("/roster/mode")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if got != mode {
            mismatches.push("roster.mode".into());
        }
    }

    let entries = settings
        .pointer("/roster/entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let want: Vec<String> = req
        .roster
        .iter()
        .filter_map(|e| format_roster_addr(e.address?, e.long_address))
        .collect();
    for (i, addr) in want.iter().enumerate() {
        let got = entries
            .get(i)
            .and_then(|e| e.get("addr"))
            .and_then(Value::as_str);
        if got != Some(addr.as_str()) {
            mismatches.push(format!("roster[{i}].addr"));
        }
    }

    mismatches
}

#[cfg(test)]
mod tests {
    use super::*;
    use wp_core::{
        BigfredCreds, RosterEntry, ThrottleServer, WifiCredentials,
    };

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
}
