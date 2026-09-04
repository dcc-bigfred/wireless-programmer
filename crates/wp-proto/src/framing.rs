//! Length-prefixed JSON framing codec.

pub use bigfred_shared_daemon::ipc::{
    read_frame, write_frame, FrameError, DEFAULT_MAX_FRAME_BYTES as MAX_FRAME_BYTES,
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde::Serialize;

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
