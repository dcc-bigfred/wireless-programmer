//! Job registry and state machine.
//!
//! The radio is an exclusive resource: at most one programming job runs at a
//! time. A second `program` returns [`JobError::Busy`]. Every exit path
//! releases the radio.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use wp_core::DriverError;
use wp_proto::{ProgramRequestWire, ReachMode};

/// Overall job deadline.
pub const JOB_DEADLINE: Duration = Duration::from_secs(120);

/// Firmware POST deadline (matches LongFred HTTP timeout).
pub const FIRMWARE_DEADLINE: Duration = Duration::from_secs(120);

/// LongFred OTA slot (`ota_0` / `ota_1`) — cap for images loaded into RAM.
pub const MAX_FIRMWARE_BYTES: u64 = 0x3C_0000;

/// Keep at most this many terminal jobs in the registry.
const MAX_JOB_HISTORY: usize = 32;

/// How often firmware jobs emit a `job.watch` frame while blocked in
/// `espflash` or an HTTP POST. Must stay well under the client idle default
/// (10 s) so Go and CLI watchers do not drop the stream.
pub const WATCH_HEARTBEAT: Duration = Duration::from_secs(3);

/// A job identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JobId(pub String);

/// Job lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    /// Waiting for the radio lock.
    Queued,
    /// Associating to the device.
    Joining,
    /// Reading device info.
    Probing,
    /// Writing configuration.
    Writing,
    /// Reading back to verify.
    Verifying,
    /// Asking the device to restart.
    Restarting,
    /// Finished successfully.
    Done,
    /// Failed; see `detail`.
    Failed,
    /// Cancelled by the caller.
    Cancelled,
}

impl JobState {
    /// Whether this state is terminal.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Done | JobState::Failed | JobState::Cancelled
        )
    }
}

/// A point-in-time snapshot of a job.
#[derive(Debug, Clone)]
pub struct JobSnapshot {
    /// Job id.
    pub id: JobId,
    /// State.
    pub state: JobState,
    /// Driver id string.
    pub driver: String,
    /// Candidate key.
    pub key: String,
    /// Detail, when present.
    pub detail: Option<String>,
    /// Created at.
    pub created_at: Instant,
}

/// A streamed progress frame.
#[derive(Debug, Clone)]
pub struct JobFrame {
    /// Job id.
    pub id: JobId,
    /// State at this frame.
    pub state: JobState,
    /// Step label.
    pub step: Option<String>,
    /// Progress 0..=100.
    pub progress: Option<u8>,
    /// Detail.
    pub detail: Option<String>,
}

