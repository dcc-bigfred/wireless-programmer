//! Daemon subcommand runner (the previous `main` behaviour).

use std::path::PathBuf;

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
}

/// Run the IPC daemon until shutdown.
pub fn run_daemon(args: DaemonArgs, socket_override: Option<PathBuf>) -> std::process::ExitCode {
    let mut cfg = Config::default();
    if let Some(s) = socket_override {
        cfg.socket = s;
    }

    let filter = if args.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let registry = DriverRegistry::new();
    let runtime = Server::new(cfg, registry);

    match runtime.run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("fatal: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
