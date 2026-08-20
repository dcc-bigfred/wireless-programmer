//! Tokio runtime wrapping radio + programming worker.
//!
//! IPC stays sync (one `std::thread` per connection). Radio work and the
//! programming worker run on a multi-threaded tokio runtime. Sync handlers
//! bridge via [`RuntimeHandle::block_on`].

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use wp_core::{
    CommissioningNet, Observation, ProgramRequest, ProgressSink, RosterEntry, ThrottleServer,
    Transport, WifiCredentials,
};
use wp_link::{BoundedHttpClient, Radio, ScanResult};
use wp_proto::{ProgramRequestWire, ReachMode};

use crate::config::Config;
use crate::drivers::{Driver, DriverRegistry};
use crate::jobs::{JobId, JobRegistry, JobState};

/// Cached candidate from the last scan (SSID needed for Soft-AP connect).
#[derive(Debug, Clone)]
pub struct CachedCandidate {
    /// Soft-AP SSID.
    pub ssid: String,
    /// Optional BSSID (colon hex).
    pub bssid: Option<String>,
    /// Driver id.
    pub driver: String,
    /// Candidate key.
    pub key: String,
    /// Label (usually SSID).
    pub label: String,
    /// RSSI when known.
    pub rssi: Option<i32>,
    /// How this candidate was discovered.
    pub mode: ReachMode,
}

/// Shared handle used by IPC and the worker.
pub struct Runtime {
    rt: tokio::runtime::Runtime,
    radio: Arc<tokio::sync::Mutex<Box<dyn Radio>>>,
    cfg: Config,
    registry: Arc<DriverRegistry>,
    jobs: JobRegistry,
    tx: tokio::sync::mpsc::Sender<JobId>,
    /// Last scan results keyed by `(driver, key)`.
    cache: Mutex<HashMap<(String, String), CachedCandidate>>,
    /// Soft-AP radio is associated for an in-flight AP job or probe.
    radio_held: AtomicBool,
}

impl Runtime {
    /// Build the runtime, spawn the programming worker, and wrap `radio`.
    pub fn new(
        cfg: Config,
        registry: DriverRegistry,
        jobs: JobRegistry,
        radio: Box<dyn Radio>,
    ) -> Result<Arc<Self>, wp_core::DriverError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("wp-runtime")
            .build()
            .map_err(|e| wp_core::DriverError::Other(format!("tokio runtime: {e}")))?;

        let (tx, rx) = tokio::sync::mpsc::channel::<JobId>(8);
        let radio = Arc::new(tokio::sync::Mutex::new(radio));
        let registry = Arc::new(registry);

        let this = Arc::new(Self {
            rt,
            radio: Arc::clone(&radio),
            cfg,
            registry: Arc::clone(&registry),
            jobs: jobs.clone(),
            tx,
            cache: Mutex::new(HashMap::new()),
            radio_held: AtomicBool::new(false),
        });

        let worker = Arc::clone(&this);
        this.rt.spawn(async move {
            worker_loop(worker, rx).await;
        });

