//! Unix socket server: length-prefixed JSON, optional SO_PEERCRED, 0660/0666.
//!
//! Wire format matches `microinit` (see `microinit/src/ipc.rs`): a 4-byte LE
//! length prefix followed by JSON, with each message `type`-tagged.
//! Peer authentication is **off by default**. When enabled (`--require-auth`
//! / `WIRELESS_PROGRAMMER_REQUIRE_AUTH`), the socket is `0660` and peers are
//! checked against an allowlist via `SO_PEERCRED`.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;

use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
use nix::unistd::{chown, Gid};
use wp_proto::{
    read_frame, write_frame, ErrorBody, Params, Request, RequestKind, Response, ResultBody,
};

use crate::config::Config;
use crate::jobs::JobState;
use crate::runtime::Runtime;

/// The IPC server.
pub struct Server {
    runtime: Arc<Runtime>,
}

impl Server {
    /// Construct the server around a shared [`Runtime`].
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self { runtime }
    }

    /// Bind and serve until shutdown.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] on bind/listen failure.
    pub fn run(self) -> io::Result<()> {
        let socket = self.runtime.config().socket.clone();
        if let Some(parent) = socket.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if socket.exists() {
            std::fs::remove_file(&socket)?;
        }
        let listener = UnixListener::bind(&socket)?;
        let perms = std::fs::Permissions::from_mode(self.runtime.config().socket_mode);
        std::fs::set_permissions(&socket, perms)?;
        set_socket_group(&socket, self.runtime.config());
        tracing::info!("listening on {}", socket.display());

        let inner = Arc::new(ServerInner {
            runtime: self.runtime,
        });

        for stream in listener.incoming() {
            let stream = stream?;
            let inner = Arc::clone(&inner);
            std::thread::spawn(move || {
                if let Err(e) = inner.handle_conn(stream) {
                    tracing::warn!("connection error: {e}");
                }
            });
        }
        Ok(())
    }
}

struct ServerInner {
    runtime: Arc<Runtime>,
}

impl ServerInner {
    fn handle_conn(&self, mut stream: UnixStream) -> io::Result<()> {
        if !self.peer_allowed(&stream) {
            let _ = write_frame(
                &mut stream,
                &err_response(RequestKind::Hello, "forbidden", "peer not allowed"),
            );
            return Ok(());
        }
        loop {
            let req: Request = match read_frame(&mut stream) {
                Ok(r) => r,
                Err(wp_proto::FrameError::UnexpectedEof { .. }) => return Ok(()),
                Err(e) => {
                    tracing::warn!("frame read error: {e}");
                    return Ok(());
                }
            };
            // JobWatch streams many frames on one connection until terminal.
            if req.kind == RequestKind::JobWatch {
                if let Err(e) = self.stream_job_watch(&mut stream, req) {
                    tracing::warn!("job.watch stream error: {e}");
                }
                return Ok(());
            }
            let resp = self.dispatch(req);
            if let Err(e) = write_frame(&mut stream, &resp) {
                tracing::warn!("frame write error: {e}");
                return Ok(());
            }
        }
    }

    fn peer_allowed(&self, stream: &UnixStream) -> bool {
        let cfg = self.runtime.config();
        if !cfg.require_auth {
            return true;
        }
        if cfg.allow_users.is_empty() {
            // Auth on with an empty list should never happen after
            // finalize_auth, but fail closed.
            return false;
        }
        let creds = match getsockopt(stream, PeerCredentials) {
            Ok(c) => c,
            Err(_) => return false,
        };
        let uid = creds.uid();
        let name = username_for_uid(uid);
        match name {
            Some(n) => cfg.allow_users.iter().any(|u| u == &n),
            None => false,
        }
    }

