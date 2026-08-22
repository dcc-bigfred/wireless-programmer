//! Daemon subcommand runner (the previous `main` behaviour).

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Args;
use wp_link::{Nl80211Radio, Radio};

use crate::config::Config;
use crate::drivers::DriverRegistry;
use crate::ipc::Server;
use crate::jobs::JobRegistry;
use crate::runtime::Runtime;

use super::{init_tracing, LogLevel};

/// `daemon` arguments.
#[derive(Debug, Clone, Default, Args)]
pub struct DaemonArgs {
    /// Verbose logging. Same as `--log-level debug` when `--log-level` is omitted.
    #[arg(short, long)]
    pub verbose: bool,

    /// Log filter: error, warn, info, debug, or trace.
    ///
    /// Overrides `-v` / `--verbose`. When omitted, `RUST_LOG` is honoured,
    /// then `info`.
    #[arg(long = "log-level", value_enum, value_name = "LEVEL")]
    pub log_level: Option<LogLevel>,

    /// Wireless interface to use (e.g. `wlan0`, `wlp2s0`).
    ///
    /// When omitted, the first wireless interface under `/sys/class/net` is
    /// selected. Overrides `WIRELESS_PROGRAMMER_INTERFACE` when set.
    ///
    /// The special value `fake` enables an in-process fake radio and Soft-AP
    /// HTTP mock (one candidate per driver) without real WiFi hardware.
    #[arg(short = 'i', long = "interface", value_name = "IFACE")]
    pub interface: Option<String>,

    /// Require SO_PEERCRED peer authentication against the allowlist.
    ///
    /// Off by default. Also enabled by `WIRELESS_PROGRAMMER_REQUIRE_AUTH=1`.
    #[arg(long = "require-auth")]
    pub require_auth: bool,

    /// Comma-separated login names allowed when `--require-auth` is set.
    /// Defaults to `bigfred,bigfred-wizard`. Overrides
    /// `WIRELESS_PROGRAMMER_ALLOW_USERS`.
    #[arg(long = "allow-users", value_name = "USERS")]
    pub allow_users: Option<String>,

    /// Listen port for the in-process fake Soft-AP HTTP server when
    /// `--interface fake`. Default 8070. Use `0` for an ephemeral port.
    #[arg(long = "fake-webserver-port", value_name = "PORT")]
    pub fake_webserver_port: Option<u16>,
}

/// Run the IPC daemon until shutdown.
pub fn run_daemon(args: DaemonArgs, socket_override: Option<PathBuf>) -> ExitCode {
    init_tracing(args.verbose, args.log_level);

    let mut cfg = Config::default();
    if let Some(s) = socket_override {
        cfg.socket = s;
    }
    if let Some(iface) = args.interface {
        let iface = iface.trim().to_string();
        if iface.is_empty() {
            tracing::error!("--interface must not be empty");
            return ExitCode::FAILURE;
        }
        cfg.interface = Some(iface);
    }
    if let Some(port) = args.fake_webserver_port {
        cfg.fake_webserver_port = Some(port);
    }
    if args.require_auth {
        cfg.require_auth = true;
    }
    if let Some(list) = args.allow_users {
        cfg.allow_users = list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Into::into)
            .collect();
        if !cfg.allow_users.is_empty() {
            cfg.require_auth = true;
        }
    }

    let fake = cfg.is_fake_radio();
    if fake && cfg.require_auth {
        tracing::warn!("fake radio mode: forcing peer auth off");
        cfg.require_auth = false;
    }
    cfg.finalize_auth();

    if !fake {
        if let Some(ref name) = cfg.interface {
            match wp_link::resolve_wireless_interface(Some(name)) {
                Ok(resolved) => cfg.interface = Some(resolved),
                Err(e) => {
                    tracing::error!("wireless interface: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    match &cfg.interface {
        Some(name) if name == "fake" => {
            tracing::info!("wireless interface: fake (in-process mock)")
        }
        Some(name) => tracing::info!("wireless interface: {name}"),
        None => tracing::info!("wireless interface: auto (first wireless)"),
    }
    if cfg.require_auth {
        tracing::info!(
            "peer auth: enabled (allow_users={:?}, mode={:o})",
            cfg.allow_users,
            cfg.socket_mode
        );
    } else {
        tracing::info!(
            "peer auth: disabled (socket mode {:o}; any local peer may connect)",
            cfg.socket_mode
        );
    }

    // For fake mode, bind the HTTP mock first so we know the port (supports 0).
    let fake_listener = if fake {
        let want = cfg.fake_webserver_port.unwrap_or(8070);
        let bind = SocketAddr::from((Ipv4Addr::LOCALHOST, want));
        match std::net::TcpListener::bind(bind) {
            Ok(l) => {
                if let Err(e) = l.set_nonblocking(true) {
                    tracing::error!("fake webserver: {e}");
                    return ExitCode::FAILURE;
                }
                match l.local_addr() {
                    Ok(addr) => {
                        cfg.commissioning_net_override =
                            Some(Config::localhost_commissioning(addr.port()));
                        tracing::info!("fake Soft-AP HTTP mock will listen on {addr}");
                        Some(l)
                    }
                    Err(e) => {
                        tracing::error!("fake webserver: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            Err(e) => {
                tracing::error!("fake webserver bind: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };

    let registry = DriverRegistry::new();
    let jobs = JobRegistry::new();

    let radio: Box<dyn Radio> = if fake {
        Box::new(wp_fake::FakeRadio::one_per_driver())
    } else {
        match Nl80211Radio::with_interface_opt(cfg.interface.as_deref()) {
            Ok(r) => Box::new(r),
            Err(e) => {
                tracing::error!("radio open: {e}");
                return ExitCode::FAILURE;
            }
        }
    };

    let runtime = match Runtime::new(cfg, registry, jobs, radio) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(std_listener) = fake_listener {
        if let Err(e) = spawn_fake_from_std_listener(&runtime, std_listener) {
            tracing::error!("fake webserver: {e}");
            return ExitCode::FAILURE;
        }
    }

    match Server::new(runtime).run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("fatal: {e}");
            ExitCode::FAILURE
        }
    }
}

fn spawn_fake_from_std_listener(
    runtime: &Arc<Runtime>,
    std_listener: std::net::TcpListener,
) -> Result<(), String> {
    let device: Arc<tokio::sync::Mutex<dyn wp_fake::FakeDevice>> =
        Arc::new(tokio::sync::Mutex::new(wp_fake::CompositeFakeDevice::all()));
    // `TcpListener::from_std` needs a Tokio reactor — enter via the daemon runtime.
    runtime.handle().block_on(async move {
        let listener =
            tokio::net::TcpListener::from_std(std_listener).map_err(|e| e.to_string())?;
        tokio::spawn(async move {
            if let Err(e) = wp_fake::FakeHttpServer::serve(listener, device).await {
                tracing::error!("fake Soft-AP HTTP mock stopped: {e}");
            }
        });
        Ok::<(), String>(())
    })
}