        Ok(this)
    }

    /// Borrow the tokio runtime (e.g. to spawn the fake HTTP server).
    pub fn handle(&self) -> tokio::runtime::Handle {
        self.rt.handle().clone()
    }

    /// Shared job registry.
    pub fn jobs(&self) -> &JobRegistry {
        &self.jobs
    }

    /// Driver registry.
    pub fn registry(&self) -> &DriverRegistry {
        &self.registry
    }

    /// Config snapshot.
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Whether the wireless radio is held for Soft-AP work.
    pub fn radio_held(&self) -> bool {
        self.radio_held.load(Ordering::SeqCst)
    }

    /// Scan the radio and claim candidates via the driver registry.
    pub fn scan(&self) -> Result<Vec<CachedCandidate>, wp_core::DriverError> {
        let radio = Arc::clone(&self.radio);
        let results = self.rt.handle().block_on(async move {
            let mut r = radio.lock().await;
            r.scan(64).await
        })?;

        let mut out = Vec::new();
        let mut cache = self.cache.lock();
        cache.retain(|_, v| v.mode != ReachMode::Ap);
        for s in results {
            let obs = observation_from_scan(&s);
            if let Some(c) = self.registry.identify(&obs) {
                let ssid = c.label.clone();
                let cached = CachedCandidate {
                    ssid,
                    bssid: s.bssid,
                    driver: c.driver.clone(),
                    key: c.key.clone(),
                    label: c.label,
                    rssi: c.rssi,
                    mode: ReachMode::Ap,
                };
                cache.insert((c.driver, c.key), cached.clone());
                out.push(cached);
            }
        }
        Ok(out)
    }

    /// Discover LongFred HTTP OTA advertisers via mDNS (`_longfred-ota._tcp`).
    pub fn scan_lan(&self) -> Result<Vec<CachedCandidate>, wp_core::DriverError> {
        let hosts = wp_link::discover_ota_hosts(Duration::from_millis(1500))
            .map_err(|e| wp_core::DriverError::Other(format!("mdns: {e}")))?;
        let mut out = Vec::new();
        let mut cache = self.cache.lock();
        for h in hosts {
            let key = h.ipv4.to_string();
            let cached = CachedCandidate {
                ssid: String::new(),
                bssid: None,
                driver: Driver::LongFred.id_str().into(),
                key: key.clone(),
                label: format!("{} ({})", h.hostname, h.ipv4),
                rssi: None,
                mode: ReachMode::Lan,
            };
            cache.insert((cached.driver.clone(), key), cached.clone());
            out.push(cached);
        }
        Ok(out)
    }

    /// Enumerate USB serial ports (`espflash list-ports` / `/dev/ttyUSB*` / `ttyACM*`).
    pub fn scan_usb(&self) -> Result<Vec<CachedCandidate>, wp_core::DriverError> {
        let ports = wp_link::list_usb_ports()
            .map_err(|e| wp_core::DriverError::Other(format!("usb scan: {e}")))?;
        let mut out = Vec::new();
        let mut cache = self.cache.lock();
        for p in ports {
            let cached = CachedCandidate {
                ssid: String::new(),
                bssid: None,
                driver: Driver::LongFred.id_str().into(),
                key: p.path.clone(),
                label: p.label,
                rssi: None,
                mode: ReachMode::Usb,
            };
            cache.insert((cached.driver.clone(), cached.key.clone()), cached.clone());
            out.push(cached);
        }
        Ok(out)
    }

    /// Discover Z21 LAN command stations (mDNS `_z21._udp` + UDP serial probe).
    pub fn scan_z21(&self) -> Result<Vec<CachedCandidate>, wp_core::DriverError> {
        let hosts = wp_link::discover_z21(Duration::from_millis(1500))
            .map_err(|e| wp_core::DriverError::Other(format!("z21 scan: {e}")))?;
        let mut out = Vec::new();
        let mut cache = self.cache.lock();
        cache.retain(|_, v| v.mode != ReachMode::Z21);
        for h in hosts {
            let key = h.key();
            let cached = CachedCandidate {
                ssid: String::new(),
                bssid: None,
                driver: Driver::Fred.id_str().into(),
                key: key.clone(),
                label: h.label(),
                rssi: None,
                mode: ReachMode::Z21,
            };
            cache.insert((cached.driver.clone(), key), cached.clone());
            out.push(cached);
        }
        Ok(out)
    }

    /// Remember a Z21 `host:port` so `program` can skip `scan --mode z21`.
    pub fn cache_z21_host(&self, key: &str, label: Option<&str>) {
        let cached = CachedCandidate {
            ssid: String::new(),
            bssid: None,
            driver: Driver::Fred.id_str().into(),
            key: key.to_string(),
            label: label.unwrap_or(key).to_string(),
            rssi: None,
            mode: ReachMode::Z21,
        };
        self.cache
            .lock()
            .insert((cached.driver.clone(), cached.key.clone()), cached);
    }

    /// Remember a USB serial device so `updateFirmware` can skip scan when `--port` is set.
    pub fn cache_usb_port(&self, port: &str, label: Option<&str>) {
        let cached = CachedCandidate {
            ssid: String::new(),
            bssid: None,
            driver: Driver::LongFred.id_str().into(),
            key: port.to_string(),
            label: label.unwrap_or(port).to_string(),
            rssi: None,
            mode: ReachMode::Usb,
        };
        self.cache
            .lock()
            .insert((cached.driver.clone(), cached.key.clone()), cached);
    }

    /// Remember a LAN host so `updateFirmware` can skip scan when `--host` is set.
    pub fn cache_lan_host(&self, host: &str, label: Option<&str>) {
        let cached = CachedCandidate {
            ssid: String::new(),
            bssid: None,
            driver: Driver::LongFred.id_str().into(),
            key: host.to_string(),
            label: label.unwrap_or(host).to_string(),
            rssi: None,
            mode: ReachMode::Lan,
        };
        self.cache
            .lock()
            .insert((cached.driver.clone(), cached.key.clone()), cached);
    }

    /// Queue a firmware-upload job.
    pub fn submit_firmware(
        &self,
        driver: Driver,
        key: &str,
        job: crate::jobs::FirmwareJob,
    ) -> Result<JobId, crate::jobs::JobError> {
        if !self.registry.supports_firmware_update(driver) {
            return Err(crate::jobs::JobError::FirmwareUnsupported);
        }
        let id = self.jobs.submit(
            driver.id_str(),
            key,
            Some(crate::jobs::JobPayload::Firmware(job)),
        )?;
        tracing::info!(
            job_id = %id.0,
            driver = driver.id_str(),
            key,
            "firmware job queued for worker"
        );
        if let Err(e) = self.tx.blocking_send(id.clone()) {
            tracing::error!(job_id = %id.0, error = %e, "failed to enqueue firmware job");
            self.jobs.transition(
                &id,
                JobState::Failed,
                None,
                None,
                Some(&format!("worker channel closed: {e}")),
            );
            return Err(crate::jobs::JobError::Driver(wp_core::DriverError::Other(
                "worker channel closed".into(),
            )));
        }
        Ok(id)
    }

    /// Look up a cached candidate.
    pub fn cached(&self, driver: &str, key: &str) -> Option<CachedCandidate> {
        self.cache
            .lock()
            .get(&(driver.to_string(), key.to_string()))
            .cloned()
    }

    /// Queue a programming job for the worker.
    pub fn submit_program(
        &self,
        driver: Driver,
        key: &str,
        request: ProgramRequestWire,
    ) -> Result<JobId, crate::jobs::JobError> {
        // Validate before occupying the radio slot.
        let owned = OwnedRequest::from_wire(request.clone());
        let borrowed = owned.borrow();
        self.registry.validate(driver, &borrowed)?;

        if driver == Driver::Fred && self.cached(driver.id_str(), key).is_none() {
            self.cache_z21_host(key, None);
        }

        let id = self.jobs.submit(
            driver.id_str(),
            key,
            Some(crate::jobs::JobPayload::Program(request)),
        )?;
        tracing::info!(
            job_id = %id.0,
            driver = driver.id_str(),
            key,
            "program job queued for worker"
        );
        if let Err(e) = self.tx.blocking_send(id.clone()) {
            tracing::error!(job_id = %id.0, error = %e, "failed to enqueue job to worker");
            self.jobs.transition(
                &id,
                JobState::Failed,
                None,
                None,
                Some(&format!("worker channel closed: {e}")),
            );
            return Err(crate::jobs::JobError::Driver(wp_core::DriverError::Other(
                "worker channel closed".into(),
            )));
        }
        Ok(id)
    }

    /// Connect to a candidate Soft-AP and probe.
    pub fn probe(
        &self,
        driver: Driver,
        key: &str,
    ) -> Result<serde_json::Value, wp_core::DriverError> {
        let candidate = self.cached(driver.id_str(), key).ok_or_else(|| {
            wp_core::DriverError::Other("candidate not in scan cache; run scan first".into())
        })?;
        let net = self.effective_net(driver);
        let radio = Arc::clone(&self.radio);
        let registry = Arc::clone(&self.registry);
        tracing::info!(
            driver = driver.id_str(),
            key,
            ssid = %candidate.ssid,
            bssid = ?candidate.bssid,
            "probe: connecting to Soft-AP"
        );
        let _hold = RadioHold::new(self);
        self.rt.handle().block_on(async move {
            let mut r = radio.lock().await;
            let bssid = parse_bssid(candidate.bssid.as_deref());
            if let Err(e) = r.connect_open(&candidate.ssid, bssid).await {
                tracing::warn!(
                    ssid = %candidate.ssid,
                    error = %e,
                    "probe: Soft-AP connect failed"
                );
                let _ = r.release().await;
                return Err(e);
            }
            tracing::info!(ssid = %candidate.ssid, "probe: Soft-AP connect ok");
            r.set_address(net.source, net.prefix).await?;
            r.link_up().await?;

            let result = {
                let mut client = make_http_client(&net);
                let transport = Transport::Http(&mut client);
                registry.probe(driver, transport).await
            };

            match &result {
                Ok(_) => tracing::info!(ssid = %candidate.ssid, "probe: done"),
                Err(e) => tracing::warn!(ssid = %candidate.ssid, error = %e, "probe: failed"),
            }

            let _ = r.release().await;
            result
        })
    }

    /// Blink a WiFred LED over the Soft-AP (`GET /flashred.html`).
    pub fn identify(
        &self,
        driver: Driver,
        key: &str,
        count: Option<u32>,
    ) -> Result<(), wp_core::DriverError> {
        let candidate = self.cached(driver.id_str(), key).ok_or_else(|| {
            wp_core::DriverError::Other("candidate not in scan cache; run scan first".into())
        })?;
        let net = self.effective_net(driver);
        let radio = Arc::clone(&self.radio);
        let registry = Arc::clone(&self.registry);
        let _hold = RadioHold::new(self);
        self.rt.handle().block_on(async move {
            let mut r = radio.lock().await;
            let bssid = parse_bssid(candidate.bssid.as_deref());
            r.connect_open(&candidate.ssid, bssid).await?;
            r.set_address(net.source, net.prefix).await?;
            r.link_up().await?;
            let result = {
                let mut client = make_http_client(&net);
                let transport = Transport::Http(&mut client);
                registry.blink(driver, transport, count).await
            };
            let _ = r.release().await;
            result
        })
    }

    fn effective_net(&self, driver: Driver) -> CommissioningNet {
        self.cfg
            .commissioning_net_override
            .unwrap_or_else(|| driver.commissioning_net())
    }
}

