//! Interface-scoped IPv4 settings needed when a device Soft-AP shares a
//! subnet with the hub's own LAN.
//!
//! Historically the LongFred Soft-AP served `192.168.0.1/24`, the same
//! address as the BigFred hub LAN. Current LongFred firmware uses
//! `192.168.4.1/24` (no overlap). The helpers below still apply whenever
//! `host` is a locally-owned address — three kernel behaviours then break
//! the HTTP conversation:
//!
//! 1. **Outbound SYN** — a route lookup for the Soft-AP IP hits the `local`
//!    table first (pref 0) and delivers to loopback. `SO_BINDTODEVICE` does
//!    **not** override that: `from all lookup local` has no oif filter.
//!    ICMP "success" with a local route is the hub answering its own address.
//!    Fixed by parking `lookup local` at a later preference and installing a
//!    more specific rule at pref 0: `from <source> to <host> lookup <table>`.
//!    Do **not** combine that with `SO_BINDTODEVICE`: the SYN-ACK source is
//!    still a local address, so the kernel may demux it on `lo` and RST.
//! 2. **Inbound SYN-ACK** — the reply arrives on the wireless interface with
//!    a source address that the host owns on another interface. The kernel
//!    treats that as a *martian source* and drops it. Fixed by
//!    `accept_local=1` on the wireless device (and a permissive `rp_filter`).
//! 3. **The ACK and the rest of the TCP flow** then follow the same pref-0
//!    rule out the wireless interface, instead of `lo`.
//!
//! [`prepare_softap`] applies (2). [`install_policy_route`] applies (1) and
//! (3); [`remove_policy_route`] puts the fib rules back.

use std::io;
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::process::Command;

/// Accept replies whose source address is owned by this host.
const ACCEPT_LOCAL: &str = "accept_local";
/// Reverse-path filter; its check fails for a locally-owned source.
const RP_FILTER: &str = "rp_filter";
/// Routing table for the Soft-AP exception. Must not collide with 253–255
/// (default / main / local).
const SOFTAP_TABLE: u32 = 100;
/// Preference of the exception rule. Occupies the slot `lookup local`
/// normally uses, so it is consulted first.
const SOFTAP_RULE_PREF: u32 = 0;
/// Where `lookup local` is parked while the exception is installed.
/// Immediately after pref 0, so every other local address still works.
const LOCAL_PARK_PREF: u32 = 1;

/// Path of an interface-scoped IPv4 sysctl.
fn conf_path(dev: &str, key: &str) -> PathBuf {
    PathBuf::from("/proc/sys/net/ipv4/conf").join(dev).join(key)
}

/// Read an interface-scoped IPv4 sysctl, or `None` when it is unreadable.
#[must_use]
pub fn read_conf(dev: &str, key: &str) -> Option<String> {
    std::fs::read_to_string(conf_path(dev, key))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Write an interface-scoped IPv4 sysctl.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] when the sysctl is missing or the
/// process lacks permission.
pub fn write_conf(dev: &str, key: &str, value: &str) -> io::Result<()> {
    std::fs::write(conf_path(dev, key), format!("{value}\n"))
}

/// Previous sysctl values captured by [`prepare_softap`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SavedConf {
    /// `(device, key, old_value)` for each changed sysctl.
    sysctls: Vec<(String, String, String)>,
}

impl SavedConf {
    /// `true` when nothing was changed (so [`restore`] is a no-op).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sysctls.is_empty()
    }
}

/// Fib-rule exception installed by [`install_policy_route`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRoute {
    source: Ipv4Addr,
    host: Ipv4Addr,
    /// `true` when `lookup local` was moved from pref 0 to [`LOCAL_PARK_PREF`].
    relocated_local: bool,
}

/// Set one sysctl, recording its previous value when it actually changes.
fn apply(saved: &mut SavedConf, dev: &str, key: &str, want: &str) {
    let Some(current) = read_conf(dev, key) else {
        log::debug!("netcfg: {dev}/{key} not present; skipping");
        return;
    };
    if current == want {
        log::debug!("netcfg: {dev}/{key} already {want}");
        return;
    }
    match write_conf(dev, key, want) {
        Ok(()) => {
            log::info!("netcfg: {dev}/{key} {current} -> {want}");
            saved
                .sysctls
                .push((dev.to_string(), key.to_string(), current));
        }
        Err(e) => log::warn!("netcfg: cannot set {dev}/{key}={want}: {e}"),
    }
}

