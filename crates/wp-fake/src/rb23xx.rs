//! RailBOX RB23xx Soft-AP HTTP mock.

use crate::device::{drop_without_reply, not_found, ok_xml, FakeDevice, FakeRequest, FakeResponse};

/// Fake RB23xx file-browser HTTP device.
pub struct Rb23xxFake {
    /// Last uploaded firmware size, when a POST succeeded.
    pub last_upload_bytes: Option<usize>,
}

impl Rb23xxFake {
    /// Empty file browser.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_upload_bytes: None,
        }
    }
}

impl Default for Rb23xxFake {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeDevice for Rb23xxFake {
    fn driver_id(&self) -> &'static str {
        "rb23xx"
    }

    fn handle(&mut self, req: FakeRequest<'_>) -> FakeResponse {
        let (path, query) = req.path.split_once('?').unwrap_or((req.path, ""));
        match (req.method, path) {
            ("GET", "/") if query.starts_with("p=/") => ok_xml(b"<html>files</html>".to_vec()),
            ("POST", "/upload") => {
                let n = req.body.map(<[u8]>::len).unwrap_or(0);
                if n == 0 {
                    return FakeResponse {
                        status: 400,
                        content_type: "text/plain",
                        body: b"empty".to_vec(),
                        drop_without_reply: false,
                    };
                }
                self.last_upload_bytes = Some(n);
                drop_without_reply()
            }
            _ => not_found(),
        }
    }
}