fn observation_from_scan(s: &ScanResult) -> Observation {
    Observation {
        ssid: s.ssid.clone(),
        bssid: s.bssid.clone(),
        rssi: s.rssi,
        extra: serde_json::Value::Null,
    }
}

fn parse_bssid(s: Option<&str>) -> Option<[u8; 6]> {
    let s = s?;
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        out[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(out)
}

fn make_http_client(net: &CommissioningNet) -> BoundedHttpClient {
    let source = SocketAddr::from((net.source, 0));
    BoundedHttpClient::new(net.host.to_string(), net.port).with_source(source)
}

fn make_http_client_cancel(net: &CommissioningNet, cancel: Arc<AtomicBool>) -> BoundedHttpClient {
    make_http_client(net).with_cancel(cancel)
}

struct RadioHold<'a> {
    rt: &'a Runtime,
}

impl<'a> RadioHold<'a> {
    fn new(rt: &'a Runtime) -> Self {
        rt.radio_held.store(true, Ordering::SeqCst);
        Self { rt }
    }
}

impl Drop for RadioHold<'_> {
    fn drop(&mut self) {
        self.rt.radio_held.store(false, Ordering::SeqCst);
    }
}

fn spawn_cancel_watch(rt: &Runtime, id: &JobId) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(rt.jobs.is_cancelled(id)));
    let jobs = rt.jobs.clone();
    let id = id.clone();
    let f = Arc::clone(&flag);
    rt.handle().spawn(async move {
        loop {
            if jobs.is_cancelled(&id) {
                f.store(true, Ordering::Relaxed);
                break;
            }
            if jobs
                .snapshot(&id)
                .map(|s| s.state.is_terminal())
                .unwrap_or(true)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    flag
}

/// Owned copy of a wire request so we can borrow into [`ProgramRequest`].
struct OwnedRequest {
    identity: String,
    wifi_ssid: String,
    wifi_psk: Option<String>,
    server_host: String,
    server_port: u16,
    server_automatic: bool,
    roster: Vec<OwnedRoster>,
    bigfred_login: Option<String>,
    bigfred_pin: Option<String>,
    roster_mode: Option<String>,
}

struct OwnedRoster {
    address: Option<u16>,
    long_address: Option<bool>,
    mode: Option<String>,
    direction: Option<u8>,
    functions: Vec<wp_core::FunctionMapping>,
}

impl OwnedRequest {
    fn from_wire(w: ProgramRequestWire) -> Self {
        Self {
            identity: w.identity,
            wifi_ssid: w.wifi.ssid,
            wifi_psk: w.wifi.psk,
            server_host: w.server.host,
            server_port: w.server.port,
            server_automatic: w.server.automatic.unwrap_or(false),
            roster: w
                .roster
                .into_iter()
                .map(|e| OwnedRoster {
                    address: e.address,
                    long_address: e.long_address,
                    mode: e.mode,
                    direction: e.direction,
                    functions: e
                        .functions
                        .into_iter()
                        .map(|f| wp_core::FunctionMapping {
                            index: f.index,
                            value: f.value,
                        })
                        .collect(),
                })
                .collect(),
            bigfred_login: w.bigfred.as_ref().map(|b| b.login.clone()),
            bigfred_pin: w.bigfred.as_ref().map(|b| b.pin.clone()),
            roster_mode: w.roster_mode,
        }
    }

    fn borrow(&self) -> ProgramRequest<'_> {
        let roster: Vec<RosterEntry<'_>> = self
            .roster
            .iter()
            .map(|e| RosterEntry {
                address: e.address,
                long_address: e.long_address,
                mode: e.mode.as_deref(),
                direction: e.direction,
                functions: e.functions.clone(),
            })
            .collect();
        let bigfred = match (&self.bigfred_login, &self.bigfred_pin) {
            (Some(login), Some(pin)) => Some(wp_core::BigfredCreds {
                login: login.as_str(),
                pin: pin.as_str(),
            }),
            _ => None,
        };
        ProgramRequest {
            identity: &self.identity,
            wifi: WifiCredentials {
                ssid: &self.wifi_ssid,
                psk: self.wifi_psk.as_deref(),
            },
            server: ThrottleServer {
                host: &self.server_host,
                port: self.server_port,
                automatic: self.server_automatic,
            },
            roster,
            bigfred,
            roster_mode: self.roster_mode.as_deref(),
        }
    }
}

struct JobProgressSink<'a> {
    jobs: &'a JobRegistry,
    id: &'a JobId,
}

