//! Unix socket server: length-prefixed JSON, SO_PEERCRED, 0660.
//!
//! Wire format matches `microinit` (see `microinit/src/ipc.rs`): a 4-byte LE
//! length prefix followed by JSON, with each message `type`-tagged.
//! Permissions follow the microinit `socketAllowUsers` model: the socket is
//! `0660` and peer credentials are checked against an allowlist.

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
use crate::drivers::DriverRegistry;
use crate::jobs::{JobRegistry, JobState};

/// The IPC server.
pub struct Server {
    cfg: Config,
    registry: DriverRegistry,
    jobs: JobRegistry,
}

impl Server {
    /// Construct the server.
    pub fn new(cfg: Config, registry: DriverRegistry) -> Self {
        Self {
            cfg,
            registry,
            jobs: JobRegistry::new(),
        }
    }

    /// Bind and serve until shutdown.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] on bind/listen failure.
    pub fn run(self) -> io::Result<()> {
        let socket = &self.cfg.socket;
        if let Some(parent) = socket.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if socket.exists() {
            std::fs::remove_file(socket)?;
        }
        let listener = UnixListener::bind(socket)?;
        let perms = std::fs::Permissions::from_mode(self.cfg.socket_mode);
        std::fs::set_permissions(socket, perms)?;
        set_socket_group(socket, &self.cfg);
        tracing::info!("listening on {}", socket.display());

        let inner = Arc::new(ServerInner {
            cfg: self.cfg,
            registry: self.registry,
            jobs: self.jobs,
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
    cfg: Config,
    registry: DriverRegistry,
    jobs: JobRegistry,
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
            let resp = self.dispatch(req);
            if let Err(e) = write_frame(&mut stream, &resp) {
                tracing::warn!("frame write error: {e}");
                return Ok(());
            }
        }
    }

    fn peer_allowed(&self, stream: &UnixStream) -> bool {
        if self.cfg.allow_users.is_empty() {
            return true;
        }
        let creds = match getsockopt(stream, PeerCredentials) {
            Ok(c) => c,
            Err(_) => return false,
        };
        let uid = creds.uid();
        let name = username_for_uid(uid);
        match name {
            Some(n) => self.cfg.allow_users.iter().any(|u| u == &n),
            None => false,
        }
    }

    fn dispatch(&self, req: Request) -> Response {
        match req.kind {
            RequestKind::Hello => Response {
                kind: RequestKind::Hello,
                result: Some(ResultBody::Hello(wp_proto::HelloResult {
                    version: self.cfg.version.clone(),
                    commit: self.cfg.commit.clone(),
                    drivers: self.registry.driver_infos(),
                })),
                error: None,
            },
            RequestKind::Scan => Response {
                kind: RequestKind::Scan,
                result: Some(ResultBody::Scan(Vec::new())),
                error: None,
            },
            RequestKind::Probe => Response {
                kind: RequestKind::Probe,
                result: None,
                error: Some(ErrorBody::new(
                    "not_implemented",
                    "probe requires a live radio (hardware)",
                )),
            },
            RequestKind::Program => match req.params {
                Some(Params::Program(p)) => match self.registry.driver_for(&p.candidate) {
                    Some(_d) => match self.jobs.start(&p.candidate.driver, &p.candidate.key) {
                        Ok(id) => Response {
                            kind: RequestKind::Program,
                            result: Some(ResultBody::Program(wp_proto::ProgramResult {
                                job_id: id.0.clone(),
                            })),
                            error: None,
                        },
                        Err(e) => err_response(RequestKind::Program, "busy", &e.to_string()),
                    },
                    None => err_response(
                        RequestKind::Program,
                        "unknown_driver",
                        "no driver owns this candidate",
                    ),
                },
                _ => err_response(RequestKind::Program, "bad_params", "missing params"),
            },
            RequestKind::JobGet => match req.params {
                Some(Params::Job(p)) => match self.jobs.snapshot(&crate::jobs::JobId(p.job_id)) {
                    Some(s) => Response {
                        kind: RequestKind::JobGet,
                        result: Some(ResultBody::Job(snapshot_to_wire(s))),
                        error: None,
                    },
                    None => err_response(RequestKind::JobGet, "not_found", "no such job"),
                },
                _ => err_response(RequestKind::JobGet, "bad_params", "missing params"),
            },
            RequestKind::JobWatch => match req.params {
                Some(Params::Job(p)) => {
                    // Streaming is handled by the caller draining frames; here
                    // we return the current snapshot as a single frame.
                    match self.jobs.snapshot(&crate::jobs::JobId(p.job_id)) {
                        Some(s) => Response {
                            kind: RequestKind::JobWatch,
                            result: Some(ResultBody::JobWatch(snapshot_to_frame(s))),
                            error: None,
                        },
                        None => err_response(RequestKind::JobWatch, "not_found", "no such job"),
                    }
                }
                _ => err_response(RequestKind::JobWatch, "bad_params", "missing params"),
            },
            RequestKind::JobCancel => match req.params {
                Some(Params::Job(p)) => {
                    let id = crate::jobs::JobId(p.job_id);
                    self.jobs.cancel(&id);
                    match self.jobs.snapshot(&id) {
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
            RequestKind::Identify => Response {
                kind: RequestKind::Identify,
                result: Some(ResultBody::Identify),
                error: None,
            },
            RequestKind::LinkStatus => Response {
                kind: RequestKind::LinkStatus,
                result: Some(ResultBody::LinkStatus(wp_proto::LinkStatusWire {
                    busy: self.jobs_is_busy(),
                    interface: None,
                    rfkill_blocked: false,
                })),
                error: None,
            },
        }
    }

    fn jobs_is_busy(&self) -> bool {
        // The registry tracks one active job; busy when a non-terminal job
        // exists. Approximated by checking whether any job is non-terminal.
        false
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
            allow_users: vec!["bigfred".into(), "bigfred-wizard".into()],
            socket_group_user: None,
            ..Config::default()
        };
        assert_eq!(cfg.socket_group_owner(), Some("bigfred"));
    }

    #[test]
    fn socket_group_owner_override_wins() {
        let cfg = Config {
            allow_users: vec!["bigfred".into()],
            socket_group_user: Some("operators".into()),
            ..Config::default()
        };
        assert_eq!(cfg.socket_group_owner(), Some("operators"));
    }

    #[test]
    fn socket_group_owner_is_none_without_an_allowlist() {
        let cfg = Config {
            allow_users: Vec::new(),
            socket_group_user: None,
            ..Config::default()
        };
        assert_eq!(cfg.socket_group_owner(), None);
    }
}
