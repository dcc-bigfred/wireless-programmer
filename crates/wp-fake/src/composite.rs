//! Composite fake that multiplexes several [`FakeDevice`]s.

use crate::device::{not_found, FakeDevice, FakeRequest, FakeResponse};
use crate::longfred::LongFredFake;
use crate::wifred::WifredFake;

/// Tries each inner device and returns the first non-404 response.
pub struct CompositeFakeDevice {
    devices: Vec<Box<dyn FakeDevice>>,
}

impl CompositeFakeDevice {
    /// Build an empty composite.
    #[must_use]
    pub fn new(devices: Vec<Box<dyn FakeDevice>>) -> Self {
        Self { devices }
    }

    /// WiFred + LongFred mocks.
    #[must_use]
    pub fn all() -> Self {
        Self::new(vec![
            Box::new(WifredFake::new()),
            Box::new(LongFredFake::new()),
        ])
    }
}

impl FakeDevice for CompositeFakeDevice {
    fn driver_id(&self) -> &'static str {
        "composite"
    }

    fn handle(&mut self, req: FakeRequest<'_>) -> FakeResponse {
        for device in &mut self.devices {
            let resp = device.handle(FakeRequest {
                method: req.method,
                path: req.path,
                body: req.body,
            });
            if resp.status != 404 {
                return resp;
            }
        }
        not_found()
    }
}