impl ProgressSink for JobProgressSink<'_> {
    fn step(&mut self, step: &str) {
        let state = match step {
            "read" | "probe" => JobState::Probing,
            "identity" | "locos" | "functions" | "server" | "wifi" | "write" => JobState::Writing,
            "verify" => JobState::Verifying,
            "restart" | "exit" => JobState::Restarting,
            _ => JobState::Writing,
        };
        tracing::info!(job_id = %self.id.0, step, ?state, "job step");
        self.jobs.transition(self.id, state, Some(step), None, None);
    }

    fn progress(&mut self, progress: u8) {
        let state = self
            .jobs
            .snapshot(self.id)
            .map(|s| s.state)
            .unwrap_or(JobState::Writing);
        self.jobs
            .transition(self.id, state, None, Some(progress), None);
    }

    fn detail(&mut self, detail: &str) {
        let state = self
            .jobs
            .snapshot(self.id)
            .map(|s| s.state)
            .unwrap_or(JobState::Writing);
        self.jobs
            .transition(self.id, state, None, None, Some(detail));
    }
}

async fn worker_loop(rt: Arc<Runtime>, mut rx: tokio::sync::mpsc::Receiver<JobId>) {
    while let Some(id) = rx.recv().await {
        run_job(&rt, id).await;
    }
    tracing::warn!("programming worker channel closed; exiting worker loop");
}

