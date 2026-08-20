# Digitrax FRED driver

Programs a wired Digitrax FRED throttle by sending
[`LAN_LOCONET_DISPATCH_ADDR`](../../../docs/content/en/specs/bigfred/protos/z21.md)
(`0xA3`) to a **physical** Z21-LAN command station that is LocoNet master
(Roco Z21, possibly RB1110). The FRED has no Wi‑Fi and no config page: the
operator plugs it into an L-NET / LOCONET-T / LOCONET-B jack after the
dispatch.

This is **not** LongFred/WiFred Soft-AP programming, and **not** BigFred's
inbound `z21server` (which does not implement `0xA3`).

## Commissioning model

The daemon binds a UDP socket and talks Z21 LAN. The driver never opens
sockets itself.

1. Optional discovery: `scan --mode z21` (mDNS `_z21._udp.local` plus a UDP
   `LAN_GET_SERIAL_NUMBER` broadcast to `255.255.255.255:21105` and `:21106`).
   Hardware Roco Z21 usually has no mDNS, so the UDP probe is required.
2. `program --driver fred --key host:port` with exactly one roster address.
   A prior scan is not required when `key` is `host:port`.
3. The runtime sends `LAN_GET_SERIAL_NUMBER` as a login/probe, then
   `LAN_LOCONET_DISPATCH_ADDR` with the 16-bit DCC address (little-endian).
   Timeout is ~2 s. Optional `LAN_LOGOFF` follows.

Candidate `key` is `ip:port` (typically `192.168.0.111:21105`).

## Capabilities

| Field                    | Value     |
|--------------------------|-----------|
| `maxRosterSlots`         | 1         |
| `maxFunctionIndex`       | 0         |
| `identityFormat`         | `any`     |
| `supportsThrottleServer` | false     |
| `supportsFirmwareUpdate` | false     |
| `commissioning`          | `Lan`     |

`identity`, `wifi`, and `server` are unused (empty defaults on the wire).

## Dispatch result

| Reply | Job outcome |
|-------|-------------|
| `Result > 0` | `done`, detail `slot N` (LocoNet slot) |
| `Result = 0` | `failed`, detail `dispatchFailed` (DISPATCH_PUT rejected) |
| `LAN_X_UNKNOWN_COMMAND` | `failed`, detail `z21NoLocoNet` (no LocoNet dispatch) |
| Serial probe succeeded, no `0xA3` reply (FW &lt; 1.22) | `done`, detail `noAck` |
| No serial reply and no dispatch reply | `failed` (unreachable) |

## CLI

```bash
wireless-programmer scan --mode z21
wireless-programmer program --driver fred --key 192.168.0.111:21105 \
  --roster-file roster.json
```

`roster.json`:

```jsonc
[{ "address": 42 }]
```

`--identity`, `--wifi-ssid`, and `--server-*` are not required for `fred`.
