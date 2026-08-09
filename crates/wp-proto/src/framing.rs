//! Length-prefixed JSON framing codec.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;

/// Absolute upper bound on a single frame payload: 1 MiB.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

const HEADER_LEN: usize = 4;

/// Errors raised by the framing codec.
#[derive(Debug, Error)]
pub enum FrameError {
    /// The payload length exceeded [`MAX_FRAME_BYTES`].
    #[error("frame payload of {len} bytes exceeds {max} bytes")]
    TooLarge {
        /// Declared payload length.
        len: usize,
        /// Configured maximum ([`MAX_FRAME_BYTES`]).
        max: usize,
    },
    /// The connection closed before a full header or payload was read.
    #[error("connection closed: read {read} of {needed} bytes")]
    UnexpectedEof {
        /// Bytes actually read.
        read: usize,
        /// Bytes needed to complete the frame.
        needed: usize,
    },
    /// The JSON payload could not be (de)serialised.
    #[error("json codec error: {0}")]
    Json(#[from] serde_json::Error),
    /// An underlying I/O failure.
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// Serialise `msg` and write one framed message to `writer`.
///
/// # Errors
///
/// Returns [`FrameError::TooLarge`] when the serialised payload exceeds
/// [`MAX_FRAME_BYTES`], and propagates I/O and JSON codec errors.
pub fn write_frame<W, T>(writer: &mut W, msg: &T) -> Result<(), FrameError>
where
    W: Write,
    T: Serialize,
{
    let payload = serde_json::to_vec(msg)?;
    let len = payload.len();
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            len,
            max: MAX_FRAME_BYTES,
        });
    }
    let header = u32::try_from(len).unwrap_or(u32::MAX).to_le_bytes();
    writer.write_all(&header)?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

/// Read one framed message from `reader` and decode it as `T`.
///
/// # Errors
///
/// Returns [`FrameError::TooLarge`] when the declared length exceeds
/// [`MAX_FRAME_BYTES`], [`FrameError::UnexpectedEof`] on a short read, and
/// propagates I/O and JSON codec errors.
pub fn read_frame<R, T>(reader: &mut R) -> Result<T, FrameError>
where
    R: Read,
    T: DeserializeOwned,
{
    let mut header = [0u8; HEADER_LEN];
    read_exact(reader, &mut header)?;
    let len = u32::from_le_bytes(header) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            len,
            max: MAX_FRAME_BYTES,
        });
    }
    let mut payload = vec![0u8; len];
    read_exact(reader, &mut payload)?;
    Ok(serde_json::from_slice(&payload)?)
}

fn read_exact<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<(), FrameError> {
    let mut filled = 0usize;
    while filled < buf.len() {
        let n = reader.read(&mut buf[filled..])?;
        if n == 0 {
            return Err(FrameError::UnexpectedEof {
                read: filled,
                needed: buf.len(),
            });
        }
        filled += n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Msg {
        v: u32,
        s: String,
    }

    #[test]
    fn round_trip_a_frame() {
        let msg = Msg {
            v: 7,
            s: "hello".into(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).expect("write");
        let back: Msg = read_frame(&mut buf.as_slice()).expect("read");
        assert_eq!(back, msg);
    }

    #[test]
    fn truncated_header_reports_eof() {
        let buf = [0u8; 2];
        let mut r = buf.as_slice();
        let err = read_frame::<_, Msg>(&mut r).unwrap_err();
        assert!(matches!(
            err,
            FrameError::UnexpectedEof { read: 2, needed: 4 }
        ));
    }

    #[test]
    fn truncated_payload_reports_eof() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(b"only a few");
        let mut r = buf.as_slice();
        let err = read_frame::<_, Msg>(&mut r).unwrap_err();
        assert!(matches!(
            err,
            FrameError::UnexpectedEof {
                read: 10,
                needed: 16
            }
        ));
    }

    #[test]
    fn oversized_payload_is_rejected_on_write() {
        // A Vec<u8> serialises to a JSON integer array far larger than its byte
        // length, so this comfortably exceeds the cap.
        let big = vec![0u8; MAX_FRAME_BYTES + 1];
        let mut buf = Vec::new();
        let err = write_frame(&mut buf, &big).unwrap_err();
        assert!(matches!(err, FrameError::TooLarge { max, .. } if max == MAX_FRAME_BYTES));
    }

    #[test]
    fn oversized_declared_length_is_rejected_on_read() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&((MAX_FRAME_BYTES as u32) + 1).to_le_bytes());
        let mut r = buf.as_slice();
        let err = read_frame::<_, Msg>(&mut r).unwrap_err();
        assert!(
            matches!(err, FrameError::TooLarge { len, max } if len == MAX_FRAME_BYTES + 1 && max == MAX_FRAME_BYTES)
        );
    }

    #[test]
    fn empty_payload_round_trips_as_unit() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &()).expect("write unit");
        let back: () = read_frame(&mut buf.as_slice()).expect("read unit");
        assert_eq!(back, ());
    }
}
