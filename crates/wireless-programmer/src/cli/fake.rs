//! Standalone Soft-AP HTTP mock (`wireless-programmer fake`).

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;

use super::{init_tracing, LogLevel};

/// `fake` arguments — runs only the Soft-AP HTTP mock (no daemon / radio / IPC).
#[derive(Debug, Parser)]
pub struct FakeArgs {
    /// Driver to emulate (`wifred` | `longfred`).
    #[arg(long)]
    pub driver: String,

    /// Bind address for the mock HTTP server.
    #[arg(long, default_value = "127.0.0.1:8070")]
    pub bind: SocketAddr,

    /// Verbose logging. Same as `--log-level debug` when `--log-level` is omitted.
    #[arg(short, long)]
    pub verbose: bool,

    /// Log filter: error, warn, info, debug, or trace.
    ///
    /// Overrides `-v` / `--verbose`. When omitted, `RUST_LOG` is honoured,
    /// then `info`.
    #[arg(long = "log-level", value_enum, value_name = "LEVEL")]
    pub log_level: Option<LogLevel>,
}

/// Run a standalone fake Soft-AP HTTP server until Ctrl-C.
pub fn run_fake(args: FakeArgs) -> ExitCode {
    init_tracing(args.verbose, args.log_level);

    let device: Arc<tokio::sync::Mutex<dyn wp_fake::FakeDevice>> = match args.driver.as_str() {
        "wifred" => Arc::new(tokio::sync::Mutex::new(wp_fake::WifredFake::new())),
        "longfred" => Arc::new(tokio::sync::Mutex::new(wp_fake::LongFredFake::new())),
        other => {
            tracing::error!("unknown driver {other:?}; expected wifred or longfred");
            return ExitCode::FAILURE;
        }
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match rt.block_on(async {
        let local = wp_fake::bind_and_serve(args.bind, device).await?;
        tracing::info!(
            "fake Soft-AP for driver={} listening on {local} (Ctrl-C to stop)",
            args.driver
        );
        tokio::signal::ctrl_c().await?;
        Ok::<(), std::io::Error>(())
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("fake server: {e}");
            ExitCode::FAILURE
        }
    }
}