async fn run_job(rt: &Runtime, id: JobId) {
    if rt.jobs.is_cancelled(&id) {
        tracing::info!(job_id = %id.0, "job cancelled before start");
        if rt
            .jobs
            .snapshot(&id)
            .map(|s| !s.state.is_terminal())
            .unwrap_or(false)
        {
            rt.jobs
                .transition(&id, JobState::Cancelled, None, None, Some("cancelled"));
        }
        return;
    }

    let Some(payload) = rt.jobs.take_payload(&id) else {
        tracing::error!(job_id = %id.0, "job missing payload");
        rt.jobs.transition(
            &id,
            JobState::Failed,
            None,
            None,
            Some("missing job payload"),
        );
        return;
    };

    match payload {
        crate::jobs::JobPayload::Program(wire) => run_program_job(rt, id, wire).await,
        crate::jobs::JobPayload::Firmware(job) => run_firmware_job(rt, id, job).await,
    }
}

async fn run_fred_program_job(rt: &Runtime, id: JobId, wire: ProgramRequestWire, key: &str) {
    let Some(target) = wp_link::Z21Host::parse_key(key) else {
        rt.jobs.transition(
            &id,
            JobState::Failed,
            None,
            None,
            Some("invalid Z21 address (want host:port)"),
        );
        return;
    };
    let Some(loco) = wire
        .roster
        .first()
        .and_then(|e| e.address)
        .filter(|a| (1..=10239).contains(a))
    else {
        rt.jobs.transition(
            &id,
            JobState::Failed,
            None,
            None,
            Some("fred programming needs exactly one DCC address 1..=10239"),
        );
        return;
    };

    if rt.jobs.is_cancelled(&id) {
        rt.jobs
            .transition(&id, JobState::Cancelled, None, None, Some("cancelled"));
        return;
    }

    rt.jobs
        .transition(&id, JobState::Writing, Some("write"), Some(10), None);

    let sock = match std::net::UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            rt.jobs.transition(
                &id,
                JobState::Failed,
                Some("write"),
                None,
                Some(&format!("udp bind: {e}")),
            );
            return;
        }
    };

    let outcome = tokio::task::spawn_blocking(move || wp_link::dispatch_addr(&sock, target, loco))
        .await
        .unwrap_or_else(|e| Err(wp_link::DispatchError::Io(e.to_string())));

    if rt.jobs.is_cancelled(&id) {
        rt.jobs
            .transition(&id, JobState::Cancelled, None, None, Some("cancelled"));
        return;
    }

    match outcome {
        Ok(wp_link::DispatchOutcome::Slot(slot)) => {
            rt.jobs.transition(
                &id,
                JobState::Done,
                Some("done"),
                Some(100),
                Some(&format!("slot {slot}")),
            );
        }
        Ok(wp_link::DispatchOutcome::NoAck) => {
            rt.jobs.transition(
                &id,
                JobState::Done,
                Some("done"),
                Some(100),
                Some("noAck"),
            );
        }
        Err(wp_link::DispatchError::Rejected) => {
            rt.jobs.transition(
                &id,
                JobState::Failed,
                Some("write"),
                None,
                Some("dispatchFailed"),
            );
        }
        Err(wp_link::DispatchError::UnknownCommand) => {
            rt.jobs.transition(
                &id,
                JobState::Failed,
                Some("write"),
                None,
                Some("z21NoLocoNet"),
            );
        }
        Err(e) => {
            rt.jobs.transition(
                &id,
                JobState::Failed,
                Some("write"),
                None,
                Some(&e.to_string()),
            );
        }
    }
}

