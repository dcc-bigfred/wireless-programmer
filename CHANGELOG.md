# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [v0.2] — 2026-08-22

Hub Soft-AP programming for LongFred (and WiFred) when the device AP shares the
hub's LAN subnet — e.g. LongFred at `192.168.0.1` on a hub whose Ethernet is
also `192.168.0.0/24`. Includes policy routing, sysctl tuning, HTTP client
framing for embassy-net RST-after-respond, active scan, association verification,
`--log-level`, and `make hub-upload` for `/data/opt` on read-only hub rootfs.

### Fixed

- Soft-AP radio scan on Raspberry Pi 5 / brcmfmac: bring the interface up
  before scanning, use an active wildcard scan (like `iw scan`) instead of a
  short passive dwell, wait for `NEW_SCAN_RESULTS` (with a timed dump
  fallback), and surface trigger failures as `scan_failed` instead of an
  empty candidate list. 2.4 GHz channels are preferred, with an all-band
  fallback if the firmware rejects the frequency list.
- Soft-AP programming on hubs whose LAN overlaps the device subnet (e.g. the
  LongFred Soft-AP at `192.168.0.1`, which is also the hub's own LAN
  address). Both directions were broken:
  - Outbound `Connection refused` — the route lookup hit the `local` table
    and delivered to loopback. `SO_BINDTODEVICE` does not override
    `from all lookup local`. The daemon parks that rule at pref 1 and
    installs `from <source> to <host> lookup 100` at pref 0, with a /32
    via the wireless device in table 100, then flushes the route cache.
    Output sockets bind the wireless source address so the rule matches.
    Restored on release.
  - Inbound `timed out` — replies whose source is a locally-owned address
    were dropped as a martian source. The radio now sets `accept_local=1`
    and `rp_filter=0` on the wireless device (and `all`) while associated,
    and restores the previous values on release.
  - `Connection reset by peer` on `read` after a successful connect — the
    LongFred HTTP server (embassy-net) sends the `200` with `Content-Length`
    and then `abort()`s, which is a TCP RST. The client used to keep reading
    until EOF, so the RST on the next `read` failed a request that had already
    arrived. It now stops when the `Content-Length` body is complete. Existing
    devices keep `abort()`; new firmware `close()`s after a successful
    response (FIN instead of RST).
- Soft-AP association was never verified, so a rejected or abandoned
  `NL80211_CMD_CONNECT` was reported as success and only surfaced later as a
  confusing HTTP error. `connect_open` now brings the link up first (the
  previous job's `release` puts it down, so association was attempted on a
  down interface), surfaces CONNECT errors instead of draining them, waits
  for carrier, and retries once after a fresh scan because the kernel drops
  its BSS cache when the link goes down. A genuine failure now reports
  `association timed out`.
- `release` removes the on-link address it assigned. A leftover
  `192.168.0.2/24` on `wlan0` kept a second route for the hub's own LAN
  subnet, which made later attempts fail with a mix of `EHOSTUNREACH`,
  `ECONNRESET`, and `ECONNREFUSED`.

### Changed

- A Soft-AP HTTP request that exhausts its retries logs the destination,
  source address, and bound device, each attempt's error kind at `debug`,
  and — when the destination is also a local address — which of the two
  collision fixes is missing.

### Added

- `--log-level error|warn|info|debug|trace` for the daemon and `fake`
  (`-v` remains an alias for `debug`; `RUST_LOG` is honoured when both are
  omitted). Scan logs the interface state, trigger result, and raw BSS
  count; at `debug` every SSID; at `info` when APs were seen but none
  matched `longfred_prog` / `wiFred-config`.
- `make hub-upload` — cross-build musl arm64 and deploy to
  `/data/opt/wireless-programmer/` on the hub (`scp -O` for Dropbear).

### Assets

Static **linux/arm64** and **linux/amd64** musl binaries (`wireless-programmer-linux-*`).
Hub operators can update without reflashing OS: `make hub-upload` to
`/data/opt/wireless-programmer/` (see [bigfred-os](https://github.com/dcc-bigfred/bigfred-os)).

## [v0.1] — 2026-08-21

First public release of **wireless-programmer** — a Rust daemon on the BigFred hub
(Raspberry Pi 5) that discovers and programs physical throttle hardware. Clients
(`bigfred`, `bigfred-wizard`, CLI) talk to it over a Unix socket.

### Added

- **Daemon & IPC** — length-prefixed JSON on
  `$BIGFRED_DATA_DIR/run/wireless-programmer/wireless-programmer.sock`; job
  registry with one programming job at a time; optional `SO_PEERCRED` peer auth.
- **WiFred driver** — Soft-AP discovery (`wiFred-config*`), HTTP programming,
  roster + wiThrottle server + BigFred identity, post-program verify.
- **LongFred driver** — Soft-AP discovery (`longfred_prog*`), HTTP programming,
  verify, and negative verify tests.
- **LongFred firmware OTA** — HTTP update over Soft-AP and LAN (mDNS
  `_longfred-ota._tcp.local`); USB update via `espflash`; streaming job progress
  for long-running uploads.
- **Digitrax FRED driver** — wired throttle commissioning via Z21 LAN
  `LAN_LOCONET_DISPATCH_ADDR` (`0xA3`); Z21 discovery (`scan --mode z21`) via
  mDNS and UDP broadcast probe.
- **Radio & link** — nl80211/rtnetlink scan; bounded HTTP client; selectable
  wireless interface (`--interface` / `WIRELESS_PROGRAMMER_INTERFACE`).
- **Fake mode** — `--interface fake` with in-process Soft-AP HTTP mocks and
  standalone `wireless-programmer fake` for dev/CI without WiFi hardware.
- **CLI** — daemon, `scan`, `probe`, `identify`, `program`, `job get|watch|cancel`,
  `link-status`, `hello`; `--json`, `--timeout`, request/roster file inputs.
- **Client SDKs** — Rust `wp-client` crate and Go client (`go/client`, vendored
  by `bigfred`).
- **Crate layout** — `wp-proto`, `wp-core`, `wp-link`, `wp-drivers`, `wp-fake`,
  `wp-client`, `wireless-programmer` binary.
- **Docs** — socket API (`docs/api.md`), CLI guide (`docs/cli.md`), Go client
  (`docs/go-client.md`), per-driver notes (`docs/drivers/`).
- **Build & CI** — Makefile (`build`, `dev`, `release`, musl targets); org
  reusable workflows for musl CI and tagged releases; ELF version section
  `.wireless-programmer.version`.

### Fixed

- Socket permissions and CLI honesty so peers and one-shot clients can reach the
  daemon reliably.

### Pull requests

- [#1](https://github.com/dcc-bigfred/wireless-programmer/pull/1) — LongFred
  Soft-AP programming driver
- [#2](https://github.com/dcc-bigfred/wireless-programmer/pull/2) — LongFred
  firmware OTA (HTTP + USB)
- [#4](https://github.com/dcc-bigfred/wireless-programmer/pull/4) — FRED
  programming via Z21 LAN dispatch

### Assets

Static **linux/arm64** and **linux/amd64** musl binaries (`wireless-programmer-linux-*`)
for hub deployment. Hub OS pulls the arm64 build (see
[bigfred-os](https://github.com/dcc-bigfred/bigfred-os)).
