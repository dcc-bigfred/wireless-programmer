//! Clap subcommand definitions and dispatch.

mod client;
mod daemon;
mod fake;
mod program;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::EnvFilter;
use wp_client::ClientError;

pub use daemon::{run_daemon, DaemonArgs};
pub use fake::{run_fake, FakeArgs};

/// Log filter for the daemon and `fake` subcommands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum LogLevel {
    /// `error` — failures only.
    Error,
    /// `warn` — failures and recoverable problems.
    Warn,
    /// `info` — default operational log.
    Info,
    /// `debug` — radio scan BSS, trigger details (`-v`).
    Debug,
    /// `trace` — most verbose.
    Trace,
}

impl LogLevel {
    /// EnvFilter directive for this level.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

/// Explicit filter from `--log-level` / `-v`, or `None` to honour `RUST_LOG`.
#[must_use]
pub fn log_filter_directive(verbose: bool, log_level: Option<LogLevel>) -> Option<&'static str> {
    if let Some(level) = log_level {
        Some(level.as_str())
    } else if verbose {
        Some("debug")
    } else {
        None
    }
}

/// Build the tracing EnvFilter: `--log-level`, else `-v` → debug, else `RUST_LOG`, else `info`.
#[must_use]
pub fn resolve_env_filter(verbose: bool, log_level: Option<LogLevel>) -> EnvFilter {
    match log_filter_directive(verbose, log_level) {
        Some(d) => EnvFilter::new(d),
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
    }
}

/// Initialise `tracing-subscriber` (and the `log` crate bridge).
pub fn init_tracing(verbose: bool, log_level: Option<LogLevel>) {
    tracing_subscriber::fmt()
        .with_env_filter(resolve_env_filter(verbose, log_level))
        .init();
}

/// Command-line interface.
#[derive(Debug, Parser)]
#[command(
    name = "wireless-programmer",
    about = "Discover and program physical throttle hardware for BigFred",
    version
)]
pub struct Cli {
    /// Subcommand to run. Omit to start the daemon.
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Override the daemon socket path (applies to every subcommand).
    #[arg(long, global = true)]
    pub socket: Option<PathBuf>,

    /// Verbose logging (daemon / fake only). Same as `--log-level debug`
    /// when `--log-level` is omitted.
    #[arg(short, long)]
    pub verbose: bool,

    /// Log filter (daemon / fake only): error, warn, info, debug, or trace.
    ///
    /// Overrides `-v` / `--verbose`. When omitted, `RUST_LOG` is honoured,
    /// otherwise `info`.
    #[arg(long = "log-level", value_enum, value_name = "LEVEL")]
    pub log_level: Option<LogLevel>,

    /// Wireless interface for the daemon (e.g. `wlan0`). Also accepted on
    /// `daemon --interface`. Overrides `WIRELESS_PROGRAMMER_INTERFACE`.
    #[arg(short = 'i', long = "interface", value_name = "IFACE")]
    pub interface: Option<String>,

    /// Require SO_PEERCRED peer authentication (daemon only). Also accepted
    /// on `daemon --require-auth`.
    #[arg(long = "require-auth")]
    pub require_auth: bool,

    /// Comma-separated allowlist when peer auth is on (daemon only).
    #[arg(long = "allow-users", value_name = "USERS")]
    pub allow_users: Option<String>,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the IPC daemon (default when no subcommand is given).
    Daemon(DaemonArgs),
    /// Enumerate candidate devices on the radio (or LAN mDNS).
    Scan(ScanArgs),
    /// Read a single candidate's device info.
    Probe(ProbeArgs),
    /// Start a programming job and stream its progress.
    Program(ProgramArgs),
    /// Upload firmware (`.app.bin`) over HTTP Soft-AP or LAN.
    UpdateFirmware(UpdateFirmwareArgs),
    /// Blink a device's LED so an operator can find it.
    Identify(IdentifyArgs),
    /// Report radio/link state.
    LinkStatus(CommonArgs),
    /// Exchange version + driver capabilities.
    Hello(CommonArgs),
    /// Inspect or control a running job.
    Job(JobArgs),
    /// Run a standalone Soft-AP HTTP mock for one driver (no daemon).
    Fake(FakeArgs),
}

