# LongFred driver

Implements [`wp_core::DeviceDriver`] for LongFred throttles in Soft-AP
programming mode.

## Commissioning model

In programming mode the firmware raises an **open** WiFi AP named
`longfred_prog_XXXXXX` (6 hex characters derived from the MAC). The Soft-AP
uses a static address `192.168.4.1/24` (ESP-IDF Soft-AP default, same as
WiFred) and a DHCP pool `192.168.4.50–200`. The wireless-programmer
source address `.2` is **outside** that pool. The driver advertises this via
`capabilities.commissioningNet`:

| Field    | Value          |
|----------|----------------|
| `host`   | `192.168.4.1`  |
| `port`   | `80`           |
| `source` | `192.168.4.2`  |
| `prefix` | `24`           |

The daemon should associate to the open AP, assign `192.168.4.2/24` on the
wireless interface (**no default route**), hand a sync `HttpClient` to the
driver, and release the radio on every exit path.

This subnet does **not** overlap the BigFred hub LAN (`192.168.0.0/24`), so
the `wp_link::netcfg` policy-route path for a locally-owned destination is
not needed for LongFred. The radio still installs the generic Soft-AP
sysctls; they are a no-op when `host` is not a local address.

Candidate identity: SSID prefix `longfred_prog`, stable key = BSSID.

## Capabilities

| Field                    | Value                              |
|--------------------------|------------------------------------|
| `maxRosterSlots`         | 12                                 |
| `maxFunctionIndex`       | 0 (no function maps via settings)  |
| `identityFormat`         | `Alphanumeric { max_len: 16 }`     |
| `supportsThrottleServer` | true (field accepted, unused)      |
| `supportsFirmwareUpdate` | true                               |
| `commissioning`          | `SoftAp`                           |
| `commissioningNet`       | `192.168.4.1` / source `.2` /24    |

`identity` is written as `wifi.hostname`. BigFred authentication uses the
optional `bigfred.login` / `bigfred.pin` fields on `ProgramRequest` (not the
6-digit wiThrottle pairing code used by WiFred).

## Read-back / probe

`GET /api/v1/settings` returns the device's current configuration as JSON,
including `device.variant` when the firmware exposes it. Probe returns that
JSON document as-is.

## Write sequence

1. `PUT /api/v1/settings` with a JSON body built from the request:
   - `wifi.ssid` / `wifi.password` / optional `wifi.hostname`
   - optional `bigfred.login` / `bigfred.pin`
   - optional `roster_mode` (`auto` / `static`)
   - `roster` entries as `{ "addr": "S3" }` / `{ "addr": "L128" }`
2. `GET /api/v1/settings` — verify WiFi SSID, hostname, BigFred login,
   roster mode and addresses.
3. `POST /api/v1/programming-mode/off` — clear the flag; the firmware resets
   and leaves the Soft-AP.

The PSK / PIN are never logged by the daemon.

## Firmware update

`POST /api/v1/firmware` with `Content-Type: application/octet-stream` and
the raw `.app.bin` body (ESP32-C6 app image, magic `0xE9`). Do not send a
merged flash dump.

- Soft-AP: same join as programming; after reboot `programming_mode` stays
  set so the device returns to the AP.
- LAN: HTTP to the layout IPv4 while the Firmware update menu is open;
  after reboot the device rejoins layout Wi‑Fi. Discover hosts via mDNS
  `_longfred-ota._tcp.local` (`scan --mode lan`).
- USB: `espflash` on a serial port (`scan --mode usb` / `--port`). ELF
  needs `--partition-table partitions.csv` (first install of the dual-slot
  table). Merged `.bin` is written at `0x0`; `.app.bin` at `ota_0`
  (`0x10000`). Requires `espflash` on `PATH`.

The HTTP transfer has a 120 s deadline and is not retried. USB `espflash`
has a 180 s deadline.

## Testing

Covered by unit tests in `longfred/discovery.rs` and `longfred/settings.rs`,
plus `crates/wp-drivers/tests/longfred_write.rs` which asserts the
PUT → GET → POST order and the PUT JSON shape.
