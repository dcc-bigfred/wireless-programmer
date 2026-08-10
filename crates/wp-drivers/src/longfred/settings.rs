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