async fn run_program_job(rt: &Runtime, id: JobId, wire: ProgramRequestWire) {
    let snap = match rt.jobs.snapshot(&id) {
        Some(s) => s,
        None => {
            tracing::error!(job_id = %id.0, "job disappeared before start");
            return;
        }
    };
    let Some(driver) = Driver::from_id(&snap.driver) else {
        tracing::error!(job_id = %id.0, driver = %snap.driver, "unknown driver");
        rt.jobs
            .transition(&id, JobState::Failed, None, None, Some("unknown driver"));
        return;
    };

    let candidate = match rt.cached(&snap.driver, &snap.key) {
        Some(c) => c,
        None => {
            tracing::error!(
                job_id = %id.0,
                driver = %snap.driver,
                key = %snap.key,
                "candidate not in scan cache; run scan first"
            );
            rt.jobs.transition(
                &id,
                JobState::Failed,
                None,
                None,
                Some("candidate not in scan cache; run scan first"),
            );
            return;
        }
    };

    tracing::info!(
        job_id = %id.0,
        driver = %snap.driver,
        key = %snap.key,
        ssid = %candidate.ssid,
        bssid = ?candidate.bssid,
        identity = %wire.identity,
        wifi_ssid = %wire.wifi.ssid,
        "job started"
    );

    if driver == Driver::Fred {
        run_fred_program_job(rt, id, wire, &candidate.key).await;
        return;
    }

    let owned = OwnedRequest::from_wire(wire);
    let net = rt.effective_net(driver);

    rt.jobs
        .transition(&id, JobState::Joining, Some("join"), None, None);

    if rt.jobs.is_cancelled(&id) {
        tracing::info!(job_id = %id.0, "job cancelled before Soft-AP join");
        rt.jobs
            .transition(&id, JobState::Cancelled, None, None, Some("cancelled"));
        return;
    }

    let _hold = RadioHold::new(rt);
    let mut radio = rt.radio.lock().await;
    let bssid = parse_bssid(candidate.bssid.as_deref());
    tracing::info!(
        job_id = %id.0,
        ssid = %candidate.ssid,
        bssid = ?candidate.bssid,
        "connecting to Soft-AP"
    );
    if let Err(e) = radio.connect_open(&candidate.ssid, bssid).await {
        tracing::warn!(
            job_id = %id.0,
            ssid = %candidate.ssid,
            error = %e,
            "Soft-AP connect failed"
        );
        rt.jobs.transition(
            &id,
            JobState::Failed,
            Some("join"),
            None,
            Some(&e.to_string()),
        );
        let _ = radio.release().await;
        return;
    }
    tracing::info!(
        job_id = %id.0,
        ssid = %candidate.ssid,
        "Soft-AP connect ok"
    );

    tracing::info!(
        job_id = %id.0,
        source = %net.source,
        prefix = net.prefix,
        host = %net.host,
        port = net.port,
        "assigning on-link address"
    );
    if let Err(e) = radio.set_address(net.source, net.prefix).await {
        tracing::warn!(
            job_id = %id.0,
            source = %net.source,
            error = %e,
            "set_address failed"
        );
        rt.jobs.transition(
            &id,
            JobState::Failed,
            Some("join"),
            None,
            Some(&e.to_string()),
        );
        let _ = radio.release().await;
        return;
    }
    if let Err(e) = radio.link_up().await {
        tracing::warn!(job_id = %id.0, error = %e, "link_up failed");
        rt.jobs.transition(
            &id,
            JobState::Failed,
            Some("join"),
            None,
            Some(&e.to_string()),
        );
        let _ = radio.release().await;
        return;
    }
    tracing::info!(
        job_id = %id.0,
        target = %format!("{}:{}", net.host, net.port),
        "radio ready; starting driver program"
    );

    if rt.jobs.is_cancelled(&id) {
        tracing::info!(job_id = %id.0, "job cancelled after Soft-AP join");
        let _ = radio.release().await;
        rt.jobs
            .transition(&id, JobState::Cancelled, None, None, Some("cancelled"));
        return;
    }

    // Drop the radio lock while the sync HTTP client talks to the device —
    // Soft-AP stays associated; we re-acquire only to release.
    drop(radio);

    let borrowed = owned.borrow();
    let mut sink = JobProgressSink {
        jobs: &rt.jobs,
        id: &id,
    };
    let cancel = spawn_cancel_watch(rt, &id);
    let mut client = make_http_client_cancel(&net, cancel);
    let transport = Transport::Http(&mut client);
    let outcome = match tokio::time::timeout(crate::jobs::JOB_DEADLINE, async {
        rt.registry
            .program(driver, transport, &borrowed, &mut sink)
            .await
    })
    .await
    {
        Ok(inner) => inner,
        Err(_) => Err(wp_core::DriverError::DeadlineElapsed("program")),
    };

    {
        let mut radio = rt.radio.lock().await;
        match radio.release().await {
            Ok(()) => tracing::info!(job_id = %id.0, "radio released"),
            Err(e) => tracing::warn!(job_id = %id.0, error = %e, "radio release failed"),
        }
    }

    if rt.jobs.is_cancelled(&id) {
        tracing::info!(job_id = %id.0, "job cancelled after program");
        if rt
            .jobs
            .snapshot(&id)
            .map(|s| !s.state.is_terminal())
            .unwrap_or(false)
        {
            rt.jobs
                .transition(&id, JobState::Cancelled, None, None, Some("cancelled"));
        }
        return;
    }

    match outcome {
        Ok(o) => {
            tracing::info!(
                job_id = %id.0,
                driver = %snap.driver,
                key = %snap.key,
                restarted = o.restarted,
                "job finished successfully"
            );
            let detail = if o.restarted { Some("restarted") } else { None };
            rt.jobs
                .transition(&id, JobState::Done, Some("done"), Some(100), detail);
        }
        Err(e) => {
            tracing::warn!(
                job_id = %id.0,
                driver = %snap.driver,
                key = %snap.key,
                error = %e,
                "job failed"
            );
            rt.jobs
                .transition(&id, JobState::Failed, None, None, Some(&e.to_string()));
        }
    }
}

