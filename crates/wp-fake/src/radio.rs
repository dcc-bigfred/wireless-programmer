//! In-memory [`wp_link::Radio`] for tests.

use parking_lot::Mutex;
use wp_link::{Radio, RadioFut, ScanResult};

/// Fake radio that returns canned scan results and records calls.
pub struct FakeRadio {
    scan_results: Vec<ScanResult>,
    calls: Mutex<Vec<String>>,
}

impl FakeRadio {
    /// Construct with explicit scan results.
    #[must_use]
    pub fn new(results: Vec<ScanResult>) -> Self {
        Self {
            scan_results: results,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// One Soft-AP scan hit per known driver prefix.
    #[must_use]
    pub fn one_per_driver() -> Self {
        Self::new(vec![
            ScanResult {
                ssid: Some(format!(
                    "{}deadbe",
                    wp_drivers::wifred::WIFI_CONFIG_SSID_PREFIX
                )),
                bssid: Some("de:ad:be:ef:00:01".into()),
                rssi: Some(-42),
            },
            ScanResult {
                ssid: Some(format!(
                    "{}_deadbe",
                    wp_drivers::longfred::WIFI_CONFIG_SSID_PREFIX
                )),
                bssid: Some("de:ad:be:ef:00:02".into()),
                rssi: Some(-42),
            },
        ])
    }

    /// Recorded method names (`scan`, `connect_open`, …).
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().clone()
    }

    fn record(&self, name: &str) {
        self.calls.lock().push(name.to_string());
    }
}

impl Radio for FakeRadio {
    fn scan(&mut self, max: usize) -> RadioFut<'_, Vec<ScanResult>> {
        self.record("scan");
        let results: Vec<_> = self.scan_results.iter().take(max).cloned().collect();
        Box::pin(async move { Ok(results) })
    }

    fn connect_open(&mut self, _ssid: &str, _bssid: Option<[u8; 6]>) -> RadioFut<'_, ()> {
        self.record("connect_open");
        Box::pin(async move { Ok(()) })
    }

    fn set_address(
        &mut self,
        _addr: std::net::Ipv4Addr,
        _prefix_len: u8,
    ) -> RadioFut<'_, ()> {
        self.record("set_address");
        Box::pin(async move { Ok(()) })
    }

    fn link_up(&mut self) -> RadioFut<'_, ()> {
        self.record("link_up");
        Box::pin(async move { Ok(()) })
    }

    fn release(&mut self) -> RadioFut<'_, ()> {
        self.record("release");
        Box::pin(async move { Ok(()) })
    }
}
