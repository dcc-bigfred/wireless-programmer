# CLI usage

The `wireless-programmer` binary is both the daemon and a one-shot client of
it. Run it with no subcommand (or `daemon`) to start the daemon; run any of
the subcommands below to talk to a running daemon over its Unix socket.

```text
wireless-programmer [OPTIONS] [COMMAND]

Commands:
  daemon        Run the IPC daemon (default when no subcommand is given)
  scan             Enumerate candidate devices (Soft-AP radio or LAN mDNS)
  probe            Read a single candidate's device info
  program          Start a programming job and stream its progress
  update-firmware  Upload a firmware image (Soft-AP, LAN, or USB espflash)
  identify         Blink a device's LED so an operator can find it
  link-status   Report radio/link state
  hello         Exchange version + driver capabilities
  job           Inspect or control a running job
  fake          Standalone Soft-AP HTTP mock for one driver (no daemon)

Options:
      --socket <SOCKET>       Override the daemon socket path (every subcommand)
  -i, --interface <IFACE>     Wireless interface for the daemon (e.g. wlan0);
                              use `fake` for in-process FakeRadio + Soft-AP mock
      --require-auth          Enforce SO_PEERCRED allowlist (daemon only; off by default)
      --allow-users <USERS>   Comma-separated allowlist (implies --require-auth)
  -v, --verbose               Verbose logging (daemon only)
  -h, --help                  Print help
  -V, --version               Print version
```

### `daemon --interface fake`

Runs the full IPC daemon with `FakeRadio` (scan returns one WiFred and one
LongFred candidate) and an in-process Soft-AP HTTP mock on
`127.0.0.1:<port>` (default port 8070; override with
`--fake-webserver-port` / `WIRELESS_PROGRAMMER_FAKE_WEB_PORT`). Peer auth is
forced off. Useful for developing `bigfred-wizard` without WiFi hardware.

### `fake --driver wifred|longfred`

Starts **only** the Soft-AP HTTP mock for the chosen driver (no radio, no
IPC). Default bind `127.0.0.1:8070`.

## Socket resolution

Client subcommands connect to the daemon socket, resolved in this order:

1. `--socket PATH` on the command line;
2. `$BIGFRED_DATA_DIR/run/wireless-programmer/wireless-programmer.sock`;
3. `$DATA_DIR/run/wireless-programmer/wireless-programmer.sock`;
4. `/data/run/wireless-programmer/wireless-programmer.sock`.

The daemon creates the parent directory and binds the socket. **Peer
authentication is off by default**: the socket is `0666` and any local
process may connect. Enable auth with `--require-auth` or
`WIRELESS_PROGRAMMER_REQUIRE_AUTH=1`; then the socket is `0660` and peers
are checked via `SO_PEERCRED` against an allowlist (default `bigfred`,
`bigfred-wizard`, override with `--allow-users` /
`WIRELESS_PROGRAMMER_ALLOW_USERS`).

When auth is on, the socket also needs a group owner, or a non-root client
is refused by the filesystem before `SO_PEERCRED` is ever consulted. On
startup the daemon chowns the socket to the primary group of the first
allowlist entry (so `bigfred` by default); set
`WIRELESS_PROGRAMMER_SOCKET_GROUP_USER` to choose a different login name
whose primary group should own it. If that user does not exist, or the
daemon is not privileged enough to chown, it warns and leaves the
socket owner-only — useful on a development machine, fatal for peers.

## Wireless interface

By default the daemon picks the first interface under `/sys/class/net` that
has a `wireless` subdirectory. On a hub with more than one radio, pin it:

```bash
wireless-programmer --interface wlan1
wireless-programmer daemon -i wlp2s0 --verbose
```

`--interface` / `-i` is accepted both at the top level (when starting the
daemon with no subcommand) and on `daemon`. A missing or non-wireless name
fails at start-up with a non-zero exit. The same choice can be set with
`WIRELESS_PROGRAMMER_INTERFACE`; the CLI flag overrides the environment.
`link-status` reports the configured (or auto-selected) interface name.

Every client subcommand accepts:

- `--json` — emit machine-readable JSON instead of human-readable text;
- `--timeout 30s` — per-operation timeout (parsed by `humantime`, default 10s).
  For `update-firmware` the default is 180s in USB mode and 120s over HTTP,
  matching the `espflash` / firmware POST deadline. The daemon also emits a
  `job.watch` detail frame every 3s during those transfers, so a 10s idle
  client (including the Go SDK) still sees progress;
- `--socket PATH` — override the daemon socket path.

## Discovery workflow

