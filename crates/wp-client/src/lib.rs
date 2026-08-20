//! Rust client SDK for the `wireless-programmer` Unix socket API.
//!
//! Mirrors `go/client`: a synchronous, std-only client over
//! [`std::os::unix::net::UnixStream`] using the length-prefixed JSON framing
//! from `wp-proto`. Intended both for programmatic callers and for the
//! `wireless-programmer` CLI subcommands (`scan`, `program`, ...).

#![forbid(unsafe_code)]

mod client;
mod error;
mod watch;

pub use client::{Client, DEFAULT_SOCKET, DEFAULT_TIMEOUT};
pub use error::ClientError;
pub use watch::WatchStream;

pub use wp_proto::{
    CandidateRef, CandidateWire, DeviceInfoWire, FunctionMappingWire, HelloResult, JobFrame,
    JobSnapshot, JobStateWire, LinkStatusWire, ProgramRequestWire, ProgramResult, ReachMode,
    RosterEntryWire, ThrottleServerWire, WifiCredentialsWire,
};