async fn run_firmware_job(rt: &Runtime, id: JobId, job: crate::jobs::FirmwareJob) {
    use std::net::Ipv4Addr;

    let snap = match rt.jobs.snapshot(&id) {
        Some(s) => s,
        None => return,
    };
    let Some(driver) = Driver::from_id(&snap.driver) else {
        rt.jobs
            .transition(&id, JobState::Failed, None, None, Some("unknown driver"));
        return;
    };

    let image = if job.mode == ReachMode::Usb {
        Vec::new()
    } else {
        match std::fs::read(&job.path) {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => {
                rt.jobs.transition(
                    &id,
                    JobState::Failed,
                    None,
                    None,
                    Some("firmware file is empty"),
                );
                return;
            }
            Err(e) => {
                rt.jobs.transition(
                    &id,
                    JobState::Failed,
                    None,
                    None,
                    Some(&format!("read {}: {e}", job.path.display())),
                );
                return;
            }
        }
    };

    if job.mode != ReachMode::Usb {
        if image.len() as u64 > crate::jobs::MAX_FIRMWARE_BYTES {
            rt.jobs.transition(
                &id,
                JobState::Failed,
                None,
                None,
                Some("firmware image exceeds LongFred OTA slot (3.75 MiB)"),
            );
            return;
        }
        let header_n = image.len().min(16);
        match wp_link::classify_image(&job.path, &image[..header_n], image.len() as u64) {
            Ok(wp_link::ImageKind::AppBin { .. }) => {}
            Ok(_) => {
                rt.jobs.transition(
                    &id,
                    JobState::Failed,
                    None,
                    None,
                    Some("HTTP firmware needs a .app.bin ESP app image, not ELF or a merged dump"),
                );
                return;
            }
            Err(e) => {
                rt.jobs
                    .transition(&id, JobState::Failed, None, None, Some(&e));
                return;
            }
        }
    }

    rt.jobs
        .transition(&id, JobState::Writing, Some("write"), Some(0), None);

    let mut sink = JobProgressSink {
        jobs: &rt.jobs,
        id: &id,
    };
    let cancel = Arc::new(AtomicBool::new(false));

    let outcome = match job.mode {
        ReachMode::Usb => {
            let port = job
                .port
                .clone()
                .or_else(|| rt.cached(&snap.driver, &snap.key).map(|c| c.key))
                .unwrap_or_else(|| snap.key.clone());
            if port.is_empty() {
                rt.jobs.transition(
                    &id,
                    JobState::Failed,
                    None,
                    None,
                    Some("USB firmware update needs --port or scan --mode usb"),
                );
                return;
            }
            sink.step("write");
            sink.detail(&format!("espflash {port}"));
            if let Ok(mut f) = std::fs::File::open(&job.path) {
                let mut header = [0u8; 16];
                if let Ok(n) = std::io::Read::read(&mut f, &mut header) {
                    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                    if matches!(
                        wp_link::classify_image(&job.path, &header[..n], len),
                        Ok(wp_link::ImageKind::AppBin { .. })
                    ) {
                        sink.detail(
                            "USB .app.bin writes ota_0 only; first dual-slot install needs ELF + --partition-table",
                        );
                    }
                }
            }
            let table = job.partition_table.clone();
            let image_path = job.path.clone();
            let label = format!("espflash {port}");
            await_blocking_with_heartbeats(
                rt,
                &id,
                &mut sink,
                &label,
                Arc::clone(&cancel),
                move |cancel| {
                    wp_link::flash_usb(&port, &image_path, table.as_deref(), Some(cancel.as_ref()))
                        .map(|()| wp_core::Outcome {
                            restarted: true,
                            mismatches: Vec::new(),
                        })
                },
            )
            .await
        }
        ReachMode::Lan => {
            let host = job
                .host
                .clone()
                .or_else(|| rt.cached(&snap.driver, &snap.key).map(|c| c.key))
                .unwrap_or_else(|| snap.key.clone());
            if host.parse::<Ipv4Addr>().is_err() {
                rt.jobs.transition(
                    &id,
                    JobState::Failed,
                    None,
                    None,
                    Some("LAN firmware update needs an IPv4 --host or scan --mode lan key"),
                );
                return;
            }
            sink.step("write");
            sink.detail(&format!("{} bytes", image.len()));
            firmware_http_with_heartbeats(
                rt,
                &id,
                &mut sink,
                driver,
                image,
                &host,
                80,
                None,
                Arc::clone(&cancel),
            )
            .await
        }
        ReachMode::Z21 => {
            rt.jobs.transition(
                &id,
                JobState::Failed,
                None,
                None,
                Some("firmware update is not supported over Z21 LAN"),
            );
            return;
        }
        ReachMode::Ap => {
            let candidate = match rt.cached(&snap.driver, &snap.key) {
                Some(c) => c,
                None => {
                    rt.jobs.transition(
                        &id,
                        JobState::Failed,
                        None,
                        None,
                        Some("candidate not in scan cache; run scan first"),
                    );
                    return;
                }
            };
            let net = rt.effective_net(driver);
            rt.jobs
                .transition(&id, JobState::Joining, Some("join"), None, None);
            let _hold = RadioHold::new(rt);
            let mut radio = rt.radio.lock().await;
            let bssid = parse_bssid(candidate.bssid.as_deref());
            if let Err(e) = radio.connect_open(&candidate.ssid, bssid).await {
                rt.jobs.transition(
                    &id,
                    JobState::Failed,
                    Some("join"),
                    None,
                    Some(&e.to_string()),
                );
                let _ = radio.release().await;
                return;
            }
            if let Err(e) = radio.set_address(net.source, net.prefix).await {
                rt.jobs.transition(
                    &id,
                    JobState::Failed,
                    Some("join"),
                    None,
                    Some(&e.to_string()),
                );
                let _ = radio.release().await;
                return;
            }
            if let Err(e) = radio.link_up().await {
                rt.jobs.transition(
                    &id,
                    JobState::Failed,
                    Some("join"),
                    None,
                    Some(&e.to_string()),
                );
                let _ = radio.release().await;
                return;
            }
            drop(radio);
            rt.jobs
                .transition(&id, JobState::Writing, Some("write"), None, None);
            sink.step("write");
            sink.detail(&format!("{} bytes", image.len()));
            let result = firmware_http_with_heartbeats(
                rt,
                &id,
                &mut sink,
                driver,
                image,
                &net.host.to_string(),
                net.port,
                Some(SocketAddr::from((net.source, 0))),
                Arc::clone(&cancel),
            )
            .await;
            {
                let mut radio = rt.radio.lock().await;
                let _ = radio.release().await;
            }
            result
        }
    };

    finish_firmware_job(rt, &id, outcome);
}