```bash
# 1. What drivers does this daemon know?
wireless-programmer hello

# 2. Bring the radio up and scan for config APs (Soft-AP, default).
wireless-programmer scan
# DRIVER     KEY                  RSSI     LABEL
# wifred     AA:BB:CC:DD:EE:01    -54      wiFred-config-AABBCCDDEE01
# wifred     AA:BB:CC:DD:EE:02    -61      wiFred-config-AABBCCDDEE02

# LAN scan (LongFred HTTP OTA via mDNS `_longfred-ota._tcp`):
wireless-programmer scan --mode lan

# USB serial ports (`espflash list-ports` / `/dev/ttyUSB*` / `ttyACM*`):
wireless-programmer scan --mode usb

# 3. Read one device's current config over the radio.
wireless-programmer probe --driver wifred --key AA:BB:CC:DD:EE:01

# 4. Blink the LED so an operator can find the physical throttle.
wireless-programmer identify --driver wifred --key AA:BB:CC:DD:EE:01 --count 5
```

`scan` and `probe` return `noCandidates` / `candidateNotFound` errors when
nothing matches; pipe `--json` for scripting:

```bash
wireless-programmer scan --json | jq '.[] | select(.rssi != null) | .key'
```

## Firmware update

`update-firmware` uploads a LongFred image. Soft-AP and LAN POST
`.app.bin` to `POST /api/v1/firmware` (120 s, not retried). USB runs
`espflash` on a serial port (ELF, merged `.bin`, or `.app.bin`). WiFred
does not support firmware upload.

Use `--mode ap` after putting the throttle into Soft-AP programming mode
(8-second chord). Use `--mode lan` when the throttle is already on the
layout Wi‑Fi and the operator has opened **Firmware update** in the Extras
menu (HTTP is enabled only while that screen is shown). Use `--mode usb`
with the throttle on a USB-UART (or native USB-Serial-JTAG) cable;
`espflash` must be on `PATH`.

```bash
# Soft-AP: join longfred_prog_*, POST the image, keep programming_mode.
wireless-programmer update-firmware --mode ap --driver longfred \
  --key AA:BB:CC:DD:EE:01 --file longfred-markwtech-esp32c6.app.bin

# LAN: no radio; HTTP to the IPv4 from scan --mode lan (or --host).
wireless-programmer update-firmware --mode lan --driver longfred \
  --key 192.168.1.42 --file longfred-markwtech-esp32c6.app.bin
wireless-programmer update-firmware --mode lan --host 192.168.1.42 \
  --file longfred-markwtech-esp32c6.app.bin

# USB: first install of the dual-slot table, or a cable update.
wireless-programmer scan --mode usb
wireless-programmer update-firmware --mode usb --port /dev/ttyUSB0 \
  --file longfred-markwtech-esp32c6.elf --partition-table partitions.csv
wireless-programmer update-firmware --mode usb --port /dev/ttyACM0 \
  --file longfred-markwtech-esp32c6.bin
```

Like `program`, the command watches the job by default; `--no-watch`
returns the job id immediately. While `espflash` or the HTTP POST is
running, the daemon writes a detail frame every 3 seconds (for example
`espflash /dev/ttyUSB0 (12s)`). `job cancel` kills the `espflash` child
and aborts an in-flight firmware POST.

## Programming workflow

`program` starts a job, then opens a `job.watch` stream and prints progress
until the job reaches a terminal state (`done`, `failed`, `cancelled`). The
exit code reflects the outcome: `0` on `done`, non-zero otherwise.

### From individual flags

```bash
wireless-programmer program \
  --driver wifred \
  --key AA:BB:CC:DD:EE:01 \
  --identity 122145 \
  --wifi-ssid bigfred2 \
  --wifi-psk-file /run/secrets/bigfred-psk \
  --server-host bigfred.local \
  --server-port 12090 \
  --roster-file roster.json
```

`--wifi-psk` takes the passphrase inline, which leaves it visible in
`/proc/<pid>/cmdline` to every local user and in shell history. Prefer
`--wifi-psk-file` (trailing newline stripped), or `--wifi-psk-file -` to read
it from stdin:

```bash
printf '%s' "$PSK" | wireless-programmer program ... --wifi-psk-file -
```

Omit both for an open network. `--server-automatic` makes `--server-host` and
`--server-port` optional, since the device discovers the server over mDNS; the
port then defaults to the wiThrottle port `12090`.

### From a request file

`--request-file` loads a complete `ProgramRequest` JSON document and ignores
the individual `--identity` / `--wifi-*` / `--server-*` / `--roster-file`
flags. This is the form `bigfred` / `bigfred-wizard` use when they already
have the full request assembled.

```bash
wireless-programmer program --driver wifred --key AA:BB:CC:DD:EE:01 \
  --request-file request.json
```

`request.json`:

```jsonc
{
  "identity": "122145",
  "wifi":   { "ssid": "bigfred2", "psk": "correct-horse-battery-staple" },
  "server": { "host": "bigfred.local", "port": 12090, "automatic": false },
  "roster": [
    {
      "address": 3,
      "longAddress": false,
      "mode": "128",
      "direction": 0,
      "functions": [
        { "index": 0, "value": 0 },
        { "index": 1, "value": 4 }
      ]
    }
  ]
}
```

