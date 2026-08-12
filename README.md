# wireless-programmer

A Rust daemon that discovers and programs physical throttle hardware for
BigFred. It runs on the BigFred hub (Raspberry Pi 5) next to `bigfred` and is
driven over a Unix socket by `bigfred` / `bigfred-wizard`.

## What it does

- **Discovers** supported throttle devices (e.g. a NewHeiko WiFred in config
  mode, which raises an open WiFi AP).
- **Programs** each device with: WiFi credentials, a BigFred identity, the
  wiThrottle server address, and a DCC vehicle list.
- **Verifies** the result by reading the device back before signalling
  success.

## Architecture

The daemon is driver-oriented. A `DeviceDriver` implementation owns the
protocol for one family of hardware; the daemon owns the radio, the socket,
and the job lifecycle. Drivers never touch transport directly — the daemon
hands them a `Transport` (HTTP, bytes, …) after establishing reachability.
This keeps the door open for non-WiFi devices (NFC, USB-serial) without
reshaping driver code.

```text
crates/
  wp-proto/            socket wire types + 4-byte-LE length+JSON framing
  wp-core/             DeviceDriver trait, capabilities, typed errors
  wp-link/             radio (nl80211/rtnetlink) + bounded HTTP client
  wp-drivers/          wifred/, longfred/ — Soft-AP programming drivers
  wp-fake/             FakeRadio + Soft-AP HTTP mocks (dev / tests)
  wp-client/           Rust client SDK (mirrors go/client)
  wireless-programmer/ bin: socket server, job registry, dispatch + CLI
go/client/             Go client (vendored by bigfred)
docs/                  api.md, cli.md, go-client.md, drivers/
```

## Fake mode (no WiFi hardware)

```bash
# Full daemon with fake radio + Soft-AP HTTP mock (one candidate per driver)
wireless-programmer daemon --interface fake --verbose
# Optional: --fake-webserver-port 8070 (default) or 0 for ephemeral

# Standalone Soft-AP HTTP mock only (no IPC / radio)
wireless-programmer fake --driver wifred --bind 127.0.0.1:8070
wireless-programmer fake --driver longfred
```

With `--interface fake`, scan always returns one WiFred and one LongFred
candidate; programming talks to an in-process HTTP mock on `127.0.0.1`.

## Memory profile

Every crate is **allocation-conscious** (an administrative service, not a hot
path). We commit to explicit bounds instead: one programming job at a time,
max 64 scan results, max 8 socket connections, max 1 MiB socket frame, max
64 KiB HTTP body, bounded retries and deadlines. See
`CODING-GUIDELINES.md` §2 for the rationale.

## Building

```bash
make build      # debug
make dev        # build + run daemon in foreground (`--verbose`)
make release    # release (opt-level z, LTO, strip)
make release-musl TARGET_MUSL=aarch64-unknown-linux-musl   # static arm64 → dist/
```

`make dev` accepts `INTERFACE=wlan0` and the usual env vars (`DATA_DIR`,
`WIRELESS_PROGRAMMER_ALLOW_USERS`, …).

Or the usual Cargo checks:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --workspace --locked --profile release-assertions
```

Static musl builds (arm64 / amd64) are produced by CI via the org reusable
workflow (`dcc-bigfred/common` `rust-musl-ci`); tagged releases inject
`.wireless-programmer.version` into the ELF binaries (`rust-release`). See
`.github/workflows/`.

## Socket API

Length-prefixed JSON on `$BIGFRED_DATA_DIR/run/wireless-programmer/wireless-programmer.sock`
(`DATA_DIR`, fallback `/data`). Peer auth is **off by default** (socket
`0666`); enable with `--require-auth` / `WIRELESS_PROGRAMMER_REQUIRE_AUTH`
for `0660` + `SO_PEERCRED` allowlist. See `docs/api.md`.

## CLI

The same binary is both the daemon and a one-shot client of it. With no
subcommand it runs the daemon; the subcommands below are clients.

```bash
# daemon (default)
wireless-programmer --socket /data/run/wireless-programmer/wireless-programmer.sock
wireless-programmer daemon --verbose
wireless-programmer daemon --interface wlan0

# discovery + programming
wireless-programmer scan                         # list candidates on the radio
wireless-programmer probe --driver wifred --key AA:BB:CC:DD:EE:FF
wireless-programmer identify --driver wifred --key AA:BB:CC:DD:EE:FF --count 5

# program + stream progress to completion
wireless-programmer program \
  --driver wifred --key AA:BB:CC:DD:EE:FF \
  --identity 122145 --wifi-ssid bigfred2 --wifi-psk-file psk.txt \
  --server-host bigfred.local --server-port 12090 \
  --roster-file roster.json

# or load the full request body from a file
wireless-programmer program --driver wifred --key AA:BB:CC:DD:EE:FF \
  --request-file request.json

# job control + link
wireless-programmer job get --id <id>
wireless-programmer job watch --id <id>
wireless-programmer job cancel --id <id>
wireless-programmer link-status
wireless-programmer hello
```

Every client subcommand accepts `--json` for machine-readable output,
`--timeout` for a per-operation deadline, and `--socket` to override the daemon
path. `program` and `job watch` print each progress frame as it arrives (one
compact JSON object per line under `--json`). See [`docs/cli.md`](docs/cli.md) for
the full CLI guide (workflows, request/roster file formats, exit codes,
environment variables). The `wp-client` crate exposes the same surface as a
library for programmatic callers; the Go client is documented in
[`docs/go-client.md`](docs/go-client.md).

## License

MIT.
