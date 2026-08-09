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
  wp-drivers/          wifred/ — NewHeiko WiFred driver
  wireless-programmer/ bin: socket server, job registry, dispatch
go/client/             Go client (vendored by bigfred)
docs/                  api.md, drivers/wifred.md
```

## Memory profile

Every crate is **allocation-conscious** (an administrative service, not a hot
path). We commit to explicit bounds instead: one programming job at a time,
max 64 scan results, max 8 socket connections, max 1 MiB socket frame, max
64 KiB HTTP body, bounded retries and deadlines. See
`CODING-GUIDELINES.md` §2 for the rationale.

## Building

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --workspace --locked --profile release-assertions
```

Static musl builds (arm64 / amd64) are produced by CI; see
`.github/workflows/ci.yml`.

## Socket API

Length-prefixed JSON on `$BIGFRED_DATA_DIR/run/wireless-programmer.sock`
(`DATA_DIR`, fallback `/data`), mode `0660`, peers verified with
`SO_PEERCRED`. See `docs/api.md`.

## License

MIT.