    fn stream_job_watch(&self, stream: &mut UnixStream, req: Request) -> io::Result<()> {
        let write = |stream: &mut UnixStream, resp: &Response| {
            write_frame(stream, resp).map_err(|e| io::Error::other(e.to_string()))
        };
        let job_id = match req.params {
            Some(Params::Job(p)) => crate::jobs::JobId(p.job_id),
            _ => {
                write(
                    stream,
                    &err_response(RequestKind::JobWatch, "bad_params", "missing params"),
                )?;
                return Ok(());
            }
        };
        if self.runtime.jobs().snapshot(&job_id).is_none() {
            write(
                stream,
                &err_response(RequestKind::JobWatch, "not_found", "no such job"),
            )?;
            return Ok(());
        }
        let mut since = 0usize;
        let mut sent_snapshot = false;
        loop {
            let Some(frames) = self.runtime.jobs().frames_since(&job_id, since) else {
                write(
                    stream,
                    &err_response(RequestKind::JobWatch, "not_found", "no such job"),
                )?;
                return Ok(());
            };
            let mut terminal = false;
            for f in &frames {
                let wire = job_frame_to_wire(f);
                terminal = wire.state.is_terminal();
                write(
                    stream,
                    &Response {
                        kind: RequestKind::JobWatch,
                        result: Some(ResultBody::JobWatch(wire)),
                        error: None,
                    },
                )?;
            }
            since += frames.len();
            if terminal {
                return Ok(());
            }
            // Emit a snapshot once so the client sees Queued before any
            // transition frames exist. Do not bump `since` — that would skip
            // the first real frame (often the only Failed/Cancelled frame).
            if since == 0 && !sent_snapshot {
                if let Some(s) = self.runtime.jobs().snapshot(&job_id) {
                    let wire = snapshot_to_frame(s);
                    let terminal = wire.state.is_terminal();
                    write(
                        stream,
                        &Response {
                            kind: RequestKind::JobWatch,
                            result: Some(ResultBody::JobWatch(wire)),
                            error: None,
                        },
                    )?;
                    sent_snapshot = true;
                    if terminal {
                        return Ok(());
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    fn dispatch(&self, req: Request) -> Response {
        match req.kind {
            RequestKind::Hello => Response {
                kind: RequestKind::Hello,
                result: Some(ResultBody::Hello(wp_proto::HelloResult {
                    version: self.runtime.config().version.clone(),
                    commit: self.runtime.config().commit.clone(),
                    drivers: self.runtime.registry().driver_infos(),
                })),
                error: None,
            },
            RequestKind::Scan => {
                let mode = match req.params {
                    Some(Params::Scan(ref p)) => p.mode,
                    _ => wp_proto::ReachMode::Ap,
                };
                if mode == wp_proto::ReachMode::Ap && self.runtime.radio_held() {
                    return err_response(
                        RequestKind::Scan,
                        "busy",
                        "radio in use by a programming job",
                    );
                }
                tracing::info!(?mode, "scan started");
                let scanned = match mode {
                    wp_proto::ReachMode::Lan => self.runtime.scan_lan(),
                    wp_proto::ReachMode::Usb => self.runtime.scan_usb(),
                    wp_proto::ReachMode::Ap => self.runtime.scan(),
                };
                match scanned {
                    Ok(found) => {
                        let candidates: Vec<wp_proto::CandidateWire> = found
                            .iter()
                            .map(|c| wp_proto::CandidateWire {
                                driver: c.driver.clone(),
                                key: c.key.clone(),
                                label: c.label.clone(),
                                rssi: c.rssi,
                            })
                            .collect();
                        if candidates.is_empty() {
                            tracing::info!("scan finished: no handsets found");
                        } else {
                            let names: Vec<&str> =
                                candidates.iter().map(|c| c.label.as_str()).collect();
                            tracing::info!(
                                count = candidates.len(),
                                ?names,
                                "scan finished: found handsets"
                            );
                            for c in &candidates {
                                tracing::info!(
                                    driver = %c.driver,
                                    key = %c.key,
                                    label = %c.label,
                                    rssi = ?c.rssi,
                                    "scan candidate"
                                );
                            }
                        }
                        Response {
                            kind: RequestKind::Scan,
                            result: Some(ResultBody::Scan(candidates)),
                            error: None,
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "scan failed");
                        err_response(RequestKind::Scan, "scan_failed", &e.to_string())
                    }
                }
            }
            RequestKind::Probe => match req.params {
                Some(Params::Probe(p)) => {
                    if self.runtime.radio_held() {
                        err_response(
                            RequestKind::Probe,
                            "busy",
                            "radio in use by a programming job",
                        )
                    } else {
                        match self.runtime.registry().driver_for(&p.candidate) {
                            Some(d) => match self.runtime.probe(d, &p.candidate.key) {
                                Ok(info) => Response {
                                    kind: RequestKind::Probe,
                                    result: Some(ResultBody::Probe(device_info_from_probe(
                                        d.id_str(),
                                        &p.candidate.key,
                                        &info,
                                    ))),
                                    error: None,
                                },
                                Err(e) => {
                                    err_response(RequestKind::Probe, "probe_failed", &e.to_string())
                                }
                            },
                            None => err_response(
                                RequestKind::Probe,
                                "unknown_driver",
                                "no driver owns this candidate",
                            ),
                        }
                    }
                }
                _ => err_response(RequestKind::Probe, "bad_params", "missing params"),
            },
            RequestKind::Program => match req.params {
                Some(Params::Program(p)) => {
                    let roster_addrs: Vec<u16> =
                        p.request.roster.iter().filter_map(|e| e.address).collect();
                    tracing::info!(
                        driver = %p.candidate.driver,
                        key = %p.candidate.key,
                        identity = %p.request.identity,
                        wifi_ssid = %p.request.wifi.ssid,
                        server = %format!("{}:{}", p.request.server.host, p.request.server.port),
                        automatic = ?p.request.server.automatic,
                        roster = ?roster_addrs,
                        bigfred_login = ?p.request.bigfred.as_ref().map(|b| b.login.as_str()),
                        roster_mode = ?p.request.roster_mode,
                        "program request received"
                    );
                    match self.runtime.registry().driver_for(&p.candidate) {
                        Some(d) => {
                            match self.runtime.submit_program(d, &p.candidate.key, p.request) {
                                Ok(id) => {
                                    tracing::info!(
                                        job_id = %id.0,
                                        driver = %p.candidate.driver,
                                        key = %p.candidate.key,
                                        "program job queued"
                                    );
                                    Response {
                                        kind: RequestKind::Program,
                                        result: Some(ResultBody::Program(
                                            wp_proto::ProgramResult {
                                                job_id: id.0.clone(),
                                            },
                                        )),
                                        error: None,
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        driver = %p.candidate.driver,
                                        key = %p.candidate.key,
                                        error = %e,
                                        "program rejected"
                                    );
                                    let code = match &e {
                                        crate::jobs::JobError::Busy(_) => "busy",
                                        crate::jobs::JobError::Validation(_) => "validation",
                                        _ => "program_failed",
                                    };
                                    err_response(RequestKind::Program, code, &e.to_string())
                                }
                            }
                        }
                        None => {
                            tracing::warn!(
                                driver = %p.candidate.driver,
                                "program rejected: unknown driver"
                            );
                            err_response(
                                RequestKind::Program,
                                "unknown_driver",
                                "no driver owns this candidate",
                            )
                        }
                    }
                }
                _ => {
                    tracing::warn!("program rejected: missing params");
                    err_response(RequestKind::Program, "bad_params", "missing params")
                }
            },
            RequestKind::JobGet => match req.params {
                Some(Params::Job(p)) => {
                    match self.runtime.jobs().snapshot(&crate::jobs::JobId(p.job_id)) {
                        Some(s) => Response {
                            kind: RequestKind::JobGet,
                            result: Some(ResultBody::Job(snapshot_to_wire(s))),
                            error: None,
                        },
                        None => err_response(RequestKind::JobGet, "not_found", "no such job"),
                    }
                }
                _ => err_response(RequestKind::JobGet, "bad_params", "missing params"),
            },
            RequestKind::JobWatch => {
                // Handled in handle_conn via stream_job_watch.
                err_response(RequestKind::JobWatch, "internal", "job.watch must stream")
            }
            RequestKind::JobCancel => match req.params {
                Some(Params::Job(p)) => {
                    let id = crate::jobs::JobId(p.job_id);
                    self.runtime.jobs().cancel(&id);
                    match self.runtime.jobs().snapshot(&id) {
                        Some(s) => Response {
                            kind: RequestKind::JobCancel,
                            result: Some(ResultBody::JobCancelled(snapshot_to_wire(s))),
                            error: None,
                        },
                        None => err_response(RequestKind::JobCancel, "not_found", "no such job"),
                    }
                }
                _ => err_response(RequestKind::JobCancel, "bad_params", "missing params"),
            },
            RequestKind::Identify => match req.params {
                Some(Params::Identify(p)) => {
                    if self.runtime.radio_held() {
                        err_response(
                            RequestKind::Identify,
                            "busy",
                            "radio in use by a programming job",
                        )
                    } else {
                        match self.runtime.registry().driver_for(&p.candidate) {
                            Some(d) => match self.runtime.identify(d, &p.candidate.key, p.count) {
                                Ok(()) => Response {
                                    kind: RequestKind::Identify,
                                    result: Some(ResultBody::Identify),
                                    error: None,
                                },
                                Err(e) => err_response(
                                    RequestKind::Identify,
                                    "driverError",
                                    &e.to_string(),
                                ),
                            },
                            None => err_response(
                                RequestKind::Identify,
                                "unknown_driver",
                                "no driver owns this candidate",
                            ),
                        }
                    }
                }
                _ => err_response(RequestKind::Identify, "bad_params", "missing params"),
            },
            RequestKind::LinkStatus => {
                let cfg = self.runtime.config();
                let rfkill_blocked = wp_link::rfkill::aggregate_state()
                    .ok()
                    .flatten()
                    .map(|s| s.blocked())
                    .unwrap_or(false);
                Response {
                    kind: RequestKind::LinkStatus,
                    result: Some(ResultBody::LinkStatus(wp_proto::LinkStatusWire {
                        busy: self.runtime.jobs().is_busy(),
                        interface: cfg.interface.clone().or_else(|| {
                            if cfg.is_fake_radio() {
                                Some("fake".into())
                            } else {
                                wp_link::first_wireless_interface().ok()
                            }
                        }),
                        rfkill_blocked,
                    })),
                    error: None,
                }
            }
            RequestKind::UpdateFirmware => match req.params {
                Some(Params::UpdateFirmware(p)) => {
                    let driver_id = p
                        .candidate
                        .as_ref()
                        .map(|c| c.driver.clone())
                        .unwrap_or_else(|| "longfred".into());
                    let key = p
                        .port
                        .clone()
                        .or_else(|| p.host.clone())
                        .or_else(|| p.candidate.as_ref().map(|c| c.key.clone()))
                        .unwrap_or_default();
                    if key.is_empty() && p.mode != wp_proto::ReachMode::Usb {
                        return err_response(
                            RequestKind::UpdateFirmware,
                            "bad_params",
                            "candidate.key, host, or port is required",
                        );
                    }
                    if p.mode == wp_proto::ReachMode::Lan {
                        if let Some(h) = p.host.as_deref() {
                            self.runtime.cache_lan_host(h, None);
                        }
                    }
                    let key = if key.is_empty() && p.mode == wp_proto::ReachMode::Usb {
                        match self.runtime.scan_usb() {
                            Ok(found) if found.len() == 1 => found[0].key.clone(),
                            Ok(found) if found.is_empty() => {
                                return err_response(
                                    RequestKind::UpdateFirmware,
                                    "noCandidates",
                                    "no USB serial ports; pass --port",
                                );
                            }
                            Ok(_) => {
                                return err_response(
                                    RequestKind::UpdateFirmware,
                                    "bad_params",
                                    "multiple USB ports; pass --port",
                                );
                            }
                            Err(e) => {
                                return err_response(
                                    RequestKind::UpdateFirmware,
                                    "scan_failed",
                                    &e.to_string(),
                                );
                            }
                        }
                    } else {
                        key
                    };
                    if p.mode == wp_proto::ReachMode::Usb {
                        self.runtime.cache_usb_port(&key, None);
                    }
                    match crate::drivers::Driver::from_id(&driver_id) {
                        Some(d) => {
                            match self.runtime.submit_firmware(
                                d,
                                &key,
                                crate::jobs::FirmwareJob {
                                    mode: p.mode,
                                    path: std::path::PathBuf::from(&p.path),
                                    host: p.host,
                                    port: p.port.or_else(|| {
                                        (p.mode == wp_proto::ReachMode::Usb).then(|| key.clone())
                                    }),
                                    partition_table: p
                                        .partition_table
                                        .map(std::path::PathBuf::from),
                                },
                            ) {
                                Ok(id) => Response {
                                    kind: RequestKind::UpdateFirmware,
                                    result: Some(ResultBody::UpdateFirmware(
                                        wp_proto::ProgramResult {
                                            job_id: id.0.clone(),
                                        },
                                    )),
                                    error: None,
                                },
                                Err(e) => {
                                    let code = match &e {
                                        crate::jobs::JobError::Busy(_) => "busy",
                                        crate::jobs::JobError::FirmwareUnsupported => "driverError",
                                        _ => "firmware_failed",
                                    };
                                    err_response(RequestKind::UpdateFirmware, code, &e.to_string())
                                }
                            }
                        }
                        None => err_response(
                            RequestKind::UpdateFirmware,
                            "unknown_driver",
                            "no driver owns this candidate",
                        ),
                    }
                }
                _ => err_response(RequestKind::UpdateFirmware, "bad_params", "missing params"),
            },
        }
    }
}

/// Give the socket a group owner so allowlisted peers can actually open it.
///
/// A `0660` socket left owned by `root:root` is unreachable for every non-root
/// peer: `connect(2)` fails with `EACCES` long before `SO_PEERCRED` is
/// consulted, which would make [`ServerInner::peer_allowed`] dead code. This
/// mirrors microinit, which chowns its control socket to the primary group of
/// the first `socketAllowUsers` entry.
///
/// Best-effort by design: on a development machine the BigFred users do not
/// exist and a non-root daemon cannot chown, so the socket stays owner-only.
/// Both cases warn rather than abort, because the daemon is still usable by
/// the user running it.
fn set_socket_group(socket: &Path, cfg: &Config) {
    let Some(owner) = cfg.socket_group_owner() else {
        return;
    };
    let Some(gid) = primary_gid_for_user(owner) else {
        tracing::warn!(
            "socket group owner {owner:?} not found in /etc/passwd; \
             {} stays owner-only and allowlisted peers will get EACCES",
            socket.display()
        );
        return;
    };
    match chown(socket, None, Some(Gid::from_raw(gid))) {
        Ok(()) => tracing::info!("socket group: gid {gid} (primary group of {owner:?})"),
        Err(e) => tracing::warn!(
            "chown {} to gid {gid} failed: {e}; allowlisted peers will get EACCES",
            socket.display()
        ),
    }
}

/// One `/etc/passwd` record.
struct PasswdEntry {
    name: String,
    uid: u32,
    gid: u32,
}

/// Parse `/etc/passwd` content, skipping malformed lines rather than aborting
/// (a single bad record must not lock every peer out).
fn parse_passwd(content: &str) -> impl Iterator<Item = PasswdEntry> + '_ {
    content.lines().filter_map(|line| {
        let mut parts = line.split(':');
        let name = parts.next()?;
        let _pw = parts.next()?;
        let uid = parts.next()?.parse().ok()?;
        let gid = parts.next()?.parse().ok()?;
        Some(PasswdEntry {
            name: name.to_string(),
            uid,
            gid,
        })
    })
}

/// Resolve a uid to a username via `/etc/passwd`.
fn username_for_uid(uid: u32) -> Option<String> {
    let content = std::fs::read_to_string("/etc/passwd").ok()?;
    let found = parse_passwd(&content)
        .find(|e| e.uid == uid)
        .map(|e| e.name);
    found
}

/// Resolve a login name's primary gid via `/etc/passwd`.
fn primary_gid_for_user(name: &str) -> Option<u32> {
    let content = std::fs::read_to_string("/etc/passwd").ok()?;
    let found = parse_passwd(&content)
        .find(|e| e.name == name)
        .map(|e| e.gid);
    found
}

fn device_info_from_probe(
    driver: &str,
    key: &str,
    info: &serde_json::Value,
) -> wp_proto::DeviceInfoWire {
    let identity = info
        .get("throttleName")
        .and_then(|v| v.as_str())
        .or_else(|| info.pointer("/wifi/hostname").and_then(|v| v.as_str()))
        .map(str::to_string);
    let firmware_revision = info
        .get("firmwareRevision")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let battery_mv = info
        .get("batteryMv")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    wp_proto::DeviceInfoWire {
        driver: driver.into(),
        key: key.into(),
        firmware_revision,
        identity,
        battery_mv,
        roster: Vec::new(),
    }
}

fn snapshot_to_wire(s: crate::jobs::JobSnapshot) -> wp_proto::JobSnapshot {
    wp_proto::JobSnapshot {
        job_id: s.id.0.clone(),
        state: state_to_wire(s.state),
        driver: s.driver,
        key: s.key,
        detail: s.detail,
    }
}

fn snapshot_to_frame(s: crate::jobs::JobSnapshot) -> wp_proto::JobFrame {
    wp_proto::JobFrame {
        job_id: s.id.0.clone(),
        state: state_to_wire(s.state),
        step: None,
        progress: None,
        detail: s.detail,
    }
}

fn job_frame_to_wire(f: &crate::jobs::JobFrame) -> wp_proto::JobFrame {
    wp_proto::JobFrame {
        job_id: f.id.0.clone(),
        state: state_to_wire(f.state),
        step: f.step.clone(),
        progress: f.progress,
        detail: f.detail.clone(),
    }
}

fn state_to_wire(s: JobState) -> wp_proto::JobStateWire {
    match s {
        JobState::Queued => wp_proto::JobStateWire::Queued,
        JobState::Joining => wp_proto::JobStateWire::Joining,
        JobState::Probing => wp_proto::JobStateWire::Probing,
        JobState::Writing => wp_proto::JobStateWire::Writing,
        JobState::Verifying => wp_proto::JobStateWire::Verifying,
        JobState::Restarting => wp_proto::JobStateWire::Restarting,
        JobState::Done => wp_proto::JobStateWire::Done,
        JobState::Failed => wp_proto::JobStateWire::Failed,
        JobState::Cancelled => wp_proto::JobStateWire::Cancelled,
    }
}

fn err_response(kind: RequestKind, code: &str, message: &str) -> Response {
    Response {
        kind,
        result: None,
        error: Some(ErrorBody::new(code, message)),
    }
}

/// Remove the socket file on shutdown (best-effort).
pub fn cleanup(socket: &Path) {
    let _ = std::fs::remove_file(socket);
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "\
root:x:0:0:root:/root:/bin/sh
daemon:x:1:1:daemon:/usr/sbin:/bin/false
bigfred:x:1000:1001:BigFred loco-server:/home/bigfred:/bin/false
";

    #[test]
    fn parses_name_uid_and_primary_gid() {
        let entries: Vec<_> = parse_passwd(PASSWD).collect();
        assert_eq!(entries.len(), 3);
        let bigfred = entries.iter().find(|e| e.name == "bigfred").unwrap();
        assert_eq!(bigfred.uid, 1000);
        assert_eq!(bigfred.gid, 1001);
    }

    #[test]
    fn skips_malformed_lines_instead_of_aborting() {
        let content = "broken\nroot:x:0:0:root:/root:/bin/sh\nalso:bad\n";
        let names: Vec<_> = parse_passwd(content).map(|e| e.name).collect();
        assert_eq!(names, vec!["root".to_string()]);
    }

    #[test]
    fn socket_group_owner_defaults_to_first_allowlist_entry() {
        let cfg = Config {
            require_auth: true,
            allow_users: vec!["bigfred".into(), "bigfred-wizard".into()],
            socket_group_user: None,
            ..Config::default()
        };
        assert_eq!(cfg.socket_group_owner(), Some("bigfred"));
    }

    #[test]
    fn socket_group_owner_override_wins() {
        let cfg = Config {
            require_auth: true,
            allow_users: vec!["bigfred".into()],
            socket_group_user: Some("operators".into()),
            ..Config::default()
        };
        assert_eq!(cfg.socket_group_owner(), Some("operators"));
    }

    #[test]
    fn socket_group_owner_is_none_without_an_allowlist() {
        let cfg = Config {
            require_auth: true,
            allow_users: Vec::new(),
            socket_group_user: None,
            ..Config::default()
        };
        assert_eq!(cfg.socket_group_owner(), None);
    }

    #[test]
    fn socket_group_owner_is_none_when_auth_disabled() {
        let cfg = Config {
            require_auth: false,
            allow_users: vec!["bigfred".into()],
            socket_group_user: None,
            ..Config::default()
        };
        assert_eq!(cfg.socket_group_owner(), None);
    }
}