/// Shared client-side flags.
#[derive(Debug, Clone, Parser)]
pub struct ClientCommon {
    /// Emit machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    pub json: bool,
    /// Per-operation timeout (e.g. `30s`). Default 10s; `update-firmware`
    /// uses 180s (USB) or 120s (HTTP) when omitted.
    #[arg(long, global = true)]
    pub timeout: Option<humantime::Duration>,
}

/// Arguments for subcommands that take only the shared client flags
/// (`link-status`, `hello`).
#[derive(Debug, Parser)]
pub struct CommonArgs {
    #[command(flatten)]
    pub common: ClientCommon,
}

/// `scan` arguments.
#[derive(Debug, Parser)]
pub struct ScanArgs {
    #[command(flatten)]
    pub common: ClientCommon,
    /// `ap` (Soft-AP radio, default), `lan` (mDNS `_longfred-ota._tcp`), `usb`, or `z21`.
    #[arg(long, default_value = "ap", value_parser = ["ap", "lan", "usb", "z21"])]
    pub mode: String,
}

/// `update-firmware` arguments.
#[derive(Debug, Parser)]
pub struct UpdateFirmwareArgs {
    #[command(flatten)]
    pub common: ClientCommon,
    /// `ap` (Soft-AP, default), `lan` (layout Wi‑Fi), or `usb` (`espflash`).
    #[arg(long, default_value = "ap", value_parser = ["ap", "lan", "usb"])]
    pub mode: String,
    /// Driver identifier (default `longfred`).
    #[arg(long, default_value = "longfred")]
    pub driver: String,
    /// Candidate key (BSSID in AP mode, IPv4 in LAN mode, serial device in USB mode).
    #[arg(long)]
    pub key: Option<String>,
    /// LAN IPv4 (skips mDNS). Implies `--mode lan` when set alone with `--file`.
    #[arg(long)]
    pub host: Option<String>,
    /// USB serial device (e.g. `/dev/ttyACM0`). Implies `--mode usb`.
    #[arg(long)]
    pub port: Option<String>,
    /// CSV partition table for ELF USB flashes (default: `partitions.csv` next to `--file`).
    #[arg(long)]
    pub partition_table: Option<PathBuf>,
    /// Path to a LongFred image (`.app.bin`, merged `.bin`, or ELF).
    #[arg(long)]
    pub file: PathBuf,
    /// Do not stream job progress after starting the job.
    #[arg(long)]
    pub no_watch: bool,
}

/// `probe` arguments.
#[derive(Debug, Parser)]
pub struct ProbeArgs {
    #[command(flatten)]
    pub common: ClientCommon,
    /// Driver identifier (e.g. `wifred`).
    #[arg(long)]
    pub driver: String,
    /// Candidate key (e.g. BSSID for WiFred).
    #[arg(long)]
    pub key: String,
}

/// `program` arguments.
#[derive(Debug, Parser)]
pub struct ProgramArgs {
    #[command(flatten)]
    pub common: ClientCommon,
    /// Driver identifier (e.g. `wifred`, `longfred`, `fred`).
    #[arg(long)]
    pub driver: String,
    /// Candidate key.
    #[arg(long)]
    pub key: String,
    /// Load the full `ProgramRequest` body from a JSON file. Overrides the
    /// individual `--identity`/`--wifi-*`/`--server-*`/`--roster-file` flags.
    #[arg(long)]
    pub request_file: Option<PathBuf>,
    /// Opaque device identity (e.g. 6-digit BigFred pairing code for WiFred).
    #[arg(long)]
    pub identity: Option<String>,
    /// WiFi SSID the device should join after programming.
    #[arg(long)]
    pub wifi_ssid: Option<String>,
    /// WiFi PSK (WPA2 passphrase). Visible in `/proc/<pid>/cmdline` and shell
    /// history — prefer `--wifi-psk-file` on a shared machine.
    #[arg(long, conflicts_with = "wifi_psk_file")]
    pub wifi_psk: Option<String>,
    /// Read the WiFi PSK from a file (trailing newline stripped). Use `-` to
    /// read it from stdin.
    #[arg(long)]
    pub wifi_psk_file: Option<PathBuf>,
    /// wiThrottle server host. Required unless `--server-automatic` is set.
    #[arg(long)]
    pub server_host: Option<String>,
    /// wiThrottle server port. Required unless `--server-automatic` is set,
    /// which defaults it to the wiThrottle port 12090.
    #[arg(long)]
    pub server_port: Option<u16>,
    /// Discover the wiThrottle server via mDNS instead of a fixed host.
    #[arg(long)]
    pub server_automatic: bool,
    /// JSON file holding the roster array (`RosterEntry[]`).
    #[arg(long)]
    pub roster_file: Option<PathBuf>,
    /// Do not stream job progress after starting the job.
    #[arg(long)]
    pub no_watch: bool,
}