### Roster file

When using the individual flags, pass the roster as a JSON array with
`--roster-file`. Each entry is a `RosterEntry`; `address: null` disables the
slot, and the driver caps the number of slots (WiFred: 4) and the highest
function index (WiFred: 16).

`roster.json`:

```jsonc
[
  { "address": 3,    "longAddress": false, "mode": "128", "direction": 0,
    "functions": [ { "index": 0, "value": 0 } ] },
  { "address": 4209, "longAddress": true,  "mode": "28",  "direction": 0,
    "functions": [ { "index": 0, "value": 0 }, { "index": 1, "value": 4 } ] }
]
```

### Flags vs. request file

| Flag | Field | Notes |
|------|-------|-------|
| `--identity` | `identity` | Opaque; 6-digit BigFred pairing code for WiFred |
| `--wifi-ssid` | `wifi.ssid` | Required |
| `--wifi-psk` / `--wifi-psk-file` | `wifi.psk` | Mutually exclusive; omit both for an open network |
| `--server-host` / `--server-port` | `server.host` / `server.port` | Required unless `--server-automatic` |
| `--server-automatic` | `server.automatic` | mDNS discovery instead of a fixed host; port defaults to 12090 |
| `--roster-file` | `roster` | JSON array of `RosterEntry` |

### Watch behaviour

By default `program` follows the job's `job.watch` stream and prints every
frame as it arrives — human-readable lines on stderr, or one compact JSON
object per line on stdout with `--json`, so a consumer can read progress
incrementally rather than waiting for the outcome. Pass `--no-watch` to
return the job id immediately and exit:

```bash
JOB=$(wireless-programmer program --driver wifred --key AA:BB:CC:DD:EE:01 \
  --request-file request.json --no-watch)
wireless-programmer job watch --id "$JOB"
```

## Job control

```bash
wireless-programmer job get     --id <id>     # snapshot
wireless-programmer job watch   --id <id>     # stream until terminal
wireless-programmer job cancel  --id <id>     # request cancellation
```

`job watch` prints `JobFrame`s as they arrive until the job reaches `done` /
`failed` / `cancelled`; with `--json` each frame is one compact JSON object on
its own line.

If no frame arrives within the timeout, the client reports `no job progress
frame within <timeout>` rather than a bare I/O error. Firmware jobs keep the
stream alive with a detail frame every 3 seconds, so watching
`update-firmware` does not depend on raising `--timeout` unless you are
talking to an older daemon. Note that the daemon's worker loop is
hardware-gated: until it drives a live radio, `job.watch` answers with a
single snapshot frame and then goes quiet, so watching a job on a
device-less host reaches that idle deadline by design.

## Link status

```bash
wireless-programmer link-status
# busy:            false
# interface:       -
# rfkill blocked:  false
```

`busy` is true while a programming job holds the radio. `rfkillBlocked`
reflects the kernel rfkill state (the hub's udev rule unblocks it at boot).

## Exit codes

| Code | Meaning |
|------|---------|
| `0`  | Success (job reached `done`, or a query returned) |
| `1`  | Failure — daemon error, `failed`/`cancelled` job, or a client/CLI error |

Errors are printed to stderr as `error: <message>`. Local problems (a missing
flag, an unreadable `--request-file`) are reported as such rather than as
daemon failures, so `error: --wifi-ssid is required (or use --request-file)`
means the invocation was wrong, not that the daemon misbehaved. With `--json`
progress frames still go to stdout, one per line, so a failed job's frames
remain machine-parseable.

## Environment variables

| Variable | Purpose |
|----------|---------|
| `BIGFRED_DATA_DIR` | Data root (default `/data`); socket is `<dir>/run/wireless-programmer/wireless-programmer.sock` |
| `DATA_DIR` | Fallback data root |
| `WIRELESS_PROGRAMMER_REQUIRE_AUTH` | Enable peer auth (`1`/`true`/`yes`/`on`); default off |
| `WIRELESS_PROGRAMMER_ALLOW_USERS` | Comma-separated peer allowlist (used when auth is on; default `bigfred,bigfred-wizard`) |
| `WIRELESS_PROGRAMMER_SOCKET_GROUP_USER` | Login name whose primary group owns the socket (daemon only; defaults to the first allowlist entry when auth is on) |
| `WIRELESS_PROGRAMMER_INTERFACE` | Wireless interface name for the daemon (e.g. `wlan0`); overridden by `--interface` |
| `WIRELESS_PROGRAMMER_GIT_COMMIT` | Git commit baked into the `hello` response (build-time) |
| `WIRELESS_PROGRAMMER_BUILD_TIME` | UTC build timestamp baked into version metadata (build-time, optional) |

Release binaries also carry an ELF section `.wireless-programmer.version`
(`{"version":"v…","commit":"…"}`) injected by the release workflow; `hello`
prefers that tag over `CARGO_PKG_VERSION` when present.
