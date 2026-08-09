//! The synchronous [`Client`] over a Unix socket.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use wp_proto::{
    read_frame, write_frame, CandidateRef, CandidateWire, DeviceInfoWire, HelloResult,
    IdentifyParams, JobParams, JobSnapshot, LinkStatusWire, Params, ProbeParams, ProgramParams,
    ProgramRequestWire, ProgramResult, Request, RequestKind, Response, ResultBody,
};

use crate::error::{unexpected_body, ClientError};

/// Default daemon socket when `DATA_DIR` is `/data`.
pub const DEFAULT_SOCKET: &str = "/data/run/wireless-programmer.sock";

/// Default per-operation timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// A synchronous client for the `wireless-programmer` daemon.
#[derive(Debug, Clone)]
pub struct Client {
    socket: PathBuf,
    timeout: Duration,
}

impl Client {
    /// Construct a client targeting `socket`.
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Construct a client targeting the resolved default socket, honouring
    /// `$BIGFRED_DATA_DIR` / `$DATA_DIR` / `/data`.
    #[must_use]
    pub fn default_socket() -> Self {
        Self::new(Self::resolve_socket())
    }

    /// Override the per-operation timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Resolve the daemon socket path from the environment.
    pub fn resolve_socket() -> PathBuf {
        if let Ok(d) = std::env::var("BIGFRED_DATA_DIR") {
            return PathBuf::from(d)
                .join("run")
                .join("wireless-programmer.sock");
        }
        if let Ok(d) = std::env::var("DATA_DIR") {
            return PathBuf::from(d)
                .join("run")
                .join("wireless-programmer.sock");
        }
        PathBuf::from(DEFAULT_SOCKET)
    }

    fn connect(&self) -> Result<UnixStream, ClientError> {
        let stream = UnixStream::connect(&self.socket).map_err(|source| ClientError::Connect {
            socket: self.socket.display().to_string(),
            source,
        })?;
        let t = Some(self.timeout);
        stream.set_read_timeout(t).ok();
        stream.set_write_timeout(t).ok();
        Ok(stream)
    }

    fn round_trip(&self, req: &Request) -> Result<Response, ClientError> {
        let mut stream = self.connect()?;
        write_frame(&mut stream, req)?;
        let resp: Response = read_frame(&mut stream)?;
        Ok(resp)
    }

    fn expect_result(
        &self,
        resp: Response,
        expected: RequestKind,
    ) -> Result<ResultBody, ClientError> {
        if let Some(e) = resp.error {
            return Err(ClientError::from_error_body(e));
        }
        if resp.kind != expected {
            return Err(ClientError::UnexpectedResponse(format!("{:?}", resp.kind)));
        }
        resp.result
            .ok_or_else(|| ClientError::UnexpectedResponse("missing result".into()))
    }

    /// `hello`: exchange version + driver capabilities.
    pub fn hello(&self) -> Result<HelloResult, ClientError> {
        let resp = self.round_trip(&Request {
            kind: RequestKind::Hello,
            params: Some(Params::None),
        })?;
        match self.expect_result(resp, RequestKind::Hello)? {
            ResultBody::Hello(h) => Ok(h),
            other => Err(unexpected_body(other)),
        }
    }

    /// `scan`: enumerate candidate devices on the radio.
    pub fn scan(&self) -> Result<Vec<CandidateWire>, ClientError> {
        let resp = self.round_trip(&Request {
            kind: RequestKind::Scan,
            params: Some(Params::None),
        })?;
        match self.expect_result(resp, RequestKind::Scan)? {
            ResultBody::Scan(c) => Ok(c),
            other => Err(unexpected_body(other)),
        }
    }

    /// `probe`: read a single candidate's device info.
    pub fn probe(&self, candidate: CandidateRef) -> Result<DeviceInfoWire, ClientError> {
        let resp = self.round_trip(&Request {
            kind: RequestKind::Probe,
            params: Some(Params::Probe(ProbeParams { candidate })),
        })?;
        match self.expect_result(resp, RequestKind::Probe)? {
            ResultBody::Probe(d) => Ok(d),
            other => Err(unexpected_body(other)),
        }
    }

    /// `program`: start a programming job, returns its id.
    pub fn program(
        &self,
        candidate: CandidateRef,
        request: ProgramRequestWire,
    ) -> Result<ProgramResult, ClientError> {
        let resp = self.round_trip(&Request {
            kind: RequestKind::Program,
            params: Some(Params::Program(ProgramParams { candidate, request })),
        })?;
        match self.expect_result(resp, RequestKind::Program)? {
            ResultBody::Program(p) => Ok(p),
            other => Err(unexpected_body(other)),
        }
    }

    /// `job.get`: snapshot a job's state.
    pub fn job_get(&self, job_id: impl Into<String>) -> Result<JobSnapshot, ClientError> {
        let resp = self.round_trip(&Request {
            kind: RequestKind::JobGet,
            params: Some(Params::Job(JobParams {
                job_id: job_id.into(),
            })),
        })?;
        match self.expect_result(resp, RequestKind::JobGet)? {
            ResultBody::Job(j) => Ok(j),
            other => Err(unexpected_body(other)),
        }
    }

    /// `job.cancel`: request cancellation of a running job.
    pub fn job_cancel(&self, job_id: impl Into<String>) -> Result<JobSnapshot, ClientError> {
        let resp = self.round_trip(&Request {
            kind: RequestKind::JobCancel,
            params: Some(Params::Job(JobParams {
                job_id: job_id.into(),
            })),
        })?;
        match self.expect_result(resp, RequestKind::JobCancel)? {
            ResultBody::JobCancelled(j) => Ok(j),
            other => Err(unexpected_body(other)),
        }
    }

    /// `identify`: blink the device LED so an operator can find it.
    pub fn identify(&self, candidate: CandidateRef, count: Option<u32>) -> Result<(), ClientError> {
        let resp = self.round_trip(&Request {
            kind: RequestKind::Identify,
            params: Some(Params::Identify(IdentifyParams { candidate, count })),
        })?;
        match self.expect_result(resp, RequestKind::Identify)? {
            ResultBody::Identify => Ok(()),
            other => Err(unexpected_body(other)),
        }
    }

    /// `link.status`: report radio/link state.
    pub fn link_status(&self) -> Result<LinkStatusWire, ClientError> {
        let resp = self.round_trip(&Request {
            kind: RequestKind::LinkStatus,
            params: Some(Params::None),
        })?;
        match self.expect_result(resp, RequestKind::LinkStatus)? {
            ResultBody::LinkStatus(s) => Ok(s),
            other => Err(unexpected_body(other)),
        }
    }

    /// `job.watch`: open a streaming connection for job progress frames.
    pub fn job_watch(&self, job_id: impl Into<String>) -> Result<crate::WatchStream, ClientError> {
        let mut stream = self.connect()?;
        write_frame(
            &mut stream,
            &Request {
                kind: RequestKind::JobWatch,
                params: Some(Params::Job(JobParams {
                    job_id: job_id.into(),
                })),
            },
        )?;
        stream.set_read_timeout(None).ok();
        Ok(crate::WatchStream {
            stream,
            idle: self.timeout,
        })
    }
}
