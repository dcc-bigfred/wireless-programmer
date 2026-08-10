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
            // Only a zero-byte read is a clean close. A partial header or
            // payload means the daemon died mid-frame, which must surface as an
            // error rather than looking like an orderly end of stream.
            Err(wp_proto::FrameError::UnexpectedEof { read: 0, .. }) => return Ok(None),
            Err(wp_proto::FrameError::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(ClientError::WatchIdle { after: self.idle });
            }
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
    /// Intermediate frames are discarded. Use [`Self::drain_with`] to observe
    /// progress as it happens.
    ///
    /// # Errors
    ///
    /// Propagates [`ClientError`] from the underlying reads.
    pub fn drain(self) -> Result<Option<JobFrame>, ClientError> {
        self.drain_with(|_| {})
    }

    /// Drain frames until the job reaches a terminal state, invoking `on_frame`
    /// for every frame — including the terminal one — as it arrives.
    ///
    /// Returns the final frame, or the last frame seen before the stream
    /// closed. This is what a progress display should use: [`Self::drain`]
    /// only ever yields the outcome, so a caller using it sees nothing at all
    /// until the job is over.
    ///
    /// # Errors
    ///
    /// Propagates [`ClientError`] from the underlying reads.
    pub fn drain_with<F>(mut self, mut on_frame: F) -> Result<Option<JobFrame>, ClientError>
    where
        F: FnMut(&JobFrame),
    {
        let mut last: Option<JobFrame> = None;
        loop {
            let Some(frame) = self.next_frame()? else {
                return Ok(last);
            };
            on_frame(&frame);
            let terminal = frame.state.is_terminal();
            last = Some(frame);
            if terminal {
                return Ok(last);
            }
        }
    }
}
