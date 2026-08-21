# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
