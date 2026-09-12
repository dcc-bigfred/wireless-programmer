//! End-to-end fake-mode tests: FakeRadio + Soft-AP HTTP mock + Runtime.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use wp_fake::{CompositeFakeDevice, FakeRadio, FakeZ21, FakeZ21Mode};
use wp_proto::{
    ProgramRequestWire, ReachMode, RosterEntryWire, ThrottleServerWire, WifiCredentialsWire,
};

use wireless_programmer::config::Config;
use wireless_programmer::drivers::{Driver, DriverRegistry};
use wireless_programmer::jobs::{FirmwareJob, JobRegistry, JobState};
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

    let mut cfg = Config {
        socket: temp_socket(),
        interface: Some("fake".into()),
        require_auth: false,
        ..Default::default()
    };
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

fn fred_request(addr: u16) -> ProgramRequestWire {
    ProgramRequestWire {
        identity: String::new(),
        wifi: WifiCredentialsWire::default(),
        server: ThrottleServerWire::default(),
        roster: vec![RosterEntryWire {
            address: Some(addr),
            long_address: None,
            mode: None,
            direction: None,
            functions: Vec::new(),
        }],
        bigfred: None,
        roster_mode: None,
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
    assert_eq!(found.len(), 3);
    assert!(found.iter().any(|c| c.driver == "wifred"));
    assert!(found.iter().any(|c| c.driver == "longfred"));
    assert!(found.iter().any(|c| c.driver == "rb23xx"));
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
    assert_eq!(
        state,
        JobState::Done,
        "detail={:?}",
        rt.jobs().snapshot(&id)
    );
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
    assert_eq!(
        state,
        JobState::Done,
        "detail={:?}",
        rt.jobs().snapshot(&id)
    );
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

#[test]
fn fake_identify_wifred() {
    let rt = setup_runtime();
    let found = rt.scan().expect("scan");
    let c = found.iter().find(|c| c.driver == "wifred").expect("wifred");
    rt.identify(Driver::WiFred, &c.key, Some(3))
        .expect("identify");
}

#[test]
fn fake_program_fred_dispatch_reaches_done() {
    let fake = FakeZ21::spawn(FakeZ21Mode::Accept).unwrap();
    let rt = setup_runtime();
    let key = fake.addr().to_string();
    let id = rt
        .submit_program(Driver::Fred, &key, fred_request(42))
        .expect("submit");
    let state = wait_terminal(&rt, &id);
    let snap = rt.jobs().snapshot(&id);
    assert_eq!(state, JobState::Done, "detail={snap:?}");
    assert_eq!(
        snap.as_ref().and_then(|s| s.detail.as_deref()),
        Some("slot 3")
    );
    assert!(fake.dispatch_count() >= 1);
}

#[test]
fn fake_program_fred_reject_fails() {
    let fake = FakeZ21::spawn(FakeZ21Mode::Reject).unwrap();
    let rt = setup_runtime();
    let key = fake.addr().to_string();
    let id = rt
        .submit_program(Driver::Fred, &key, fred_request(7))
        .expect("submit");
    let state = wait_terminal(&rt, &id);
    let snap = rt.jobs().snapshot(&id);
    assert_eq!(state, JobState::Failed, "detail={snap:?}");
    assert_eq!(
        snap.as_ref().and_then(|s| s.detail.as_deref()),
        Some("dispatchFailed")
    );
}

#[test]
fn fake_program_fred_unknown_command_fails() {
    let fake = FakeZ21::spawn(FakeZ21Mode::UnknownCommand).unwrap();
    let rt = setup_runtime();
    let key = fake.addr().to_string();
    let id = rt
        .submit_program(Driver::Fred, &key, fred_request(9))
        .expect("submit");
    let state = wait_terminal(&rt, &id);
    let snap = rt.jobs().snapshot(&id);
    assert_eq!(state, JobState::Failed, "detail={snap:?}");
    assert_eq!(
        snap.as_ref().and_then(|s| s.detail.as_deref()),
        Some("z21NoLocoNet")
    );
}

#[test]
fn fake_program_fred_no_ack_reaches_done() {
    let fake = FakeZ21::spawn(FakeZ21Mode::NoAck).unwrap();
    let rt = setup_runtime();
    let key = fake.addr().to_string();
    let id = rt
        .submit_program(Driver::Fred, &key, fred_request(99))
        .expect("submit");
    let state = wait_terminal(&rt, &id);
    let snap = rt.jobs().snapshot(&id);
    assert_eq!(state, JobState::Done, "detail={snap:?}");
    assert_eq!(
        snap.as_ref().and_then(|s| s.detail.as_deref()),
        Some("noAck")
    );
}

#[test]
fn fake_probe_rb23xx() {
    let rt = setup_runtime();
    let found = rt.scan().expect("scan");
    let c = found.iter().find(|c| c.driver == "rb23xx").expect("rb23xx");
    let info = rt.probe(Driver::Rb23xx, &c.key).expect("probe");
    assert_eq!(info.get("ok").and_then(|v| v.as_bool()), Some(true));
}

#[test]
fn fake_update_firmware_rb23xx_reaches_done() {
    let rt = setup_runtime();
    let found = rt.scan().expect("scan");
    let c = found.iter().find(|c| c.driver == "rb23xx").expect("rb23xx");
    let path = std::env::temp_dir().join(format!(
        "wp-rb23xx-fw-{}-{}.bin",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, b"rb-firmware-bytes").unwrap();
    let id = rt
        .submit_firmware(
            Driver::Rb23xx,
            &c.key,
            FirmwareJob {
                mode: ReachMode::Ap,
                path: path.clone(),
                host: None,
                port: None,
                partition_table: None,
            },
        )
        .expect("submit");
    let state = wait_terminal(&rt, &id);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        state,
        JobState::Done,
        "detail={:?}",
        rt.jobs().snapshot(&id)
    );
}

#[test]
fn fake_update_firmware_rb23xx_rejects_oversize() {
    let rt = setup_runtime();
    let found = rt.scan().expect("scan");
    let c = found.iter().find(|c| c.driver == "rb23xx").expect("rb23xx");
    let path = std::env::temp_dir().join(format!(
        "wp-rb23xx-big-{}-{}.bin",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let oversize = usize::try_from(wp_drivers::rb23xx::MAX_FIRMWARE_BYTES).unwrap() + 1;
    std::fs::write(&path, vec![0u8; oversize]).unwrap();
    let id = rt
        .submit_firmware(
            Driver::Rb23xx,
            &c.key,
            FirmwareJob {
                mode: ReachMode::Ap,
                path: path.clone(),
                host: None,
                port: None,
                partition_table: None,
            },
        )
        .expect("submit");
    let state = wait_terminal(&rt, &id);
    let snap = rt.jobs().snapshot(&id);
    let _ = std::fs::remove_file(&path);
    assert_eq!(state, JobState::Failed, "detail={snap:?}");
    assert!(
        snap.as_ref()
            .and_then(|s| s.detail.as_deref())
            .is_some_and(|d| d.contains("5 MiB")),
        "{snap:?}"
    );
}

#[test]
fn fake_update_firmware_rb23xx_rejects_lan() {
    let rt = setup_runtime();
    let found = rt.scan().expect("scan");
    let c = found.iter().find(|c| c.driver == "rb23xx").expect("rb23xx");
    let path = std::env::temp_dir().join(format!(
        "wp-rb23xx-lan-{}-{}.bin",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, b"rb-firmware-bytes").unwrap();
    let id = rt
        .submit_firmware(
            Driver::Rb23xx,
            &c.key,
            FirmwareJob {
                mode: ReachMode::Lan,
                path: path.clone(),
                host: Some("192.168.4.1".into()),
                port: None,
                partition_table: None,
            },
        )
        .expect("submit");
    let state = wait_terminal(&rt, &id);
    let snap = rt.jobs().snapshot(&id);
    let _ = std::fs::remove_file(&path);
    assert_eq!(state, JobState::Failed, "detail={snap:?}");
    assert!(
        snap.as_ref()
            .and_then(|s| s.detail.as_deref())
            .is_some_and(|d| d.contains("Soft-AP")),
        "{snap:?}"
    );
}
