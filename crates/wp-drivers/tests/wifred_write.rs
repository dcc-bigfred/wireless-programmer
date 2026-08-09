//! Recording fake HTTP client + write-sequence tests for the WiFred driver.

use std::io;

use wp_core::{
    DeviceDriver, FunctionMapping, HttpClient, ProgramRequest, RosterEntry, ThrottleServer,
    Transport, WifiCredentials,
};
use wp_drivers::{Direction, WiFredDriver};

/// A path-aware fake HTTP client. Returns queued XML bodies for
/// `/api/getConfigXML` (one per read, in order) and an empty body for every
/// other path. Records every requested path in order.
struct FakeHttp {
    requests: Vec<String>,
    xml_responses: std::collections::VecDeque<Vec<u8>>,
}

impl FakeHttp {
    fn queue_xml(&mut self, body: &[u8]) {
        self.xml_responses.push_back(body.to_vec());
    }
}

impl HttpClient for FakeHttp {
    fn get(&mut self, path: &str) -> io::Result<Vec<u8>> {
        self.requests.push(path.to_string());
        if path == "/api/getConfigXML" {
            self.xml_responses
                .pop_front()
                .ok_or_else(|| io::Error::other("no queued XML response"))
        } else {
            Ok(Vec::new())
        }
    }
}

fn ok_xml() -> Vec<u8> {
    b"<?XML version=\"1.0\" encoding=\"UTF?8\"?><wiFred>\
<structurVersion value=\"1\"/>\
<throttleName value=\"122145\"/>\
<LOCOS><LOCO ID=\"1\"><DCCadress value=\"3\"/><Mode value=\"128\"/><Direction value=\"0\"/><LongAdress value=\"0\"/></LOCO></LOCOS>\
<NETWORKS><NETWORK><SSID value=\"bigfred2\"/><Key value=\"x\"/><Enabled value=\"1\"/></NETWORK></NETWORKS>\
<LOCOSERVER><ServerName value=\"bigfred.local\"/><Port value=\"12090\"/><Automatic value=\"0\"/></LOCOSERVER>\
</wiFred>"
    .to_vec()
}

fn make_request<'a>() -> ProgramRequest<'a> {
    ProgramRequest {
        identity: "122145",
        wifi: WifiCredentials {
            ssid: "bigfred2",
            psk: Some("super-secret"),
        },
        server: ThrottleServer {
            host: "bigfred.local",
            port: 12090,
            automatic: false,
        },
        roster: vec![RosterEntry {
            address: Some(3),
            long_address: Some(false),
            mode: Some("128"),
            direction: Some(Direction::Normal as u8),
            functions: vec![
                FunctionMapping { index: 0, value: 0 },
                FunctionMapping { index: 1, value: 4 },
            ],
        }],
    }
}

#[tokio::test]
async fn program_writes_in_the_required_order() {
    let mut fake = FakeHttp {
        requests: Vec::new(),
        xml_responses: std::collections::VecDeque::new(),
    };
    // read, verify (two XML reads), plus restart.
    fake.queue_xml(&ok_xml());
    fake.queue_xml(&ok_xml());
    // restart needs no XML

    let req = make_request();
    let mut progress = wp_core::NoProgress;
    let transport = Transport::Http(&mut fake);
    let outcome = WiFredDriver::new()
        .program(transport, &req, &mut progress)
        .await
        .expect("program");
    assert!(outcome.restarted);

    // The exact sequence of paths the driver must issue, in order.
    let expected: &[&str] = &[
        "/api/getConfigXML",
        "/index.html?throttleName=122145",
        "/index.html?loco=1&loco.address=3&loco.mode=128&loco.direction=0",
        "/index.html?loco=1&f0=0&f1=4",
        "/index.html?loco.serverName=bigfred.local&loco.serverPort=12090",
        "/index.html?remove=bigfred2",
        "/index.html?wifiSSID=bigfred2&wifiKEY=super-secret",
        "/api/getConfigXML",
        "/restart.html",
    ];
    assert_eq!(fake.requests.as_slice(), expected);
}

