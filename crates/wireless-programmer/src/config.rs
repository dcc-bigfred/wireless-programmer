//! Daemon configuration.

use std::net::Ipv4Addr;
use std::path::PathBuf;

use wp_core::CommissioningNet;

/// Daemon configuration, resolved from CLI + environment.
#[derive(Debug, Clone)]
pub struct Config {
    /// Unix socket path.
    pub socket: PathBuf,
    /// Socket mode (permissions).
    pub socket_mode: u32,
    /// When `true`, enforce [`Self::allow_users`] via `SO_PEERCRED`.
    /// Off by default (open to any local peer that can open the socket).
    pub require_auth: bool,
    /// Users allowed to connect (login names), matched via SO_PEERCRED.
    /// Used only when [`Self::require_auth`] is `true`.
    pub allow_users: Vec<String>,
    /// Login name whose primary group owns the socket. `None` means "use the
    /// first entry of `allow_users`" when auth is on, matching microinit's
    /// `socketAllowUsers` model. Without a group owner a `0660` socket is
    /// unreachable for every allowlisted peer, since DAC rejects `connect(2)`
    /// before `SO_PEERCRED`.
    pub socket_group_user: Option<String>,
    /// Data directory (BIGFRED_DATA_DIR / DATA_DIR / /data).
    pub data_dir: PathBuf,
    /// Daemon version string.
    pub version: String,
    /// Git commit, when built with WIRELESS_PROGRAMMER_GIT_COMMIT.
    pub commit: Option<String>,
    /// Wireless interface to use (`wlan0`, `wlp2s0`, …). `None` means auto-
    /// select the first wireless interface at radio open time. The special
    /// value `"fake"` enables in-process fake radio + HTTP device mock.
    pub interface: Option<String>,
    /// When set, override driver Soft-AP addressing (fake mode points at
    /// `127.0.0.1:port`).
    pub commissioning_net_override: Option<CommissioningNet>,
    /// Listen port for the in-process fake HTTP server when
    /// `interface == "fake"`. `None` defaults to 8070; `Some(0)` asks the OS
    /// for an ephemeral port.
    pub fake_webserver_port: Option<u16>,
}

impl Default for Config {
    fn default() -> Self {
        let data_dir = resolve_data_dir();
        let require_auth = resolve_require_auth();
        let allow_users = resolve_allow_users(require_auth);
        let socket_mode = if require_auth { 0o660 } else { 0o666 };
        Self {
            socket: data_dir
                .join("run")
                .join("wireless-programmer")
                .join("wireless-programmer.sock"),
            socket_mode,
            require_auth,
            allow_users,
            socket_group_user: std::env::var("WIRELESS_PROGRAMMER_SOCKET_GROUP_USER")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            data_dir,
            version: resolve_version(),
            commit: resolve_commit(),
            interface: resolve_interface_env(),
            commissioning_net_override: None,
            fake_webserver_port: resolve_fake_web_port_env(),
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
    /// Apply auth-related settings after CLI overrides. Keeps socket mode in
    /// sync with [`Self::require_auth`] and fills the default allowlist when
    /// auth is enabled without an explicit list.
    pub fn finalize_auth(&mut self) {
        if self.require_auth && self.allow_users.is_empty() {
            self.allow_users = default_allow_users();
        }
        if !self.require_auth {
            // Open socket when peer auth is off — any local process may connect.
            self.socket_mode = 0o666;
        } else if self.socket_mode == 0o666 {
            self.socket_mode = 0o660;
        }
    }

    /// Whether this config requests fake radio mode (`--interface fake`).
    #[must_use]
    pub fn is_fake_radio(&self) -> bool {
        self.interface.as_deref() == Some("fake")
    }

    /// Login name whose primary group should own the socket: the explicit
    /// override when set, otherwise the first allowlist entry (auth on only).
    #[must_use]
    pub fn socket_group_owner(&self) -> Option<&str> {
        if let Some(ref u) = self.socket_group_user {
            return Some(u.as_str());
        }
        if self.require_auth {
            self.allow_users.first().map(String::as_str)
        } else {
            None
        }
    }

    /// Build a local commissioning override pointing at `127.0.0.1:port`.
    #[must_use]
    pub fn localhost_commissioning(port: u16) -> CommissioningNet {
        CommissioningNet {
            host: Ipv4Addr::LOCALHOST,
            port,
            source: Ipv4Addr::LOCALHOST,
            prefix: 8,
        }
    }
}

/// `WIRELESS_PROGRAMMER_REQUIRE_AUTH` — truthy values enable peer auth.
/// Default: off.
fn resolve_require_auth() -> bool {
    match std::env::var("WIRELESS_PROGRAMMER_REQUIRE_AUTH") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        }
        Err(_) => false,
    }
}

fn default_allow_users() -> Vec<String> {
    vec!["bigfred".into(), "bigfred-wizard".into()]
}

/// Resolve the peer allowlist. When auth is off, returns empty (unused).
/// When auth is on: `WIRELESS_PROGRAMMER_ALLOW_USERS` or the BigFred defaults.
fn resolve_allow_users(require_auth: bool) -> Vec<String> {
    match std::env::var("WIRELESS_PROGRAMMER_ALLOW_USERS") {
        Ok(v) => {
            let list = v
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(Into::into)
                .collect::<Vec<_>>();
            if require_auth && list.is_empty() {
                default_allow_users()
            } else {
                list
            }
        }
        Err(_) if require_auth => default_allow_users(),
        Err(_) => Vec::new(),
    }
}

/// Optional wireless interface from `WIRELESS_PROGRAMMER_INTERFACE`.
fn resolve_interface_env() -> Option<String> {
    std::env::var("WIRELESS_PROGRAMMER_INTERFACE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn resolve_fake_web_port_env() -> Option<u16> {
    std::env::var("WIRELESS_PROGRAMMER_FAKE_WEB_PORT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalize_auth_fills_default_allowlist() {
        let mut cfg = Config {
            require_auth: true,
            allow_users: Vec::new(),
            socket_mode: 0o666,
            ..Config::default()
        };
        cfg.finalize_auth();
        assert_eq!(cfg.allow_users, default_allow_users());
        assert_eq!(cfg.socket_mode, 0o660);
    }

    #[test]
    fn finalize_auth_opens_socket_when_auth_off() {
        let mut cfg = Config {
            require_auth: false,
            allow_users: default_allow_users(),
            socket_mode: 0o660,
            ..Config::default()
        };
        cfg.finalize_auth();
        assert_eq!(cfg.socket_mode, 0o666);
    }

    #[test]
    fn socket_group_owner_none_when_auth_off() {
        let cfg = Config {
            require_auth: false,
            allow_users: default_allow_users(),
            socket_group_user: None,
            ..Config::default()
        };
        assert_eq!(cfg.socket_group_owner(), None);
    }

    #[test]
    fn socket_group_owner_uses_allowlist_when_auth_on() {
        let cfg = Config {
            require_auth: true,
            allow_users: vec!["bigfred".into()],
            socket_group_user: None,
            ..Config::default()
        };
        assert_eq!(cfg.socket_group_owner(), Some("bigfred"));
    }

    #[test]
    fn is_fake_radio_detects_interface() {
        let cfg = Config {
            interface: Some("fake".into()),
            ..Config::default()
        };
        assert!(cfg.is_fake_radio());
    }
}
