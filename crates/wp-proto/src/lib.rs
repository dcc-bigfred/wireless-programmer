//! Socket wire types and length-prefixed JSON framing for `wireless-programmer`.
//!
//! Wire format matches the workspace convention from `microinit`
//! (`microinit/src/ipc.rs`): a 4-byte little-endian `u32` length header
//! followed by that many bytes of JSON. Each message is `type`-tagged.
//!
//! # Allocation
//!
//! Framing helpers allocate a buffer for the payload on the read path; this is
//! an allocation-conscious administrative protocol, not a hot path. The
//! [`MAX_FRAME_BYTES`] cap bounds the worst case to 1 MiB.

#![forbid(unsafe_code)]

mod framing;
mod results;
mod wire;

pub use framing::{read_frame, write_frame, FrameError, MAX_FRAME_BYTES};
pub use results::*;
pub use wire::*;