#[tokio::test]
async fn program_disables_unused_slot_with_minus_one() {
    let mut fake = FakeHttp {
        requests: Vec::new(),
        xml_responses: std::collections::VecDeque::new(),
    };
    // Two-slot roster where slot 2 is disabled (no address).
    let xml = b"<?XML version=\"1.0\" encoding=\"UTF?8\"?><wiFred>\
<structurVersion value=\"1\"/>\
<throttleName value=\"122145\"/>\
<LOCOS>\
<LOCO ID=\"1\"><DCCadress value=\"3\"/><Mode value=\"128\"/><Direction value=\"0\"/><LongAdress value=\"0\"/></LOCO>\
<LOCO ID=\"2\"><DCCadress value=\"-1\"/><Mode value=\"\"/><Direction value=\"0\"/><LongAdress value=\"0\"/></LOCO>\
</LOCOS>\
<NETWORKS><NETWORK><SSID value=\"bigfred2\"/><Key value=\"x\"/><Enabled value=\"1\"/></NETWORK></NETWORKS>\
<LOCOSERVER><ServerName value=\"bigfred.local\"/><Port value=\"12090\"/><Automatic value=\"0\"/></LOCOSERVER>\
</wiFred>"
    .to_vec();
    fake.queue_xml(&xml);
    fake.queue_xml(&xml);
    // restart needs no XML

    let req = ProgramRequest {
        identity: "122145",
        wifi: WifiCredentials {
            ssid: "bigfred2",
            psk: Some("pw"),
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
                mode: Some("128"),
                direction: Some(Direction::Normal as u8),
                functions: vec![],
            },
            RosterEntry {
                address: None,
                long_address: None,
                mode: None,
                direction: None,
                functions: vec![],
            },
        ],
    };
    let mut progress = wp_core::NoProgress;
    let transport = Transport::Http(&mut fake);
    let outcome = WiFredDriver::new()
        .program(transport, &req, &mut progress)
        .await
        .expect("program");
    assert!(outcome.restarted);
    // Slot 2 must be written with address=-1 to disable it.
    assert!(
        fake.requests
            .iter()
            .any(|p| p == "/index.html?loco=2&loco.address=-1&loco.mode=&loco.direction=2"),
        "slot 2 should be disabled with -1, got {:?}",
        fake.requests
    );
}

#[tokio::test]
async fn program_emits_long_address_flag_only_when_true() {
    let mut fake = FakeHttp {
        requests: Vec::new(),
        xml_responses: std::collections::VecDeque::new(),
    };
    let xml = b"<?XML version=\"1.0\" encoding=\"UTF?8\"?><wiFred>\
<structurVersion value=\"1\"/>\
<throttleName value=\"122145\"/>\
<LOCOS><LOCO ID=\"1\"><DCCadress value=\"300\"/><Mode value=\"128\"/><Direction value=\"0\"/><LongAdress value=\"1\"/></LOCO></LOCOS>\
<NETWORKS><NETWORK><SSID value=\"bigfred2\"/><Key value=\"x\"/><Enabled value=\"1\"/></NETWORK></NETWORKS>\
<LOCOSERVER><ServerName value=\"bigfred.local\"/><Port value=\"12090\"/><Automatic value=\"0\"/></LOCOSERVER>\
</wiFred>"
    .to_vec();
    fake.queue_xml(&xml);
    fake.queue_xml(&xml);
    // restart needs no XML

    let req = ProgramRequest {
        identity: "122145",
        wifi: WifiCredentials {
            ssid: "bigfred2",
            psk: Some("pw"),
        },
        server: ThrottleServer {
            host: "bigfred.local",
            port: 12090,
            automatic: false,
        },
        roster: vec![RosterEntry {
            address: Some(300),
            long_address: Some(true),
            mode: Some("128"),
            direction: Some(Direction::Normal as u8),
            functions: vec![],
        }],
    };
    let mut progress = wp_core::NoProgress;
    let transport = Transport::Http(&mut fake);
    WiFredDriver::new()
        .program(transport, &req, &mut progress)
        .await
        .expect("program");
    // longAddress=on must appear for a long address.
    assert!(
        fake.requests
            .iter()
            .any(|p| p.contains("loco.longAddress=on")),
        "expected longAddress=on, got {:?}",
        fake.requests
    );
    // And exactly one loco write for slot 1.
    let loco1 = fake
        .requests
        .iter()
        .filter(|p| p.starts_with("/index.html?loco=1&loco.address"))
        .count();
    assert_eq!(loco1, 1);
}

#[tokio::test]
async fn program_emits_automatic_flag_when_requested() {
    let mut fake = FakeHttp {
        requests: Vec::new(),
        xml_responses: std::collections::VecDeque::new(),
    };
    let xml = b"<?XML version=\"1.0\" encoding=\"UTF?8\"?><wiFred>\
<structurVersion value=\"1\"/>\
<throttleName value=\"122145\"/>\
<LOCOS><LOCO ID=\"1\"><DCCadress value=\"3\"/><Mode value=\"128\"/><Direction value=\"0\"/><LongAdress value=\"0\"/></LOCO></LOCOS>\
<NETWORKS><NETWORK><SSID value=\"bigfred2\"/><Key value=\"x\"/><Enabled value=\"1\"/></NETWORK></NETWORKS>\
<LOCOSERVER><ServerName value=\"bigfred.local\"/><Port value=\"12090\"/><Automatic value=\"1\"/></LOCOSERVER>\
</wiFred>"
    .to_vec();
    fake.queue_xml(&xml);
    fake.queue_xml(&xml);
    // restart needs no XML

    let req = ProgramRequest {
        identity: "122145",
        wifi: WifiCredentials {
            ssid: "bigfred2",
            psk: Some("pw"),
        },
        server: ThrottleServer {
            host: "bigfred.local",
            port: 12090,
            automatic: true,
        },
        roster: vec![RosterEntry {
            address: Some(3),
            long_address: Some(false),
            mode: Some("128"),
            direction: Some(Direction::Normal as u8),
            functions: vec![],
        }],
    };
    let mut progress = wp_core::NoProgress;
    let transport = Transport::Http(&mut fake);
    WiFredDriver::new()
        .program(transport, &req, &mut progress)
        .await
        .expect("program");
    assert!(
        fake.requests
            .iter()
            .any(|p| p.contains("loco.automatic=on")),
        "expected automatic=on, got {:?}",
        fake.requests
    );
}

