//! `wireless-programmer` daemon.
//!
//! Discovers and programs physical throttle hardware for BigFred. Exposes a
//! length-prefixed-JSON Unix socket API (matching `microinit`'s convention)
//! consumed by `bigfred` / `bigfred-wizard`.
//!
//! The radio/worker loop that actually drives a device is hardware-gated and
//! not exercised in CI; its state-machine and config surface are therefore
//! allowed to be unused until wired to a live adapter.

#![forbid(unsafe_code)]
#![allow(dead_code)]

mod config;
mod drivers;
mod ipc;
mod jobs;

use std::path::PathBuf;

use clap::Parser;
use config::Config;
use tracing_subscriber::EnvFilter;

/// Command-line arguments.
#[derive(Debug, Parser)]
#[command(
    name = "wireless-programmer",
    about = "Discover and program physical throttle hardware for BigFred",
    version
)]
struct Args {
    /// Unix socket path.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Verbose logging.
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> std::process::ExitCode {
    let args = Args::parse();
    let mut cfg = Config::default();
    if let Some(s) = args.socket {
        cfg.socket = s;
    }

    let filter = if args.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let registry = drivers::DriverRegistry::new();
    let runtime = ipc::Server::new(cfg, registry);

    match runtime.run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("fatal: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