#[allow(clippy::too_many_arguments)]
async fn firmware_http_with_heartbeats(
    rt: &Runtime,
    id: &JobId,
    sink: &mut JobProgressSink<'_>,
    driver: Driver,
    image: Vec<u8>,
    host: &str,
    port: u16,
    source: Option<SocketAddr>,
    cancel: Arc<AtomicBool>,
) -> Result<wp_core::Outcome, wp_core::DriverError> {
    let tokio_h = rt.handle();
    let registry = Arc::clone(&rt.registry);
    let client = make_firmware_http_client(host, port, source, Some(Arc::clone(&cancel)));
    await_blocking_with_heartbeats(rt, id, sink, "firmware http", cancel, move |_cancel| {
        let mut client = client;
        let mut nop = wp_core::NoProgress;
        let transport = Transport::Http(&mut client);
        tokio_h.block_on(registry.update_firmware(driver, transport, &image, &mut nop))
    })
    .await
}

/// Run blocking firmware work off the worker thread and keep `job.watch`
/// alive with a detail frame every [`crate::jobs::WATCH_HEARTBEAT`].
async fn await_blocking_with_heartbeats<T, F>(
    rt: &Runtime,
    id: &JobId,
    sink: &mut JobProgressSink<'_>,
    label: &str,
    cancel: Arc<AtomicBool>,
    work: F,
) -> Result<T, wp_core::DriverError>
where
    T: Send + 'static,
    F: FnOnce(Arc<AtomicBool>) -> Result<T, wp_core::DriverError> + Send + 'static,
{
    let mut handle = tokio::task::spawn_blocking({
        let cancel = Arc::clone(&cancel);
        move || work(cancel)
    });
    let started = Instant::now();
    loop {
        tokio::select! {
            biased;
            joined = &mut handle => {
                return joined.map_err(|e| {
                    wp_core::DriverError::Other(format!("firmware worker join: {e}"))
                })?;
            }
            _ = tokio::time::sleep(crate::jobs::WATCH_HEARTBEAT) => {
                if rt.jobs.is_cancelled(id) {
                    cancel.store(true, Ordering::Relaxed);
                    continue;
                }
                let secs = started.elapsed().as_secs();
                sink.detail(&format!("{label} ({secs}s)"));
            }
        }
    }
}

fn finish_firmware_job(
    rt: &Runtime,
    id: &JobId,
    outcome: Result<wp_core::Outcome, wp_core::DriverError>,
) {
    if rt
        .jobs
        .snapshot(id)
        .map(|s| s.state.is_terminal())
        .unwrap_or(false)
    {
        return;
    }
    if rt.jobs.is_cancelled(id) || matches!(outcome, Err(wp_core::DriverError::Cancelled)) {
        rt.jobs
            .transition(id, JobState::Cancelled, None, None, Some("cancelled"));
        return;
    }
    match outcome {
        Ok(o) => {
            let detail = if o.restarted { Some("restarted") } else { None };
            rt.jobs
                .transition(id, JobState::Done, Some("done"), Some(100), detail);
        }
        Err(e) => {
            rt.jobs
                .transition(id, JobState::Failed, None, None, Some(&e.to_string()));
        }
    }
}

fn make_firmware_http_client(
    host: &str,
    port: u16,
    source: Option<SocketAddr>,
    cancel: Option<Arc<AtomicBool>>,
) -> BoundedHttpClient {
    let mut c = BoundedHttpClient::new(host, port)
        .with_deadline(crate::jobs::FIRMWARE_DEADLINE)
        .with_retries(0);
    if let Some(src) = source {
        c = c.with_source(src);
    }
    if let Some(flag) = cancel {
        c = c.with_cancel(flag);
    }
    c
}

/// Helper used by tests / fake mode to wait briefly for frames.
pub fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}
