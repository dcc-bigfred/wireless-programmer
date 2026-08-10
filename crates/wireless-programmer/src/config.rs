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
    /// Login name whose primary group owns the socket. `None` means "use the
    /// first entry of `allow_users`", matching microinit's `socketAllowUsers`
    /// model. Without a group owner a `0660` socket is unreachable for every
    /// allowlisted peer, since DAC rejects `connect(2)` before `SO_PEERCRED`.
    pub socket_group_user: Option<String>,
    /// Data directory (BIGFRED_DATA_DIR / DATA_DIR / /data).
    pub data_dir: PathBuf,
    /// Daemon version string.
    pub version: String,
    /// Git commit, when built with WIRELESS_PROGRAMMER_GIT_COMMIT.
    pub commit: Option<String>,
    /// Source address bound on the wireless interface during programming.
    pub source_addr: SocketAddr,
    /// Wireless interface to use (`wlan0`, `wlp2s0`, …). `None` means auto-
    /// select the first wireless interface at radio open time.
    pub interface: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        let data_dir = resolve_data_dir();
        Self {
            socket: data_dir
                .join("run")
                .join("wireless-programmer")
                .join("wireless-programmer.sock"),
            socket_mode: 0o660,
            allow_users: resolve_allow_users(),
            socket_group_user: std::env::var("WIRELESS_PROGRAMMER_SOCKET_GROUP_USER")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            data_dir,
            version: resolve_version(),
            commit: resolve_commit(),
            source_addr: "192.168.4.2:0".parse().expect("valid default source addr"),
            interface: resolve_interface_env(),
        }
    }
}

/// Prefer the release tag from `.wireless-programmer.version` when present;
/// otherwise the Cargo package version (local / CI builds without inject).
fn resolve_version() -> String {
    let i = crate::version::info();
    if i.version != "dev" {
        i.version
    } else {
        env!("CARGO_PKG_VERSION").into()
    }
}

/// Prefer the ELF tag commit when present; otherwise the build-time env.
fn resolve_commit() -> Option<String> {
    let i = crate::version::info();
    if !i.tag_commit.is_empty() {
        Some(i.tag_commit)
    } else if i.build_commit != "unknown" {
        Some(i.build_commit)
    } else {
        None
    }
}

impl Config {
    /// Login name whose primary group should own the socket: the explicit
    /// override when set, otherwise the first allowlist entry.
    #[must_use]
    pub fn socket_group_owner(&self) -> Option<&str> {
        self.socket_group_user
            .as_deref()
            .or_else(|| self.allow_users.first().map(String::as_str))
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

/// Optional wireless interface from `WIRELESS_PROGRAMMER_INTERFACE`.
fn resolve_interface_env() -> Option<String> {
    std::env::var("WIRELESS_PROGRAMMER_INTERFACE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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
