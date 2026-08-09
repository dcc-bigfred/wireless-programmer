//! Job registry and state machine.
//!
//! The radio is an exclusive resource: at most one programming job runs at a
//! time. A second `program` returns [`JobError::Busy`]. Every exit path
//! releases the radio.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use wp_core::DriverError;

/// Overall job deadline.
pub const JOB_DEADLINE: Duration = Duration::from_secs(120);

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
}

/// Internal job record.
struct JobRecord {
    snapshot: JobSnapshot,
    frames: Vec<JobFrame>,
    cancel: bool,
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
        };
        inner.active = Some(id.clone());
        inner.jobs.insert(id.clone(), rec);
        Ok(JobId(id))
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
            }
        }
    }

    /// Mark a job cancelled (caller request).
    pub fn cancel(&self, id: &JobId) {
        let mut inner = self.inner.lock();
        if let Some(rec) = inner.jobs.get_mut(&id.0) {
            rec.cancel = true;
        }
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
