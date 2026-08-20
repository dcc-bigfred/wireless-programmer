# NewHeiko WiFred driver

Implements [`wp_core::DeviceDriver`] for the NewHeiko WiFred
([`https://github.com/newHeiko/wiFred`](https://github.com/newHeiko/wiFred)).

## Commissioning model

In config mode the firmware calls `initWiFiAP()`, raising an **open** WiFi
AP. The SSID is built as `"wiFred-config" + String(mac[3],16) +
String(mac[4],16) + String(mac[5],16)` — Arduino's hex conversion does **not**
zero-pad, so the driver matches on the prefix `wiFred-config` only, never on
a fixed length.

The AP runs a web server on port 80 with a built-in DHCP server at
`192.168.4.1/24` (the ESP-IDF default; the firmware never calls
`softAPConfig`). The daemon:

1. associates to the open AP via nl80211 (`OpenSystem`, no PSK),
2. assigns itself `192.168.4.2/24` on the wireless interface — **no default
   route**, so the hub's Ethernet default gateway is never hijacked,
3. hands a sync `HttpClient` to the driver,
4. on every exit path (success or failure) disconnects and releases the
   radio.

## Capabilities

| Field                    | Value                  |
|--------------------------|------------------------|
| `maxRosterSlots`         | 4 (`locos[4]`)         |
| `maxFunctionIndex`       | 16 (`MAX_FUNCTION`)    |
| `identityFormat`         | `Digits { len: 6 }`    |
| `supportsThrottleServer` | true                   |
| `supportsFirmwareUpdate` | false                  |
| `commissioning`          | `SoftAp`               |

The identity is a **6-digit BigFred pairing code** written into the firmware's
`throttleName` field, which the device sends as the wiThrottle `N<name>` line.
BigFred pairs on that line using `ValidWithrottleCode`. There is no separate
login/PIN on the WiFred.

## Read-back

`GET /api/getConfigXML` returns the device's current configuration. The
firmware emits a malformed XML prolog (`<?XML version="1.0"
encoding="UTF?8"?>` — uppercase `XML`, `UTF?8`) and serves it as
`text/html`, so the parser is lenient: it skips the declaration and any
unknown elements and only collects the attributes it cares about
(`<structurVersion>`, `<throttleName>`, `<firmwareRevision>`,
`<batteryVoltage>`, `<LOCOS>/<LOCO>`, `<NETWORKS>/<NETWORK>`,
`<LOCOSERVER>`).

The driver rejects a device whose `<structurVersion value="..."/>` is not
`"1"` with `UnsupportedStructureVersion`.

## Write sequence

Configuration is written via a series of `GET /index.html?...` requests whose
query args are consumed by the firmware's `server.arg()` handlers. The
order is **load-bearing**: WiFi settings and restart are applied **last**,
after everything else is written and verified, so the device does not leave
our AP mid-programming.

1. `GET /api/getConfigXML` — snapshot current state, check structure version.
2. `GET /index.html?throttleName=<identity>` — write the pairing code.
3. For each loco slot `n` (1..4):
   `GET /index.html?loco=n&loco.address=<addr>&loco.mode=<mode>&loco.direction=<dir>[&loco.longAddress=on]`
   An unused slot is written with `loco.address=-1` to disable it.
4. For each slot with function maps:
   `GET /index.html?loco=n&f<index>=<value>&...`
5. `GET /index.html?loco.serverName=<host>&loco.serverPort=<port>[&loco.automatic=on]`
   — wiThrottle server.
6. `GET /index.html?remove=<ssid>` then
   `GET /index.html?wifiSSID=<ssid>&wifiKEY=<psk>` — WiFi, applied last.
7. `GET /api/getConfigXML` — verify the write.
8. `GET /restart.html` — restart so WiFi settings take effect.

Query values are percent-encoded (RFC 3986 unreserved set kept). The PSK is
never logged by the daemon.

## Identify

`GET /flashred.html?count=N` blinks the device LED so an operator can find
the physical throttle.

## Modes and directions

Speed-step mode strings accepted by the firmware (`MODES` table in
`locoHandling.cpp`): `""`, `128`, `28`, `27`, `14`, `motorola_28`, `tmcc_32`,
`incremental`, `1`, `2`, `4`, `8`, `16`.

Direction values (`eDirection`): `0` forward, `1` reverse, `2` do-not-change.

Function mapping values (`functionInfo`): `0` throttle, `1` throttle
momentary, `2` throttle locking, `3` throttle single, `4` always-on, `5`
always-off, `6` ignore.

## Testing

The write sequence is covered by `crates/wp-drivers/tests/wifred_write.rs`,
which uses a recording fake HTTP client to assert the **exact** query
strings and their order, including the disabled-slot `-1`, the
`longAddress=on` flag only when set, the `automatic=on` flag only when
requested, and that `restart.html` is never issued on a verification
mismatch.
