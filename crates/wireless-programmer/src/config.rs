//! Daemon configuration.

use std::net::SocketAddr;
use std::path::PathBuf;

/// Daemon configuration, resolved from CLI + environment.
#[derive(Debug, Clone)]
pub struct Config {
    /// Unix socket path.
    pub socket: PathBuf,
    /// Socket mode (permissions).
    pub socket_mode: u32,
    /// Users allowed to connect (login names), matched via SO_PEERCRED.
    pub allow_users: Vec<String>,
    /// Data directory (BIGFRED_DATA_DIR / DATA_DIR / /data).
    pub data_dir: PathBuf,
    /// Daemon version string.
    pub version: String,
    /// Git commit, when built with WIRELESS_PROGRAMMER_GIT_COMMIT.
    pub commit: Option<String>,
    /// Source address bound on the wireless interface during programming.
    pub source_addr: SocketAddr,
}

impl Default for Config {
    fn default() -> Self {
        let data_dir = resolve_data_dir();
        Self {
            socket: data_dir.join("run").join("wireless-programmer.sock"),
            socket_mode: 0o660,
            allow_users: resolve_allow_users(),
            data_dir,
            version: env!("CARGO_PKG_VERSION").into(),
            commit: option_env!("WIRELESS_PROGRAMMER_GIT_COMMIT").map(Into::into),
            source_addr: "192.168.4.2:0".parse().expect("valid default source addr"),
        }
    }
}

/// Resolve the peer allowlist. Defaults to `bigfred` and `bigfred-wizard`;
/// override with `WIRELESS_PROGRAMMER_ALLOW_USERS` (comma-separated login
/// names, replaces the default).
fn resolve_allow_users() -> Vec<String> {
    match std::env::var("WIRELESS_PROGRAMMER_ALLOW_USERS") {
        Ok(v) => v
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Into::into)
            .collect::<Vec<_>>(),
        Err(_) => vec!["bigfred".into(), "bigfred-wizard".into()],
    }
}

/// Resolve the BigFred data directory.
pub fn resolve_data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("BIGFRED_DATA_DIR") {
        return PathBuf::from(d);
    }
    if let Ok(d) = std::env::var("DATA_DIR") {
        return PathBuf::from(d);
    }
    PathBuf::from("/data")
}
