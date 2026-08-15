# Socket API

`wireless-programmer` exposes a Unix socket (default
`/data/run/wireless-programmer/wireless-programmer.sock`, mode `0660`) for `bigfred` and
`bigfred-wizard` to discover and program physical throttle hardware.

## Wire format

Every message is a 4-byte little-endian `u32` length prefix followed by that
many bytes of UTF-8 JSON. The maximum frame payload is **1 MiB**; larger frames
are rejected. This matches the convention used by `microinit`
(`microinit/src/ipc.rs`).

Each request and response is `type`-tagged. The `type` field is mirrored in
the response so callers can correlate requests without an explicit id.

```jsonc
// Request envelope
{ "type": "scan", "params": { ... } }

// Response envelope: exactly one of result / error is present.
{ "type": "scan", "result": { ... } }
{ "type": "scan", "error": { "code": "noCandidates", "message": "..." } }
```

## Methods

| Method         | Params                  | Result                | Notes                          |
|----------------|-------------------------|-----------------------|--------------------------------|
| `hello`           | none                    | `HelloResult`         | Version + driver capabilities  |
| `scan`            | `{ mode? }`             | `Candidate[]`         | Soft-AP radio (`ap`, default) or LAN mDNS (`lan`) |
| `probe`           | `{ candidate }`        | `DeviceInfo`         | Read a single device's info     |
| `program`         | `{ candidate, request }` | `ProgramResult`     | Start a job, returns `jobId`  |
| `updateFirmware`  | `{ mode, candidate?, path, host? }` | `ProgramResult` | HTTP firmware upload job |
| `job.get`      | `{ jobId }`             | `JobSnapshot`        | Snapshot a job's state          |
| `job.watch`    | `{ jobId }`             | `JobFrame` (stream)  | Stream progress until terminal |
| `job.cancel`   | `{ jobId }`             | `JobSnapshot`        | Request cancellation            |
| `identify`     | `{ candidate, count? }` | (empty)              | Blink the device LED            |
| `link.status`  | none                    | `LinkStatus`         | Radio/link state                |

### `hello`

Returns the daemon version and the list of registered drivers with their
capabilities (max roster slots, max function index, identity format,
commissioning kind, optional Soft-AP `commissioningNet`, throttle-server
support, firmware-update support).

`version` is the release tag from the ELF section `.wireless-programmer.version`
when the binary was published via the release workflow; otherwise the Cargo
package version. `commit` is the matching tag/build commit when available.

### `scan`

Optional `params.mode` is `"ap"` (default) or `"lan"`.

Soft-AP (`ap`) triggers an nl80211 scan and returns the candidates each
driver claims:

- WiFred: every AP whose SSID starts with `wiFred-config`
- LongFred: every AP whose SSID starts with `longfred_prog`

LAN (`lan`) does not use the radio. It queries mDNS for
`_longfred-ota._tcp.local` and returns LongFred candidates whose `key` is
the advertised IPv4.

### `updateFirmware`

Starts a firmware-upload job. The image path is on the hub filesystem
(typically a `.app.bin` produced by `espflash save-image` without
`--merge`). `mode` is `"ap"` or `"lan"`.

- **AP**: join the LongFred Soft-AP like `program`, then
  `POST /api/v1/firmware` with `application/octet-stream`. The HTTP
  transfer has a 120 s deadline and is not retried. After a successful
  reboot the device stays in programming mode.
- **LAN**: no radio. HTTP to `candidate.key` (an IPv4 from `scan` with
  `mode: "lan"`) or `params.host`. The throttle must have HTTP OTA
  enabled from the Firmware update menu. After reboot it rejoins layout
  Wi‑Fi.

A driver with `supportsFirmwareUpdate: false` (WiFred) returns
`driverError`. A second job while the radio is held returns `busy`
(LAN jobs do not take the radio).

```jsonc
{
  "type": "updateFirmware",
  "params": {
    "mode": "ap",
    "candidate": { "driver": "longfred", "key": "AA:BB:CC:DD:EE:01" },
    "path": "/data/firmware/longfred-markwtech-esp32c6.app.bin"
  }
}
```

### `probe`

Reads a single candidate's device info over the radio (associate → HTTP GET
→ parse → release). For WiFred this is `/api/getConfigXML`; for LongFred it
is `/api/v1/settings` (JSON, including `device.variant` when present).

### `program`

Starts a programming job. The radio is an exclusive resource: at most one
job runs at a time; a second `program` returns `error.code = "busy"`. The
request body is supplied by the caller (`bigfred`/`bigfred-wizard`), keeping
`wireless-programmer` decoupled from BigFred's REST API, DB and auth.

`ProgramRequest`:

```jsonc
{
  "identity": "122145",            // opaque; WiFred: 6-digit pairing code; LongFred: hostname
  "wifi":   { "ssid": "bigfred2", "psk": "..." },
  "server": { "host": "bigfred.local", "port": 12090, "automatic": false },
  "roster": [
    {
      "address": 3, "longAddress": false, "mode": "128",
      "direction": 0,
      "functions": [ { "index": 0, "value": 0 }, { "index": 1, "value": 4 } ]
    }
  ],
  // LongFred (optional):
  "bigfred": { "login": "ops", "pin": "1234" },
  "rosterMode": "static"
}
```

See [`drivers/wifred.md`](drivers/wifred.md) and
[`drivers/longfred.md`](drivers/longfred.md) for per-driver write sequences.

The job runs through the state machine: `queued → joining → probing →
writing → verifying → restarting → done`. Progress is observable via
`job.watch`.

### `job.watch`

Opens a streaming connection. The daemon writes `JobFrame` messages until
the job reaches a terminal state (`done`, `failed`, `cancelled`). Callers
should set a per-frame idle read deadline (the Go client does this
automatically).

### `identify`

Asks the device to blink its LED so an operator can find the physical
throttle. For WiFred this maps to `GET /flashred.html?count=N`.

## Error codes

| Code                 | Meaning                                       |
|----------------------|-----------------------------------------------|
| `busy`               | The radio is already in use by another job     |
| `notFound`           | The referenced job does not exist             |
| `candidateNotFound`  | The referenced candidate does not exist       |
| `noCandidates`       | A scan found no devices                       |
| `noWireless`         | No wireless adapter is available              |
| `radioBlocked`       | rfkill blocks the radio                       |
| `deviceUnreachable`  | The device could not be reached after associating |
| `invalidRequest`     | The programming request failed validation    |
| `driverError`        | The underlying driver reported an error       |
| `jobCancelled`       | The job was cancelled                         |
| `internal`           | An internal daemon error                      |

## Permissions

Peer authentication is **off by default**. The socket is then `0666` and any
local process may connect — convenient for development (`make dev`).

Enable authentication with `--require-auth` or
`WIRELESS_PROGRAMMER_REQUIRE_AUTH=1`. Then the socket is `0660` and peer
credentials are checked via `SO_PEERCRED` against an allowlist (default
`bigfred`, `bigfred-wizard`). Override the allowlist with `--allow-users`
or `WIRELESS_PROGRAMMER_ALLOW_USERS` (comma-separated login names). Only
those users may issue commands.

With auth on, the allowlist is only reachable if the socket has a group the
peers belong to: with `0660` and no group owner, a non-root client is refused
with `EACCES` at `connect(2)`, before the daemon can inspect its credentials.
So after binding, the daemon chowns the socket to the primary group of the
first allowlist entry — on BigFred OS that makes it `root:bigfred 0660`, which
the `bigfred` service can open. `WIRELESS_PROGRAMMER_SOCKET_GROUP_USER`
selects a different login name whose primary group should own it. When the
user cannot be resolved, or the daemon lacks the privilege to chown, it
warns and leaves the socket owner-only rather than refusing to start; this
keeps a non-privileged development run usable, and the warning is the signal
that peers will not get in.

## CLI

The `wireless-programmer` binary is also a client of its own daemon. With no
subcommand it runs the daemon; the subcommands below are one-shot clients
over the same socket. Every client subcommand accepts `--json`
(machine-readable output) and `--socket` (override the daemon path).

| Subcommand | Purpose |
|------------|---------|
| `scan [--mode ap\|lan]` | Enumerate Soft-AP APs or LAN OTA hosts |
| `probe --driver --key` | Read a single candidate's device info |
| `program --driver --key ...` | Start a programming job and stream progress to completion |
| `update-firmware --mode ap\|lan --file ...` | Upload firmware over HTTP |
| `identify --driver --key [--count N]` | Blink the device LED |
| `job get\|watch\|cancel --id` | Inspect or control a running job |
| `link-status` | Report radio/link state |
| `hello` | Exchange version + driver capabilities |
| `daemon [--verbose] [-i|--interface IFACE]` | Run the IPC daemon (also the default with no subcommand) |

`program` builds the request either from individual flags (`--identity`,
`--wifi-ssid`, `--wifi-psk` / `--wifi-psk-file`, `--server-host`,
`--server-port`, `--server-automatic`, `--roster-file`) or from
`--request-file` (a full `ProgramRequest` JSON document). After the job starts
it opens a `job.watch` stream and prints each frame as it arrives until the job
reaches a terminal state; pass `--no-watch` to return the job id immediately
instead.

The `wp-client` crate (`crates/wp-client`) exposes the same surface as a
synchronous, std-only Rust library for programmatic callers. For the full
CLI guide see [`cli.md`](cli.md); for the Go client see
[`go-client.md`](go-client.md).