/// Errors raised by the registry.
#[derive(Debug, thiserror::Error)]
pub enum JobError {
    /// The radio is busy with another job.
    #[error("radio busy with job {0}")]
    Busy(String),
    /// No such job.
    #[error("no such job")]
    NotFound,
    /// The driver rejected the candidate.
    #[error("unknown driver")]
    UnknownDriver,
    /// The driver rejected the request.
    #[error("validation: {0}")]
    Validation(#[from] wp_core::ValidationError),
    /// The driver failed at runtime.
    #[error("driver: {0}")]
    Driver(#[from] DriverError),
    /// Firmware update is not supported by this driver.
    #[error("firmware update is not supported")]
    FirmwareUnsupported,
}

/// Payload stored for the worker.
#[derive(Debug, Clone)]
pub enum JobPayload {
    /// Soft-AP settings programming.
    Program(ProgramRequestWire),
    /// HTTP firmware upload.
    Firmware(FirmwareJob),
}

/// Firmware job parameters (image stays on disk).
#[derive(Debug, Clone)]
pub struct FirmwareJob {
    /// Soft-AP, LAN, or USB.
    pub mode: ReachMode,
    /// Path to the image (`.app.bin`, merged `.bin`, or ELF).
    pub path: std::path::PathBuf,
    /// Explicit LAN IPv4, when set.
    pub host: Option<String>,
    /// USB serial device, when set.
    pub port: Option<String>,
    /// CSV partition table for ELF USB flashes.
    pub partition_table: Option<std::path::PathBuf>,
}

/// Internal job record.
struct JobRecord {
    snapshot: JobSnapshot,
    frames: Vec<JobFrame>,
    cancel: bool,
    payload: Option<JobPayload>,
}

/// A shared job registry. Only one job may be active at a time.
#[derive(Clone)]
pub struct JobRegistry {
    inner: Arc<Mutex<JobRegistryInner>>,
}

#[derive(Default)]
struct JobRegistryInner {
    active: Option<String>,
    jobs: std::collections::HashMap<String, JobRecord>,
    seq: u64,
}

impl JobRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(JobRegistryInner::default())),
        }
    }

    /// Try to start a job. Returns [`JobError::Busy`] when one is active.
    pub fn start(&self, driver: &str, key: &str) -> Result<JobId, JobError> {
        self.submit(driver, key, None)
    }

    /// Start a job and store the payload for the worker.
    pub fn submit(
        &self,
        driver: &str,
        key: &str,
        payload: Option<JobPayload>,
    ) -> Result<JobId, JobError> {
        let mut inner = self.inner.lock();
        if let Some(active) = inner.active.as_ref() {
            return Err(JobError::Busy(active.clone()));
        }
        inner.seq += 1;
        let id = format!("job-{}", inner.seq);
        let now = Instant::now();
        let rec = JobRecord {
            snapshot: JobSnapshot {
                id: JobId(id.clone()),
                state: JobState::Queued,
                driver: driver.into(),
                key: key.into(),
                detail: None,
                created_at: now,
            },
            frames: Vec::new(),
            cancel: false,
            payload,
        };
        inner.active = Some(id.clone());
        inner.jobs.insert(id.clone(), rec);
        Ok(JobId(id))
    }

    /// Take the stored payload (worker pulls once).
    pub fn take_payload(&self, id: &JobId) -> Option<JobPayload> {
        self.inner
            .lock()
            .jobs
            .get_mut(&id.0)
            .and_then(|r| r.payload.take())
    }

    /// Take the stored programming request (worker pulls once).
    pub fn take_request(&self, id: &JobId) -> Option<ProgramRequestWire> {
        match self.take_payload(id) {
            Some(JobPayload::Program(w)) => Some(w),
            _ => None,
        }
    }

    /// Whether a non-terminal job currently holds the radio.
    pub fn is_busy(&self) -> bool {
        self.inner.lock().active.is_some()
    }

    /// Push a state transition + frame for a job.
    pub fn transition(
        &self,
        id: &JobId,
        state: JobState,
        step: Option<&str>,
        progress: Option<u8>,
        detail: Option<&str>,
    ) {
        let mut inner = self.inner.lock();
        if let Some(rec) = inner.jobs.get_mut(&id.0) {
            rec.snapshot.state = state;
            if let Some(d) = detail {
                rec.snapshot.detail = Some(d.into());
            }
            rec.frames.push(JobFrame {
                id: id.clone(),
                state,
                step: step.map(Into::into),
                progress,
                detail: detail.map(Into::into),
            });
            if state.is_terminal() {
                inner.active = None;
                evict_old_jobs(&mut inner);
            }
        }
    }

    /// Mark a job cancelled. Does **not** free the radio slot or become
    /// terminal until the worker observes the flag, aborts work, and calls
    /// [`Self::transition`] to [`JobState::Cancelled`]. A second `program`
    /// while the worker is still tearing down returns [`JobError::Busy`].
    pub fn cancel(&self, id: &JobId) {
        let mut inner = self.inner.lock();
        let Some(rec) = inner.jobs.get_mut(&id.0) else {
            return;
        };
        if rec.snapshot.state.is_terminal() {
            rec.cancel = true;
            return;
        }
        rec.cancel = true;
        rec.snapshot.detail = Some("cancel requested".into());
        rec.frames.push(JobFrame {
            id: id.clone(),
            state: rec.snapshot.state,
            step: None,
            progress: None,
            detail: Some("cancel requested".into()),
        });
    }

    /// Whether cancellation was requested for a job.
    pub fn is_cancelled(&self, id: &JobId) -> bool {
        self.inner
            .lock()
            .jobs
            .get(&id.0)
            .map(|r| r.cancel)
            .unwrap_or(false)
    }

    /// Snapshot a job.
    pub fn snapshot(&self, id: &JobId) -> Option<JobSnapshot> {
        self.inner
            .lock()
            .jobs
            .get(&id.0)
            .map(|r| r.snapshot.clone())
    }

    /// Drain pending frames for a job since the given index.
    pub fn frames_since(&self, id: &JobId, since: usize) -> Option<Vec<JobFrame>> {
        self.inner
            .lock()
            .jobs
            .get(&id.0)
            .map(|r| r.frames.iter().skip(since).cloned().collect())
    }

    /// Number of frames recorded for a job.
    pub fn frame_count(&self, id: &JobId) -> usize {
        self.inner
            .lock()
            .jobs
            .get(&id.0)
            .map(|r| r.frames.len())
            .unwrap_or(0)
    }
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn evict_old_jobs(inner: &mut JobRegistryInner) {
    let over = inner.jobs.len().saturating_sub(MAX_JOB_HISTORY);
    if over == 0 {
        return;
    }
    let mut terminal: Vec<(String, Instant)> = inner
        .jobs
        .iter()
        .filter(|(id, rec)| {
            rec.snapshot.state.is_terminal() && inner.active.as_deref() != Some(id.as_str())
        })
        .map(|(id, rec)| (id.clone(), rec.snapshot.created_at))
        .collect();
    terminal.sort_by_key(|(_, t)| *t);
    for (id, _) in terminal.into_iter().take(over) {
        inner.jobs.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_heartbeat_beats_default_client_idle() {
        assert!(WATCH_HEARTBEAT < Duration::from_secs(10));
        assert!(WATCH_HEARTBEAT < FIRMWARE_DEADLINE);
        assert!(FIRMWARE_DEADLINE <= wp_link::USB_FLASH_DEADLINE);
    }

    #[test]
    fn cancel_keeps_slot_until_worker_transitions() {
        let jobs = JobRegistry::new();
        let id = jobs.submit("longfred", "key", None).expect("submit");
        assert!(jobs.is_busy());
        jobs.cancel(&id);
        assert!(jobs.is_cancelled(&id));
        assert!(
            jobs.is_busy(),
            "slot must stay held while worker tears down"
        );
        let snap = jobs.snapshot(&id).expect("snap");
        assert!(!snap.state.is_terminal());
        jobs.transition(&id, JobState::Cancelled, None, None, Some("cancelled"));
        assert!(!jobs.is_busy());
        assert_eq!(jobs.snapshot(&id).unwrap().state, JobState::Cancelled);
    }

    #[test]
    fn second_submit_is_busy_after_cancel_before_terminal() {
        let jobs = JobRegistry::new();
        let id = jobs.submit("wifred", "a", None).expect("first");
        jobs.cancel(&id);
        let err = jobs.submit("wifred", "b", None).expect_err("busy");
        assert!(matches!(err, JobError::Busy(_)));
    }
}
