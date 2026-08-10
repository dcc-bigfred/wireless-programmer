//! Recording fake HTTP client + write-sequence tests for the LongFred driver.

use std::io;

use serde_json::json;
use wp_core::{
    BigfredCreds, DeviceDriver, HttpClient, ProgramRequest, RosterEntry, ThrottleServer, Transport,
    WifiCredentials,
};
use wp_drivers::LongFredDriver;

struct FakeHttp {
    /// Recorded as `(method, path, body)`.
    requests: Vec<(String, String, Option<Vec<u8>>)>,
    get_settings: std::collections::VecDeque<Vec<u8>>,
}

impl FakeHttp {
    fn queue_settings(&mut self, body: &[u8]) {
        self.get_settings.push_back(body.to_vec());
    }
}

impl HttpClient for FakeHttp {
    fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Option<(&str, &[u8])>,
    ) -> io::Result<Vec<u8>> {
        self.requests.push((
            method.to_string(),
            path.to_string(),
            body.map(|(_, b)| b.to_vec()),
        ));
        match (method, path) {
            ("GET", "/api/v1/settings") => self
                .get_settings
                .pop_front()
                .ok_or_else(|| io::Error::other("no queued settings")),
            ("PUT", "/api/v1/settings") | ("POST", "/api/v1/programming-mode/off") => {
                Ok(Vec::new())
            }
            _ => Err(io::Error::other(format!("unexpected {method} {path}"))),
        }
    }
}

fn ok_settings() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "device": { "name": "Pilot", "id": 4242, "variant": "longfred-standard" },
        "wifi": { "hostname": "pilot1", "networks": ["club-wifi"] },
        "bigfred": { "login": "ops", "pin_set": true },
        "roster": {
            "mode": "static",
            "entries": [ { "addr": "S3" }, { "addr": "L128" } ]
        },
        "programming_mode": true
    }))
    .unwrap()
}

fn make_request<'a>() -> ProgramRequest<'a> {
    ProgramRequest {
        identity: "pilot1",
        wifi: WifiCredentials {
            ssid: "club-wifi",
            psk: Some("secret"),
        },
        server: ThrottleServer {
            host: "bigfred.local",
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

#[tokio::test]
async fn program_put_verify_exit_order() {
    let mut fake = FakeHttp {
        requests: Vec::new(),
        get_settings: std::collections::VecDeque::new(),
    };
    fake.queue_settings(&ok_settings());

    let req = make_request();
    let mut progress = wp_core::NoProgress;
    let transport = Transport::Http(&mut fake);
    let outcome = LongFredDriver::new()
        .program(transport, &req, &mut progress)
        .await
        .expect("program");
    assert!(outcome.restarted);

    assert_eq!(fake.requests.len(), 3);
    assert_eq!(fake.requests[0].0, "PUT");
    assert_eq!(fake.requests[0].1, "/api/v1/settings");
    let put: serde_json::Value =
        serde_json::from_slice(fake.requests[0].2.as_ref().unwrap()).unwrap();
    assert_eq!(put["wifi"]["ssid"], "club-wifi");
    assert_eq!(put["wifi"]["password"], "secret");
    assert_eq!(put["wifi"]["hostname"], "pilot1");
    assert_eq!(put["bigfred"]["login"], "ops");
    assert_eq!(put["roster"][0]["addr"], "S3");
    assert_eq!(put["roster"][1]["addr"], "L128");

    assert_eq!(fake.requests[1].0, "GET");
    assert_eq!(fake.requests[1].1, "/api/v1/settings");

    assert_eq!(fake.requests[2].0, "POST");
    assert_eq!(fake.requests[2].1, "/api/v1/programming-mode/off");
}

#[tokio::test]
async fn probe_returns_settings_json_with_variant() {
    let mut fake = FakeHttp {
        requests: Vec::new(),
        get_settings: std::collections::VecDeque::new(),
    };
    fake.queue_settings(&ok_settings());
    let transport = Transport::Http(&mut fake);
    let info = LongFredDriver::new().probe(transport).await.expect("probe");
    assert_eq!(info["device"]["variant"], "longfred-standard");
    assert_eq!(fake.requests[0].0, "GET");
    assert_eq!(fake.requests[0].1, "/api/v1/settings");
}

#[tokio::test]
async fn program_skips_exit_on_verify_mismatch() {
    let mut fake = FakeHttp {
        requests: Vec::new(),
        get_settings: std::collections::VecDeque::new(),
    };
    fake.queue_settings(
        &serde_json::to_vec(&json!({
            "wifi": { "hostname": "pilot1", "networks": ["wrong"] },
            "bigfred": { "login": "ops" },
            "roster": { "mode": "static", "entries": [] }
        }))
        .unwrap(),
    );
    let req = make_request();
    let mut progress = wp_core::NoProgress;
    let transport = Transport::Http(&mut fake);
    let err = LongFredDriver::new()
        .program(transport, &req, &mut progress)
        .await
        .expect_err("mismatch");
    assert!(matches!(
        err,
        wp_core::DriverError::VerificationFailed { .. }
    ));
    assert_eq!(fake.requests.len(), 2);
    assert_eq!(fake.requests[1].0, "GET");
}