#[tokio::test]
async fn program_fails_on_verification_mismatch() {
    let mut fake = FakeHttp {
        requests: Vec::new(),
        xml_responses: std::collections::VecDeque::new(),
    };
    let before = b"<?XML version=\"1.0\" encoding=\"UTF?8\"?><wiFred>\
<structurVersion value=\"1\"/>\
<throttleName value=\"old\"/>\
<LOCOS><LOCO ID=\"1\"><DCCadress value=\"3\"/><Mode value=\"128\"/><Direction value=\"0\"/><LongAdress value=\"0\"/></LOCO></LOCOS>\
<NETWORKS></NETWORKS>\
<LOCOSERVER><ServerName value=\"bigfred.local\"/><Port value=\"12090\"/><Automatic value=\"0\"/></LOCOSERVER>\
</wiFred>"
    .to_vec();
    // After-write XML still reports the OLD identity (device did not persist).
    fake.queue_xml(&before);
    fake.queue_xml(&before);
    // No restart queued: verification fails first.

    let req = make_request();
    let mut progress = wp_core::NoProgress;
    let transport = Transport::Http(&mut fake);
    let err = WiFredDriver::new()
        .program(transport, &req, &mut progress)
        .await
        .expect_err("should fail verification");
    assert!(
        matches!(err, wp_core::DriverError::VerificationFailed { .. }),
        "got {err:?}"
    );
    // restart.html must NOT have been issued.
    assert!(
        !fake.requests.iter().any(|p| p == "/restart.html"),
        "restart must not fire on verification failure"
    );
}

#[tokio::test]
async fn validate_rejects_too_many_vehicles() {
    let driver = WiFredDriver::new();
    let mut roster = Vec::new();
    for _ in 0..5 {
        roster.push(RosterEntry {
            address: Some(3),
            long_address: Some(false),
            mode: Some("128"),
            direction: Some(0),
            functions: vec![],
        });
    }
    let req = ProgramRequest {
        identity: "122145",
        wifi: WifiCredentials {
            ssid: "bigfred2",
            psk: None,
        },
        server: ThrottleServer {
            host: "bigfred.local",
            port: 12090,
            automatic: false,
        },
        roster,
    };
    let err = driver.validate(&req).expect_err("should reject");
    assert!(
        matches!(
            err,
            wp_core::ValidationError::CapacityExceeded {
                capacity: 4,
                requested: 5
            }
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn validate_rejects_non_digit_identity() {
    let driver = WiFredDriver::new();
    let req = ProgramRequest {
        identity: "abc",
        wifi: WifiCredentials {
            ssid: "bigfred2",
            psk: None,
        },
        server: ThrottleServer {
            host: "bigfred.local",
            port: 12090,
            automatic: false,
        },
        roster: vec![],
    };
    let err = driver.validate(&req).expect_err("should reject");
    assert!(
        matches!(err, wp_core::ValidationError::IdentityFormat),
        "got {err:?}"
    );
}

#[tokio::test]
async fn validate_rejects_function_index_out_of_range() {
    let driver = WiFredDriver::new();
    let req = ProgramRequest {
        identity: "122145",
        wifi: WifiCredentials {
            ssid: "bigfred2",
            psk: None,
        },
        server: ThrottleServer {
            host: "bigfred.local",
            port: 12090,
            automatic: false,
        },
        roster: vec![RosterEntry {
            address: Some(3),
            long_address: Some(false),
            mode: Some("128"),
            direction: Some(0),
            functions: vec![FunctionMapping {
                index: 17,
                value: 0,
            }],
        }],
    };
    let err = driver.validate(&req).expect_err("should reject");
    assert!(
        matches!(
            err,
            wp_core::ValidationError::FunctionIndexOutOfRange { index: 17, max: 16 }
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn validate_rejects_empty_ssid() {
    let driver = WiFredDriver::new();
    let req = ProgramRequest {
        identity: "122145",
        wifi: WifiCredentials {
            ssid: "",
            psk: None,
        },
        server: ThrottleServer {
            host: "bigfred.local",
            port: 12090,
            automatic: false,
        },
        roster: vec![],
    };
    let err = driver.validate(&req).expect_err("should reject");
    assert!(
        matches!(err, wp_core::ValidationError::EmptySsid),
        "got {err:?}"
    );
}