/// Run `ip` with the given arguments, returning `Ok` on success.
fn ip(args: &[&str]) -> io::Result<()> {
    let output = Command::new("ip").args(args).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "ip {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn ip_stdout(args: &[&str]) -> String {
    Command::new("ip")
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Preference of the first IPv4 `lookup local` rule, if any.
fn parse_local_pref(text: &str) -> Option<u32> {
    for line in text.lines() {
        let line = line.trim();
        if !(line.contains("lookup local") || line.contains("lookup 255")) {
            continue;
        }
        let pref = line.split(':').next()?.trim();
        return pref.parse().ok();
    }
    None
}

/// `true` when `ip route get` delivered via the local table / loopback.
#[must_use]
pub fn route_is_via_loopback(route_get: &str) -> bool {
    let t = route_get.trim();
    t.contains("dev lo") || t.starts_with("local ") || t.contains(" cache <local>")
}

fn is_exists(e: &io::Error) -> bool {
    let msg = e.to_string();
    msg.contains("exists") || msg.contains("File exists") || msg.contains("error -17")
}

/// Install a policy route so packets from `source` to `host` go via `device`,
/// even when `host` is a local address or the default gateway.
///
/// `FRA_PRIORITY` is an unsigned `u32`; there is no way to insert a rule
/// *before* `lookup local` at pref 0 other than moving that rule to a later
/// pref. The move is skipped when `host` is not local — `lookup local` then
/// misses and the exception at pref 0 is reached anyway.
#[must_use]
pub fn install_policy_route(source: Ipv4Addr, host: Ipv4Addr, device: &str) -> PolicyRoute {
    let mut installed = PolicyRoute {
        source,
        host,
        relocated_local: false,
    };
    log::info!("netcfg: installing policy route {source} -> {host} via {device}");

    let table = SOFTAP_TABLE.to_string();
    let dest = format!("{host}/32");
    let src = source.to_string();
    let host_s = host.to_string();
    let pref_0 = SOFTAP_RULE_PREF.to_string();
    let pref_park = LOCAL_PARK_PREF.to_string();

    match ip(&[
        "route", "replace", "table", &table, &dest, "dev", device, "scope", "link", "src", &src,
    ]) {
        Ok(()) => log::debug!("netcfg: policy route {dest} dev {device} src {src}"),
        Err(e) => {
            log::warn!("netcfg: cannot add policy route: {e}");
            return installed;
        }
    }

    match ip(&[
        "-4", "rule", "add", "pref", &pref_0, "from", &src, "to", &host_s, "lookup", &table,
    ]) {
        Ok(()) => log::info!(
            "netcfg: policy rule pref {SOFTAP_RULE_PREF} {source} -> {host} lookup {SOFTAP_TABLE}"
        ),
        Err(e) if is_exists(&e) => log::debug!("netcfg: policy rule already present"),
        Err(e) => {
            log::warn!("netcfg: cannot add policy rule: {e}");
            let _ = ip(&["route", "flush", "table", &table]);
            return installed;
        }
    }

    // `lookup local` at pref 0 wins for any locally-owned destination, even
    // with SO_BINDTODEVICE. Park it so the exception above is consulted first.
    if is_local_address(host) {
        let rules = ip_stdout(&["-4", "rule", "list"]);
        match parse_local_pref(&rules) {
            Some(0) => {
                if let Err(e) = ip(&["-4", "rule", "add", "pref", &pref_park, "table", "local"]) {
                    if !is_exists(&e) {
                        log::warn!(
                            "netcfg: cannot park lookup local at pref {LOCAL_PARK_PREF}: {e}"
                        );
                    }
                }
                match ip(&["-4", "rule", "del", "pref", &pref_0, "table", "local"]) {
                    Ok(()) => {
                        log::info!(
                            "netcfg: parked lookup local at pref {LOCAL_PARK_PREF} \
                             so {source} -> {host} can leave via {device}"
                        );
                        installed.relocated_local = true;
                    }
                    Err(e) => {
                        log::warn!("netcfg: cannot move lookup local off pref 0: {e}");
                        let _ = ip(&["-4", "rule", "del", "pref", &pref_park, "table", "local"]);
                    }
                }
            }
            Some(p) => log::debug!("netcfg: lookup local already at pref {p}; leaving it"),
            None => log::warn!("netcfg: no IPv4 lookup local rule found"),
        }
    }

    let _ = ip(&["route", "flush", "cache"]);

    let rules = ip_stdout(&["-4", "rule", "list"]);
    log::info!("netcfg: ip -4 rule list:\n{rules}");
    let got = ip_stdout(&["-4", "route", "get", &host_s, "from", &src]);
    if route_is_via_loopback(&got) {
        log::warn!("netcfg: route still local after policy install: {got}");
    } else {
        log::info!("netcfg: ip -4 route get {host} from {source}: {got}");
    }
    installed
}

/// Drop the exception installed by [`install_policy_route`].
///
/// Restores `lookup local` at pref 0 *before* removing the parked copy, so
/// local delivery is never absent.
pub fn remove_policy_route(route: &PolicyRoute) {
    let table = SOFTAP_TABLE.to_string();
    let src = route.source.to_string();
    let host = route.host.to_string();
    let pref_0 = SOFTAP_RULE_PREF.to_string();
    let pref_park = LOCAL_PARK_PREF.to_string();

    match ip(&[
        "-4", "rule", "del", "pref", &pref_0, "from", &src, "to", &host, "lookup", &table,
    ]) {
        Ok(()) => log::debug!(
            "netcfg: policy rule {} -> {} removed",
            route.source,
            route.host
        ),
        Err(e) => log::debug!("netcfg: cannot remove policy rule: {e}"),
    }

    if route.relocated_local {
        match ip(&["-4", "rule", "add", "pref", &pref_0, "table", "local"]) {
            Ok(()) => log::debug!("netcfg: lookup local restored at pref 0"),
            Err(e) if is_exists(&e) => {}
            Err(e) => log::warn!("netcfg: cannot restore lookup local at pref 0: {e}"),
        }
        match ip(&["-4", "rule", "del", "pref", &pref_park, "table", "local"]) {
            Ok(()) => log::debug!("netcfg: parked lookup local at pref {LOCAL_PARK_PREF} removed"),
            Err(e) => log::debug!("netcfg: cannot remove parked lookup local: {e}"),
        }
    }

    match ip(&["route", "flush", "table", &table]) {
        Ok(()) => log::debug!("netcfg: policy route table {table} flushed"),
        Err(e) => log::debug!("netcfg: cannot flush table {table}: {e}"),
    }
    let _ = ip(&["route", "flush", "cache"]);
}

/// Allow Soft-AP replies whose source collides with a local address.
///
/// Best-effort: every failure is logged and skipped. `rp_filter` is also
/// relaxed on the `all` pseudo-interface, since the effective value is
/// `max(all, dev)`.
#[must_use]
pub fn prepare_softap(dev: &str) -> SavedConf {
    let mut saved = SavedConf::default();
    apply(&mut saved, dev, ACCEPT_LOCAL, "1");
    apply(&mut saved, "all", ACCEPT_LOCAL, "1");
    apply(&mut saved, dev, RP_FILTER, "0");
    apply(&mut saved, "all", RP_FILTER, "0");
    saved
}

/// Put back every sysctl [`prepare_softap`] changed.
pub fn restore(saved: &SavedConf) {
    for (dev, key, value) in saved.sysctls.iter().rev() {
        match write_conf(dev, key, value) {
            Ok(()) => log::debug!("netcfg: {dev}/{key} restored to {value}"),
            Err(e) => log::warn!("netcfg: cannot restore {dev}/{key}={value}: {e}"),
        }
    }
}

/// `true` when `addr` is assigned to some interface on this host.
///
/// Implemented by binding a throwaway socket: the kernel only allows a bind
/// to a local address, and answers with `EADDRNOTAVAIL` otherwise. Used to
/// decide whether `lookup local` must be moved, so a false negative only
/// skips the relocate (the exception rule is still installed).
#[must_use]
pub fn is_local_address(addr: Ipv4Addr) -> bool {
    TcpListener::bind((addr, 0)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_is_local_and_test_net_is_not() {
        assert!(is_local_address(Ipv4Addr::LOCALHOST));
        // RFC 5737 TEST-NET-1: never assigned on a real host.
        assert!(!is_local_address(Ipv4Addr::new(192, 0, 2, 1)));
    }

    #[test]
    fn conf_path_is_interface_scoped() {
        assert_eq!(
            conf_path("wlan0", ACCEPT_LOCAL),
            PathBuf::from("/proc/sys/net/ipv4/conf/wlan0/accept_local")
        );
    }

    #[test]
    fn read_conf_is_none_for_unknown_interface() {
        assert!(read_conf("wp-no-such-iface", ACCEPT_LOCAL).is_none());
    }

    #[test]
    fn saved_conf_starts_empty_and_restore_is_a_noop() {
        let saved = SavedConf::default();
        assert!(saved.is_empty());
        restore(&saved);
    }

    #[test]
    fn apply_skips_a_missing_sysctl() {
        let mut saved = SavedConf::default();
        apply(&mut saved, "wp-no-such-iface", ACCEPT_LOCAL, "1");
        assert!(saved.is_empty());
    }

    #[test]
    fn apply_records_nothing_when_already_at_the_wanted_value() {
        let Some(current) = read_conf("lo", ACCEPT_LOCAL) else {
            return;
        };
        let mut saved = SavedConf::default();
        apply(&mut saved, "lo", ACCEPT_LOCAL, &current);
        assert!(saved.is_empty());
    }

    #[test]
    fn parse_local_pref_from_iproute2_default() {
        let text = "0:\tfrom all lookup local\n32766:\tfrom all lookup main\n32767:\tfrom all lookup default\n";
        assert_eq!(parse_local_pref(text), Some(0));
    }

    #[test]
    fn parse_local_pref_when_parked() {
        let text = "0:\tfrom 192.168.0.2 to 192.168.0.1 lookup 100\n1:\tfrom all lookup local\n32766:\tfrom all lookup main\n";
        assert_eq!(parse_local_pref(text), Some(1));
    }

    #[test]
    fn parse_local_pref_missing() {
        assert_eq!(parse_local_pref("32766:\tfrom all lookup main\n"), None);
    }

    #[test]
    fn route_get_local_is_loopback() {
        assert!(route_is_via_loopback(
            "local 192.168.0.1 from 192.168.0.2 dev lo uid 0 \n    cache <local>"
        ));
        assert!(!route_is_via_loopback(
            "192.168.0.1 from 192.168.0.2 dev wlan0 src 192.168.0.2 uid 0"
        ));
    }
}