/// `identify` arguments.
#[derive(Debug, Parser)]
pub struct IdentifyArgs {
    #[command(flatten)]
    pub common: ClientCommon,
    /// Driver identifier.
    #[arg(long)]
    pub driver: String,
    /// Candidate key.
    #[arg(long)]
    pub key: String,
    /// Number of blinks (driver default applies when omitted).
    #[arg(long)]
    pub count: Option<u32>,
}

/// `job` arguments.
#[derive(Debug, Parser)]
pub struct JobArgs {
    #[command(flatten)]
    pub common: ClientCommon,
    #[command(subcommand)]
    pub action: JobAction,
}

/// `job` subcommands.
#[derive(Debug, Subcommand)]
pub enum JobAction {
    /// Snapshot a job's state.
    Get {
        #[arg(long)]
        id: String,
    },
    /// Stream a job's progress until it terminates.
    Watch {
        #[arg(long)]
        id: String,
    },
    /// Request cancellation of a running job.
    Cancel {
        #[arg(long)]
        id: String,
    },
}

/// Errors surfaced by the client subcommands.
///
/// Kept separate from [`wp_client::ClientError`] so that a local problem (a
/// missing flag, an unreadable file) is not reported as if the daemon had
/// misbehaved.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Missing or contradictory flags.
    #[error("{0}")]
    Usage(String),
    /// A `--request-file` / `--roster-file` / `--wifi-psk-file` could not be
    /// read or decoded.
    #[error("{path}: {message}")]
    File {
        /// Path as given on the command line.
        path: String,
        /// Underlying read or decode failure.
        message: String,
    },
    /// The job reached a terminal state other than `done`.
    #[error("job {state}{detail}")]
    Job {
        /// Terminal state (`failed` / `cancelled`).
        state: String,
        /// Daemon-supplied detail, pre-formatted with a leading separator.
        detail: String,
    },
    /// The watch stream closed before the job reached a terminal state.
    #[error("job stream closed before a terminal state was reached")]
    Truncated,
    /// The daemon reported an error, or the transport failed.
    #[error(transparent)]
    Client(#[from] ClientError),
}

/// Resolve the socket path from CLI override or the environment.
pub(crate) fn resolve_socket(cli_socket: Option<&PathBuf>) -> PathBuf {
    if let Some(s) = cli_socket {
        return s.clone();
    }
    wp_client::Client::resolve_socket()
}

/// Build a client from the resolved socket + timeout.
pub(crate) fn build_client(
    socket: &Path,
    timeout: Option<humantime::Duration>,
) -> wp_client::Client {
    let mut c = wp_client::Client::new(socket);
    if let Some(t) = timeout {
        // Deref, not `from_secs(as_secs())`: truncating to whole seconds turns
        // `--timeout 500ms` into a zero timeout, which the kernel rejects and
        // which therefore silently means "block forever".
        c = c.with_timeout(*t);
    }
    c
}

/// Dispatch a client subcommand.
pub fn run_client(command: Command, socket: Option<PathBuf>) -> std::process::ExitCode {
    client::run(command, socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_level_overrides_verbose() {
        assert_eq!(
            log_filter_directive(true, Some(LogLevel::Warn)),
            Some("warn")
        );
        assert_eq!(
            log_filter_directive(false, Some(LogLevel::Trace)),
            Some("trace")
        );
    }

    #[test]
    fn verbose_is_debug_when_no_log_level() {
        assert_eq!(log_filter_directive(true, None), Some("debug"));
        assert_eq!(log_filter_directive(false, None), None);
    }

    #[test]
    fn log_level_as_str_matches_envfilter() {
        for level in [
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            let filter = resolve_env_filter(false, Some(level));
            assert_eq!(filter.to_string(), level.as_str());
        }
    }
}
