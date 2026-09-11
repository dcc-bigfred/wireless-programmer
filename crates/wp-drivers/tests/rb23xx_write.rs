//! Recording fake HTTP client + firmware POST tests for the RB23xx driver.

use std::io;

use wp_core::{
    DeviceDriver, HttpClient, ProgramRequest, RosterEntry, ThrottleServer, Transport,
    WifiCredentials,
};
use wp_drivers::Rb23xxDriver;

struct RecordedRequest {
    method: String,
    path: String,
    content_type: Option<String>,
    body: Option<Vec<u8>>,
}

struct FakeHttp {
    requests: Vec<RecordedRequest>,
    responses: std::collections::VecDeque<io::Result<Vec<u8>>>,
}

impl HttpClient for FakeHttp {
    fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Option<(&str, &[u8])>,
    ) -> io::Result<Vec<u8>> {
        self.requests.push(RecordedRequest {
            method: method.to_string(),
            path: path.to_string(),
            content_type: body.map(|(ct, _)| ct.to_string()),
            body: body.map(|(_, b)| b.to_vec()),
        });
        self.responses.pop_front().unwrap_or_else(|| Ok(Vec::new()))
    }
}

#[tokio::test]
async fn update_firmware_posts_raw_bin_like_rb() {
    let mut fake = FakeHttp {
        requests: Vec::new(),
        responses: [Ok(b"Uploaded successfully".to_vec())].into(),
    };
    let image = b"rb-firmware-image".to_vec();
    let mut progress = wp_core::NoProgress;
    let transport = Transport::Http(&mut fake);
    let outcome = Rb23xxDriver::new()
        .update_firmware(
            transport,
            &image,
            "/data/fw/RB_Sound_1.15.1.bin",
            &mut progress,
        )
        .await
        .expect("firmware");
    assert!(outcome.restarted);
    assert_eq!(fake.requests.len(), 1);
    assert_eq!(fake.requests[0].method, "POST");
    assert_eq!(fake.requests[0].path, "/upload?p=/RB_Sound_1.15.1.bin");
    assert_eq!(
        fake.requests[0].content_type.as_deref(),
        Some("multipart/form-data")
    );
    assert_eq!(fake.requests[0].body.as_ref().unwrap(), &image);
}

#[tokio::test]
async fn update_firmware_treats_empty_ok_as_restart() {
    let mut fake = FakeHttp {
        requests: Vec::new(),
        responses: [Ok(Vec::new())].into(),
    };
    let image = vec![1, 2, 3];
    let mut progress = wp_core::NoProgress;
    let transport = Transport::Http(&mut fake);
    let outcome = Rb23xxDriver::new()
        .update_firmware(transport, &image, "firmware.bin", &mut progress)
        .await
        .expect("reboot");
    assert!(outcome.restarted);
}

#[tokio::test]
async fn probe_gets_root_listing() {
    let mut fake = FakeHttp {
        requests: Vec::new(),
        responses: [Ok(b"<html>files</html>".to_vec())].into(),
    };
    let transport = Transport::Http(&mut fake);
    let info = Rb23xxDriver::new().probe(transport).await.expect("probe");
    assert_eq!(
        info.get("ok").and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(fake.requests[0].method, "GET");
    assert_eq!(fake.requests[0].path, "/?p=/");
}

#[tokio::test]
async fn program_is_unsupported() {
    let mut fake = FakeHttp {
        requests: Vec::new(),
        responses: std::collections::VecDeque::new(),
    };
    let req = ProgramRequest {
        identity: "",
        wifi: WifiCredentials {
            ssid: "x",
            psk: None,
        },
        server: ThrottleServer {
            host: "",
            port: 0,
            automatic: false,
        },
        roster: Vec::<RosterEntry<'_>>::new(),
        bigfred: None,
        roster_mode: None,
    };
    let mut progress = wp_core::NoProgress;
    let transport = Transport::Http(&mut fake);
    let err = Rb23xxDriver::new()
        .program(transport, &req, &mut progress)
        .await
        .expect_err("unsupported");
    assert!(err.to_string().contains("updateFirmware"));
    assert!(fake.requests.is_empty());
}
