//! End-to-end fake-mode tests: FakeRadio + Soft-AP HTTP mock + Runtime.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use wp_fake::{CompositeFakeDevice, FakeRadio};
use wp_proto::{
    ProgramRequestWire, RosterEntryWire, ThrottleServerWire, WifiCredentialsWire,
};

use wireless_programmer::config::Config;
use wireless_programmer::drivers::{Driver, DriverRegistry};
use wireless_programmer::jobs::{JobRegistry, JobState};
use wireless_programmer::runtime::Runtime;

fn temp_socket() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "wp-fake-test-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

fn setup_runtime() -> Arc<Runtime> {
    let bind = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let bootstrap = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let device = Arc::new(tokio::sync::Mutex::new(CompositeFakeDevice::all()));
    let local = bootstrap.block_on(async {
        let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
        let local = listener.local_addr().unwrap();
        let device = Arc::clone(&device);
        tokio::spawn(async move {
            let _ = wp_fake::FakeHttpServer::serve(listener, device).await;
        });
        local
    });
    // Keep the accept loop alive for the duration of the test process.
    std::mem::forget(bootstrap);

    let mut cfg = Config::default();
    cfg.socket = temp_socket();
    cfg.interface = Some("fake".into());
    cfg.require_auth = false;
    cfg.finalize_auth();
    cfg.commissioning_net_override = Some(Config::localhost_commissioning(local.port()));

    let radio = Box::new(FakeRadio::one_per_driver());
    Runtime::new(cfg, DriverRegistry::new(), JobRegistry::new(), radio).expect("runtime")
}

fn wifred_request() -> ProgramRequestWire {
    ProgramRequestWire {
        identity: "122145".into(),
        wifi: WifiCredentialsWire {
            ssid: "club-wifi".into(),
            psk: Some("secret".into()),
        },
        server: ThrottleServerWire {
            host: "bigfred.local".into(),
            port: 12090,
            automatic: Some(false),
        },
        roster: vec![RosterEntryWire {
            address: Some(3),
            long_address: Some(false),
            mode: Some("128".into()),
            direction: Some(0),
            functions: Vec::new(),
        }],
        bigfred: None,
        roster_mode: None,
    }
}

fn longfred_request() -> ProgramRequestWire {
    ProgramRequestWire {
        identity: "pilot1".into(),
        wifi: WifiCredentialsWire {
            ssid: "club-wifi".into(),
            psk: Some("secret".into()),
        },
        server: ThrottleServerWire {
            host: "unused.local".into(),
            port: 12090,
            automatic: Some(false),
        },
        roster: vec![RosterEntryWire {
            address: Some(3),
            long_address: Some(false),
            mode: None,
            direction: None,
            functions: Vec::new(),
        }],
        bigfred: Some(wp_proto::BigfredCredsWire {
            login: "ops".into(),
            pin: "1234".into(),
        }),
        roster_mode: Some("static".into()),
    }
}

fn wait_terminal(rt: &Runtime, id: &wireless_programmer::jobs::JobId) -> JobState {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(s) = rt.jobs().snapshot(id) {
            if s.state.is_terminal() {
                return s.state;
            }
        }
        if std::time::Instant::now() > deadline {
            panic!("job did not reach terminal state");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn fake_scan_returns_one_candidate_per_driver() {
    let rt = setup_runtime();
    let found = rt.scan().expect("scan");
    assert_eq!(found.len(), 2);
    assert!(found.iter().any(|c| c.driver == "wifred"));
    assert!(found.iter().any(|c| c.driver == "longfred"));
}

#[test]
fn fake_program_wifred_reaches_done() {
    let rt = setup_runtime();
    let found = rt.scan().expect("scan");
    let c = found.iter().find(|c| c.driver == "wifred").expect("wifred");
    let id = rt
        .submit_program(Driver::WiFred, &c.key, wifred_request())
        .expect("submit");
    let state = wait_terminal(&rt, &id);
    assert_eq!(state, JobState::Done, "detail={:?}", rt.jobs().snapshot(&id));
}

#[test]
fn fake_program_longfred_reaches_done() {
    let rt = setup_runtime();
    let found = rt.scan().expect("scan");
    let c = found
        .iter()
        .find(|c| c.driver == "longfred")
        .expect("longfred");
    let id = rt
        .submit_program(Driver::LongFred, &c.key, longfred_request())
        .expect("submit");
    let state = wait_terminal(&rt, &id);
    assert_eq!(state, JobState::Done, "detail={:?}", rt.jobs().snapshot(&id));
}

#[test]
fn fake_probe_wifred() {
    let rt = setup_runtime();
    let found = rt.scan().expect("scan");
    let c = found.iter().find(|c| c.driver == "wifred").expect("wifred");
    let info = rt.probe(Driver::WiFred, &c.key).expect("probe");
    assert_eq!(
        info.get("structureVersion").and_then(|v| v.as_str()),
        Some("1")
    );
}
