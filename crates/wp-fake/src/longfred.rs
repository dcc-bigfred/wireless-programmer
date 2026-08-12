//! LongFred Soft-AP HTTP mock.

use serde_json::{json, Value};

use crate::device::{not_found, ok_json, ok_text, FakeDevice, FakeRequest, FakeResponse};

/// Fake LongFred programming-mode HTTP device.
pub struct LongFredFake {
    /// GET-shaped settings document.
    pub settings: Value,
    /// Whether programming mode is still active.
    pub programming_mode: bool,
}

impl LongFredFake {
    /// Default factory settings in programming mode.
    #[must_use]
    pub fn new() -> Self {
        Self {
            settings: json!({
                "wifi": { "hostname": "", "networks": [] },
                "roster": { "mode": "static", "entries": [] },
                "bigfred": { "login": "", "pin_set": false },
                "programming_mode": true
            }),
            programming_mode: true,
        }
    }

    fn apply_put(&mut self, body: &Value) {
        // wifi.ssid → push into wifi.networks; keep hostname from wifi.hostname
        if let Some(wifi) = body.get("wifi") {
            if let Some(hostname) = wifi.get("hostname").and_then(Value::as_str) {
                if let Some(obj) = self.settings.get_mut("wifi").and_then(Value::as_object_mut) {
                    obj.insert("hostname".into(), json!(hostname));
                }
            }
            if let Some(ssid) = wifi.get("ssid").and_then(Value::as_str) {
                let networks = self
                    .settings
                    .pointer_mut("/wifi/networks")
                    .and_then(Value::as_array_mut);
                if let Some(arr) = networks {
                    if !arr.iter().any(|n| n.as_str() == Some(ssid)) {
                        arr.push(json!(ssid));
                    }
                }
            }
        }

        if let Some(login) = body.pointer("/bigfred/login").and_then(Value::as_str) {
            if let Some(obj) = self.settings.get_mut("bigfred").and_then(Value::as_object_mut) {
                obj.insert("login".into(), json!(login));
                obj.insert("pin_set".into(), json!(true));
            }
        }

        if let Some(mode) = body.get("roster_mode").and_then(Value::as_str) {
            if let Some(obj) = self.settings.get_mut("roster").and_then(Value::as_object_mut) {
                obj.insert("mode".into(), json!(mode));
            }
        }

        if let Some(roster) = body.get("roster").and_then(Value::as_array) {
            if let Some(obj) = self.settings.get_mut("roster").and_then(Value::as_object_mut) {
                obj.insert("entries".into(), Value::Array(roster.clone()));
            }
        }
    }
}

impl Default for LongFredFake {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeDevice for LongFredFake {
    fn driver_id(&self) -> &'static str {
        "longfred"
    }

    fn handle(&mut self, req: FakeRequest<'_>) -> FakeResponse {
        let path = req.path.split('?').next().unwrap_or(req.path);

        match (req.method, path) {
            ("GET", "/api/v1/settings") => {
                let body = serde_json::to_vec(&self.settings).unwrap_or_default();
                ok_json(body)
            }
            ("PUT", "/api/v1/settings") => {
                let raw = req.body.unwrap_or(b"{}");
                match serde_json::from_slice::<Value>(raw) {
                    Ok(body) => {
                        self.apply_put(&body);
                        ok_json(b"{}".to_vec())
                    }
                    Err(_) => FakeResponse {
                        status: 400,
                        content_type: "text/plain",
                        body: b"bad json".to_vec(),
                    },
                }
            }
            ("POST", "/api/v1/programming-mode/off") => {
                self.programming_mode = false;
                if let Some(obj) = self.settings.as_object_mut() {
                    obj.insert("programming_mode".into(), json!(false));
                }
                ok_text("ok")
            }
            _ => not_found(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wp_core::{BigfredCreds, ProgramRequest, RosterEntry, ThrottleServer, WifiCredentials};
    use wp_drivers::longfred::{build_settings_put, verify};

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
    fn put_to_get_round_trip_verifies() {
        let mut fake = LongFredFake::new();
        let req = base_req();
        let put = build_settings_put(&req);
        let body = serde_json::to_vec(&put).expect("serialize");

        let resp = fake.handle(FakeRequest {
            method: "PUT",
            path: "/api/v1/settings",
            body: Some(&body),
        });
        assert_eq!(resp.status, 200);

        let get = fake.handle(FakeRequest {
            method: "GET",
            path: "/api/v1/settings",
            body: None,
        });
        assert_eq!(get.status, 200);
        let settings: Value = serde_json::from_slice(&get.body).expect("json");
        let mismatches = verify(&settings, &req);
        assert!(
            mismatches.is_empty(),
            "expected verify to pass, got {mismatches:?}; settings={settings}"
        );
    }

    #[test]
    fn programming_mode_off() {
        let mut fake = LongFredFake::new();
        assert!(fake.programming_mode);
        let resp = fake.handle(FakeRequest {
            method: "POST",
            path: "/api/v1/programming-mode/off",
            body: None,
        });
        assert_eq!(resp.status, 200);
        assert!(!fake.programming_mode);
        assert_eq!(fake.settings["programming_mode"], false);
    }
}
