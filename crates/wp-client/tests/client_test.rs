//! Integration tests for the Rust client SDK.
//!
//! Each test stands up a throwaway `UnixListener` playing the daemon, so the
//! framing, the request/response pairing, and the error mapping are exercised
//! end to end without needing a radio.

use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;

use wp_client::{Client, ClientError, JobFrame, JobStateWire};
use wp_proto::{
    read_frame, write_frame, ErrorBody, HelloResult, Request, RequestKind, Response, ResultBody,
};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A listening fake daemon that removes its socket on drop.
struct FakeDaemon {
    path: PathBuf,
    handle: Option<thread::JoinHandle<()>>,
}

impl FakeDaemon {
    /// Bind a unique socket and serve exactly one connection with `serve`.
    fn spawn<F>(serve: F) -> Self
    where
        F: FnOnce(UnixStream) + Send + 'static,
    {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("wp-client-{}-{n}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind fake daemon socket");
        let handle = thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                serve(stream);
            }
        });
        Self {
            path,
            handle: Some(handle),
        }
    }

    fn client(&self) -> Client {
        Client::new(&self.path)
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Read one request and reply with `make` applied to its kind.
fn reply_once<F>(make: F) -> impl FnOnce(UnixStream)
where
    F: FnOnce(RequestKind) -> Response + Send + 'static,
{
    move |mut stream| {
        let req: Request = read_frame(&mut stream).expect("read request");
        let resp = make(req.kind);
        write_frame(&mut stream, &resp).expect("write response");
    }
}

fn hello_result() -> HelloResult {
    HelloResult {
        version: "0.1.0".into(),
        commit: Some("deadbee".into()),
        drivers: Vec::new(),
    }
}

fn frame(state: JobStateWire, detail: Option<&str>) -> JobFrame {
    JobFrame {
        job_id: "job-1".into(),
        state,
        step: None,
        progress: None,
        detail: detail.map(Into::into),
    }
}

#[test]
fn hello_round_trips() {
    let daemon = FakeDaemon::spawn(reply_once(|kind| {
        assert_eq!(kind, RequestKind::Hello);
        Response {
            kind: RequestKind::Hello,
            result: Some(ResultBody::Hello(hello_result())),
            error: None,
        }
    }));

    let got = daemon.client().hello().expect("hello");
    assert_eq!(got.version, "0.1.0");
    assert_eq!(got.commit.as_deref(), Some("deadbee"));
}

#[test]
fn scan_decodes_an_empty_candidate_list() {
    let daemon = FakeDaemon::spawn(reply_once(|_| Response {
        kind: RequestKind::Scan,
        result: Some(ResultBody::Scan(Vec::new())),
        error: None,
    }));

    assert!(daemon.client().scan().expect("scan").is_empty());
}

#[test]
fn busy_maps_to_a_typed_error() {
    let daemon = FakeDaemon::spawn(reply_once(|_| Response {
        kind: RequestKind::Program,
        result: None,
        error: Some(ErrorBody::new("busy", "radio held by job-7")),
    }));

    let err = daemon
        .client()
        .program(
            wp_client::CandidateRef {
                driver: "wifred".into(),
                key: "AA:BB".into(),
            },
            wp_client::ProgramRequestWire {
                identity: "122145".into(),
                wifi: wp_client::WifiCredentialsWire {
                    ssid: "bigfred2".into(),
                    psk: None,
                },
                server: wp_client::ThrottleServerWire {
                    host: "bigfred.local".into(),
                    port: 12090,
                    automatic: None,
                },
                roster: Vec::new(),
                bigfred: None,
                roster_mode: None,
            },
        )
        .expect_err("expected busy");

    assert!(err.is_busy(), "{err:?}");
    assert!(err.to_string().contains("radio held by job-7"));
}

#[test]
fn not_found_and_unknown_codes_map_distinctly() {
    let daemon = FakeDaemon::spawn(reply_once(|_| Response {
        kind: RequestKind::JobGet,
        result: None,
        error: Some(ErrorBody::new("notFound", "no such job")),
    }));
    let err = daemon.client().job_get("nope").expect_err("expected error");
    assert!(matches!(err, ClientError::NotFound { .. }), "{err:?}");

    let daemon = FakeDaemon::spawn(reply_once(|_| Response {
        kind: RequestKind::JobGet,
        result: None,
        error: Some(ErrorBody::new("bad_params", "missing params")),
    }));
    let err = daemon.client().job_get("nope").expect_err("expected error");
    match err {
        ClientError::Server { code, message } => {
            assert_eq!(code, "bad_params");
            assert_eq!(message, "missing params");
        }
        other => panic!("expected Server, got {other:?}"),
    }
}

#[test]
fn a_mismatched_response_kind_is_rejected() {
    let daemon = FakeDaemon::spawn(reply_once(|_| Response {
        kind: RequestKind::Scan,
        result: Some(ResultBody::Scan(Vec::new())),
        error: None,
    }));

    let err = daemon.client().hello().expect_err("expected mismatch");
    assert!(matches!(err, ClientError::UnexpectedResponse(_)), "{err:?}");
}

#[test]
fn connect_failure_names_the_socket() {
    let missing = std::env::temp_dir().join("wp-client-does-not-exist.sock");
    let _ = std::fs::remove_file(&missing);
    let err = Client::new(&missing)
        .hello()
        .expect_err("expected connect error");
    match err {
        ClientError::Connect { socket, .. } => {
            assert_eq!(socket, missing.display().to_string());
        }
        other => panic!("expected Connect, got {other:?}"),
    }
}

#[test]
fn watch_observes_every_frame_and_returns_the_terminal_one() {
    let daemon = FakeDaemon::spawn(|mut stream| {
        let _req: Request = read_frame(&mut stream).expect("read watch request");
        for state in [
            JobStateWire::Queued,
            JobStateWire::Joining,
            JobStateWire::Writing,
            JobStateWire::Done,
        ] {
            let resp = Response {
                kind: RequestKind::JobWatch,
                result: Some(ResultBody::JobWatch(frame(state, None))),
                error: None,
            };
            write_frame(&mut stream, &resp).expect("write frame");
        }
    });

    let mut seen = Vec::new();
    let last = daemon
        .client()
        .job_watch("job-1")
        .expect("watch")
        .drain_with(|f| seen.push(f.state))
        .expect("drain");

    assert_eq!(
        seen,
        vec![
            JobStateWire::Queued,
            JobStateWire::Joining,
            JobStateWire::Writing,
            JobStateWire::Done,
        ]
    );
    assert_eq!(last.map(|f| f.state), Some(JobStateWire::Done));
}

#[test]
fn watch_stops_at_a_terminal_failure_and_keeps_the_detail() {
    let daemon = FakeDaemon::spawn(|mut stream| {
        let _req: Request = read_frame(&mut stream).expect("read watch request");
        for f in [
            frame(JobStateWire::Joining, None),
            frame(JobStateWire::Failed, Some("association timed out")),
        ] {
            let resp = Response {
                kind: RequestKind::JobWatch,
                result: Some(ResultBody::JobWatch(f)),
                error: None,
            };
            write_frame(&mut stream, &resp).expect("write frame");
        }
    });

    let last = daemon
        .client()
        .job_watch("job-1")
        .expect("watch")
        .drain()
        .expect("drain")
        .expect("a terminal frame");
    assert_eq!(last.state, JobStateWire::Failed);
    assert_eq!(last.detail.as_deref(), Some("association timed out"));
}

#[test]
fn a_clean_close_before_any_frame_is_not_an_error() {
    let daemon = FakeDaemon::spawn(|mut stream| {
        let _req: Request = read_frame(&mut stream).expect("read watch request");
        // Close without sending anything.
        drop(stream);
    });

    let last = daemon
        .client()
        .job_watch("job-1")
        .expect("watch")
        .drain()
        .expect("clean EOF is Ok(None)");
    assert!(last.is_none());
}

#[test]
fn a_truncated_frame_is_an_error_not_a_clean_close() {
    let daemon = FakeDaemon::spawn(|mut stream| {
        let _req: Request = read_frame(&mut stream).expect("read watch request");
        // Two bytes of a four-byte length header, then hang up.
        stream
            .write_all(&[0x10, 0x00])
            .expect("write partial header");
    });

    let err = daemon
        .client()
        .job_watch("job-1")
        .expect("watch")
        .drain()
        .expect_err("a partial header must not look like an orderly end");
    assert!(matches!(err, ClientError::Frame(_)), "{err:?}");
}
