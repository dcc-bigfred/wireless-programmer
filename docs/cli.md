# CLI usage

The `wireless-programmer` binary is both the daemon and a one-shot client of
it. Run it with no subcommand (or `daemon`) to start the daemon; run any of
the subcommands below to talk to a running daemon over its Unix socket.

```text
wireless-programmer [OPTIONS] [COMMAND]

Commands:
  daemon        Run the IPC daemon (default when no subcommand is given)
  scan          Enumerate candidate devices on the radio
  probe         Read a single candidate's device info
  program       Start a programming job and stream its progress
  identify      Blink a device's LED so an operator can find it
  link-status   Report radio/link state
  hello         Exchange version + driver capabilities
  job           Inspect or control a running job

Options:
      --socket <SOCKET>  Override the daemon socket path (every subcommand)
  -v, --verbose          Verbose logging (daemon only)
  -h, --help              Print help
  -V, --version           Print version
```

## Socket resolution

Client subcommands connect to the daemon socket, resolved in this order:

1. `--socket PATH` on the command line;
2. `$BIGFRED_DATA_DIR/run/wireless-programmer/wireless-programmer.sock`;
3. `$DATA_DIR/run/wireless-programmer/wireless-programmer.sock`;
4. `/data/run/wireless-programmer/wireless-programmer.sock`.

The daemon creates the parent directory and binds the socket with mode
`0660`. Peers are checked via `SO_PEERCRED` against an allowlist (default
`bigfred`, `bigfred-wizard`); override it with
`WIRELESS_PROGRAMMER_ALLOW_USERS=alice,bob` (comma-separated login names).

Every client subcommand accepts:

- `--json` — emit machine-readable JSON instead of human-readable text;
- `--timeout 30s` — per-operation timeout (parsed by `humantime`, default 10s);
- `--socket PATH` — override the daemon socket path.

## Discovery workflow

```bash
# 1. What drivers does this daemon know?
wireless-programmer hello

# 2. Bring the radio up and scan for config APs.
wireless-programmer scan
# DRIVER     KEY                  RSSI     LABEL
# wifred     AA:BB:CC:DD:EE:01    -54      wiFred-config-AABBCCDDEE01
# wifred     AA:BB:CC:DD:EE:02    -61      wiFred-config-AABBCCDDEE02

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
  --wifi-psk 'correct-horse-battery-staple' \
  --server-host bigfred.local \
  --server-port 12090 \
  --roster-file roster.json
```

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
| `--wifi-ssid` / `--wifi-psk` | `wifi.ssid` / `wifi.psk` | `psk` optional for open networks |
| `--server-host` / `--server-port` | `server.host` / `server.port` | |
| `--server-automatic` | `server.automatic` | mDNS discovery instead of a fixed host |
| `--roster-file` | `roster` | JSON array of `RosterEntry` |

### Watch behaviour

By default `program` streams `job.watch` frames to stderr until the job is
terminal and prints the final frame as JSON (with `--json`). Pass
`--no-watch` to return the job id immediately and exit:

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

`job watch` streams `JobFrame`s until the job reaches `done` / `failed` /
`cancelled`; with `--json` each frame is printed on its own line.

## Link status

```bash
wireless-programmer link-status
# { "busy": false, "rfkillBlocked": false }
```

`busy` is true while a programming job holds the radio. `rfkillBlocked`
reflects the kernel rfkill state (the hub's udev rule unblocks it at boot).

## Exit codes

| Code | Meaning |
|------|---------|
| `0`  | Success (job reached `done`, or a query returned) |
| `1`  | Failure — daemon error, `failed`/`cancelled` job, or a client/CLI error |

Client errors are printed to stderr as `error: <message>`. With `--json` the
final frame is still emitted on stdout for machine parsing.

## Environment variables

| Variable | Purpose |
|----------|---------|
| `BIGFRED_DATA_DIR` | Data root (default `/data`); socket is `<dir>/run/wireless-programmer/wireless-programmer.sock` |
| `DATA_DIR` | Fallback data root |
| `WIRELESS_PROGRAMMER_ALLOW_USERS` | Comma-separated peer allowlist (daemon only) |
| `WIRELESS_PROGRAMMER_GIT_COMMIT` | Git commit baked into the `hello` response (build-time) |
