//! Daemon subcommand runner (the previous `main` behaviour).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::drivers::DriverRegistry;
use crate::ipc::Server;

/// `daemon` arguments.
#[derive(Debug, Clone, Default, Args)]
pub struct DaemonArgs {
    /// Verbose logging.
    #[arg(short, long)]
    pub verbose: bool,

    /// Wireless interface to use (e.g. `wlan0`, `wlp2s0`).
    ///
    /// When omitted, the first wireless interface under `/sys/class/net` is
    /// selected. Overrides `WIRELESS_PROGRAMMER_INTERFACE` when set.
    #[arg(short = 'i', long = "interface", value_name = "IFACE")]
    pub interface: Option<String>,
}

/// Run the IPC daemon until shutdown.
pub fn run_daemon(args: DaemonArgs, socket_override: Option<PathBuf>) -> ExitCode {
    let filter = if args.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let mut cfg = Config::default();
    if let Some(s) = socket_override {
        cfg.socket = s;
    }
    // CLI wins over the environment default baked into Config::default.
    if let Some(iface) = args.interface {
        let iface = iface.trim().to_string();
        if iface.is_empty() {
            tracing::error!("--interface must not be empty");
            return ExitCode::FAILURE;
        }
        cfg.interface = Some(iface);
    }

    // Validate the preferred interface early so a typo fails at start-up
    // rather than on the first scan/program request.
    if let Some(ref name) = cfg.interface {
        match wp_link::resolve_wireless_interface(Some(name)) {
            Ok(resolved) => cfg.interface = Some(resolved),
            Err(e) => {
                tracing::error!("wireless interface: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    match &cfg.interface {
        Some(name) => tracing::info!("wireless interface: {name}"),
        None => tracing::info!("wireless interface: auto (first wireless)"),
    }

    let registry = DriverRegistry::new();
    let runtime = Server::new(cfg, registry);

    match runtime.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("fatal: {e}");
            ExitCode::FAILURE
        }
    }
}
