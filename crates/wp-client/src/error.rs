//! Client errors.

use std::io;

use wp_proto::ErrorBody;

/// Errors raised by [`crate::Client`].
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Could not connect to the daemon socket.
    #[error("connect {socket}: {source} (is wireless-programmer running?)")]
    Connect {
        /// Socket path that was attempted.
        socket: String,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// Framing / I/O / JSON codec failure on the wire.
    #[error(transparent)]
    Frame(#[from] wp_proto::FrameError),
    /// The daemon replied with an unexpected `type` tag.
    #[error("unexpected response type: {0}")]
    UnexpectedResponse(String),
    /// The daemon reported an error that did not map to a typed variant.
    #[error("{message}")]
    Server {
        /// Machine-readable error code.
        code: String,
        /// Human-readable message.
        message: String,
    },
    /// The radio is already in use by another job (`busy`).
    #[error("radio busy: {message}")]
    Busy {
        /// Human-readable message.
        message: String,
    },
    /// The referenced job or candidate does not exist (`notFound` /
    /// `candidateNotFound`).
    #[error("not found: {message}")]
    NotFound {
        /// Human-readable message.
        message: String,
    },
    /// A scan found no devices (`noCandidates`).
    #[error("no candidates: {message}")]
    NoCandidates {
        /// Human-readable message.
        message: String,
    },
    /// No progress frame arrived on a `job.watch` stream within the idle
    /// deadline. Distinguished from a plain I/O error because the usual cause
    /// is the daemon not pushing frames for this job, not a broken socket.
    #[error("no job progress frame within {after:?}")]
    WatchIdle {
        /// Idle deadline that elapsed (the client timeout).
        after: std::time::Duration,
    },
}

impl ClientError {
    /// Map a wire [`ErrorBody`] to a typed [`ClientError`].
    pub(crate) fn from_error_body(e: ErrorBody) -> Self {
        let ErrorBody { code, message } = e;
        match code.as_str() {
            "busy" => Self::Busy { message },
            "notFound" | "candidateNotFound" => Self::NotFound { message },
            "noCandidates" => Self::NoCandidates { message },
            _ => Self::Server { code, message },
        }
    }

    /// Whether this error is the radio being busy.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Busy { .. })
    }
}

/// Build an "unexpected body" error from a `ResultBody` debug string.
pub(crate) fn unexpected_body(b: wp_proto::ResultBody) -> ClientError {
    ClientError::UnexpectedResponse(format!("{b:?}"))
}
