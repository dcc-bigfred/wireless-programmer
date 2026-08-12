# Go client SDK

`github.com/dcc-bigfred/wireless-programmer/go/client` is the Go client for
the `wireless-programmer` daemon. It speaks the same length-prefixed JSON
Unix-socket protocol as `microinit`'s IPC (4-byte little-endian length +
UTF-8 JSON, max 1 MiB) and is intended to be vendored by `bigfred` and
`bigfred-wizard`.

## Vendoring

The module lives under `wireless-programmer/go`:

```
wireless-programmer/
  go/
    go.mod            module github.com/dcc-bigfred/wireless-programmer/go
    client/
      client.go       Client + methods
      doc.go          package doc
      frame_test.go   framing tests
```

`bigfred` vendors it (e.g. via a `replace` directive pointing at a pinned
commit or a vendored copy):

```go
// go.mod
require github.com/dcc-bigfred/wireless-programmer/go v0.1.0

// Pin to a specific commit until a tag is cut.
replace github.com/dcc-bigfred/wireless-programmer/go => github.com/dcc-bigfred/wireless-programmer/go <commit>
```

## Constructing a client

```go
import "github.com/dcc-bigfred/wireless-programmer/go/client"

c := &client.Client{
    Socket:  client.DefaultSocket,            // /data/run/wireless-programmer/wireless-programmer.sock
    Timeout: 10 * time.Second,                 // per-operation timeout
    // Dial: net.DialTimeout,                  // override for tests
}
```

`Socket` defaults to `DefaultSocket` when empty; `Timeout` defaults to 10s
when zero. Override `Dial` in tests to point at an in-memory listener.

The socket is mode `0666` when peer auth is off (the default). With
`--require-auth` it is `0660`, owned by the primary group of the daemon's
first allowlisted user (`bigfred` by default), so the calling process must
be that user or in that group. A `permission denied` from `Dial` means the
caller is outside the group — the daemon's `SO_PEERCRED` allowlist never
gets a chance to run, so widening it does not help. See the permissions
section of [`api.md`](api.md).

## Methods

Each method performs one request/response round-trip over a fresh
connection, sets a deadline of `Timeout`, and returns a typed error on
failure (see [Errors](#errors)).

| Method | Wire method | Returns |
|--------|-------------|---------|
| `Hello()` | `hello` | `*HelloResult` (version + drivers) |
| `Scan()` | `scan` | `[]CandidateWire` |
| `Probe(candidate)` | `probe` | `*DeviceInfoWire` |
| `Program(candidate, req)` | `program` | `*ProgramResult` (job id) |
| `JobGet(jobID)` | `job.get` | `*JobSnapshot` |
| `JobCancel(jobID)` | `job.cancel` | `*JobSnapshot` |
| `Identify(candidate, count)` | `identify` | `nil` |
| `LinkStatus()` | `link.status` | `*LinkStatusWire` |
| `JobWatch(jobID)` | `job.watch` | `net.Conn` (stream; see below) |

### Discovery

```go
cands, err := c.Scan()
if err != nil {
    if errors.Is(err, client.ErrNoCandidates) {
        // nothing on the radio yet
    }
    return err
}
for _, cand := range cands {
    info, err := c.Probe(client.CandidateRef{Driver: cand.Driver, Key: cand.Key})
    // ...
}
```

### Programming

```go
// Small local pointer helper (the client does not ship one).
ptr := func[T any](v T) *T { return &v }

req := client.ProgramRequestWire{
    Identity: "122145",
    Wifi:     client.WifiCredentialsWire{SSID: "bigfred2", PSK: "..."},
    Server:   client.ThrottleServerWire{Host: "bigfred.local", Port: 12090},
    Roster: []client.RosterEntryWire{
        {
            Address:     ptr(uint16(3)),
            LongAddress:  ptr(false),
            Mode:         "128",
            Direction:    ptr(uint8(0)),
            Functions: []client.FunctionMappingWire{
                {Index: 0, Value: 0},
                {Index: 1, Value: 4},
            },
        },
    },
}
res, err := c.Program(client.CandidateRef{Driver: "wifred", Key: "AA:BB:CC:DD:EE:01"}, req)
if err != nil {
    if errors.Is(err, client.ErrBusy) {
        // radio held by another job; retry or surface to the user
    }
    return err
}
// res.JobID
```

The wire structs use pointers (`*uint16`, `*bool`, ...) so omitted fields are
omitted from JSON (via `omitempty`), not zero-valued. Use a pointer helper
like the one above to set them.

### Streaming job progress

`JobWatch` opens a streaming connection and returns it; the caller drains
`JobFrame`s with `ReadFrame`, which sets a per-frame idle read deadline of
`Timeout`. Close the conn when done.

```go
conn, err := c.JobWatch(jobID)
if err != nil {
    return err
}
defer conn.Close()

for {
    resp, err := c.ReadFrame(conn)
    if err != nil {
        // EOF / idle-timeout / codec error
        return err
    }
    if resp.Error != nil {
        return resp.Error
    }
    var frame client.JobFrame
    if err := json.Unmarshal(resp.Result, &frame); err != nil {
        return err
    }
    pct := 0
    if frame.Progress != nil {
        pct = int(*frame.Progress)
    }
    log.Printf("job %s: %s %d%%", frame.JobID, frame.State, pct)
    switch frame.State {
    case "done", "failed", "cancelled":
        return nil
    }
}
```

## Errors

`responseError` maps the daemon's `error.code` to typed sentinel errors:

| Code | Sentinel | Meaning |
|------|----------|---------|
| `busy` | `ErrBusy` | Radio held by another job |
| `notFound`, `candidateNotFound` | `ErrNotFound` | Referenced job/candidate missing |
| `noCandidates` | `ErrNoCandidates` | Scan found nothing |
| (other) | `*ErrorBody` | Wrapped as `resp.Error` (`.Code`, `.Message`) |

Use `errors.Is`:

```go
if errors.Is(err, client.ErrBusy) { ... }
```

The raw `*ErrorBody` is also returned for unmapped codes and satisfies the
`error` interface (its `Error()` returns the message).

## Wire types

The Go structs mirror `wp-proto` 1:1 (camelCase JSON tags). The main ones:

- `CandidateWire{Driver, Key, Label, RSSI *int32}`
- `CandidateRef{Driver, Key}`
- `HelloResult{Version, Commit, Drivers []DriverInfoWire}`
- `DriverInfoWire{ID, Name, Capabilities}` / `CapabilitiesWire{MaxRosterSlots, MaxFunctionIndex, IdentityFormat, SupportsThrottleServer, Commissioning, CommissioningNet}`
- `ProgramRequestWire{Identity, Wifi, Server, Roster []RosterEntryWire, Bigfred, RosterMode}`
- `WifiCredentialsWire{SSID, PSK}` / `ThrottleServerWire{Host, Port, Automatic *bool}`
- `RosterEntryWire{Address *uint16, LongAddress *bool, Mode string, Direction *uint8, Functions []FunctionMappingWire}`
- `DeviceInfoWire{Driver, Key, FirmwareRevision, Identity, BatteryMV *uint32, Roster}`
- `JobSnapshot{JobID, State, Driver, Key, Detail}` / `JobFrame{JobID, State, Step, Progress *uint8, Detail}`
- `LinkStatusWire{Busy, Interface, RfkillBlocked}`

See `client.go` for the full field list and JSON tags, and `docs/api.md`
for the protocol-level description.
