//! Streaming reader for `job.watch` progress frames.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use wp_proto::{read_frame, JobFrame, RequestKind, Response, ResultBody};

use crate::error::{unexpected_body, ClientError};

/// A streaming reader for `job.watch` progress frames.
///
/// Owns the connection and yields [`JobFrame`]s until the job reaches a
/// terminal state (`done`, `failed`, `cancelled`). Each read uses the
/// client's timeout as a per-frame idle deadline.
pub struct WatchStream {
    pub(crate) stream: UnixStream,
    pub(crate) idle: Duration,
}

impl WatchStream {
    /// Read the next progress frame, or `None` when the stream closes.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Frame`] on a short read, oversized frame, or
    /// codec failure. The per-frame idle read deadline is the client timeout.
    pub fn next_frame(&mut self) -> Result<Option<JobFrame>, ClientError> {
        self.stream
            .set_read_timeout(Some(self.idle))
            .map_err(wp_proto::FrameError::from)?;
        let resp: Response = match read_frame(&mut self.stream) {
            Ok(r) => r,
            Err(wp_proto::FrameError::UnexpectedEof { .. }) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if let Some(e) = resp.error {
            return Err(ClientError::from_error_body(e));
        }
        if resp.kind != RequestKind::JobWatch {
            return Err(ClientError::UnexpectedResponse(format!("{:?}", resp.kind)));
        }
        match resp.result {
            Some(ResultBody::JobWatch(f)) => Ok(Some(f)),
            Some(other) => Err(unexpected_body(other)),
            None => Err(ClientError::UnexpectedResponse("missing result".into())),
        }
    }

    /// Drain frames until the job reaches a terminal state, returning the
    /// final frame (or the last frame seen before an error / EOF).
    ///
    /// # Errors
    ///
    /// Propagates [`ClientError`] from the underlying reads.
    pub fn drain(mut self) -> Result<Option<JobFrame>, ClientError> {
        let mut last: Option<JobFrame> = None;
        loop {
            let frame = match self.next_frame()? {
                Some(f) => f,
                None => return Ok(last),
            };
            let terminal = frame.state.is_terminal();
            last = Some(frame);
            if terminal {
                return Ok(last);
            }
        }
    }
}
