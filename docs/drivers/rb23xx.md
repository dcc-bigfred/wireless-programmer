# RB23xx driver

Implements [`wp_core::DeviceDriver`] for RailBOX **RB2300 / RB2310 / RB2302**
sound decoders in Wi-Fi file-browser mode.

## Commissioning model

With **F28** on, the decoder raises a **WPA2-PSK** Soft-AP named
`RB2300_XXXXX` (also `RB2310_` / `RB2302_`). The factory password
`000000000` is a publicly documented default for a temporary commissioning
AP, not a secret. The AP uses `192.168.4.1/24`. The daemon
assigns `192.168.4.2/24` on the wireless interface (**no default route**),
hands a sync `HttpClient` to the driver, and releases the radio on every
exit path.

| Field    | Value          |
|----------|----------------|
| `host`   | `192.168.4.1`  |
| `port`   | `80`           |
| `source` | `192.168.4.2`  |
| `prefix` | `24`           |

Candidate identity: SSID prefixes above, stable key = BSSID.

Turning F28 on is out of scope for this driver.

## Capabilities

| Field                    | Value                           |
|--------------------------|---------------------------------|
| `maxRosterSlots`         | 0                               |
| `maxFunctionIndex`       | 0                               |
| `identityFormat`         | `Any`                           |
| `supportsThrottleServer` | false                           |
| `supportsFirmwareUpdate` | true                            |
| `maxFirmwareBytes`       | 5 MiB                           |
| `firmwareModes`          | `ap`                            |
| `firmwareRequireEspAppBin` | false                         |
| `commissioning`          | `SoftAp`                        |
| `commissioningNet`       | `192.168.4.1` / source `.2` /24 |

`program` is not supported. Use `updateFirmware`.

## Read-back / probe

`GET /?p=/` confirms the file browser is up. Probe returns
`{ "ok": true, "bytes": <listing length> }`.

## WPA2-PSK join

The daemon derives the PMK from the SSID and the factory passphrase
(`000000000`) via PBKDF2-HMAC-SHA1 (4096 rounds, 32-byte output) and passes
it to `NL80211_CMD_CONNECT` with `privacy`, `WPA2`, CCMP, and
`AkmSuite::Psk`. This lets firmware that offloads the 4-way handshake
complete association without a userspace supplicant.

**Fallback.** brcmfmac on Raspberry Pi 5 does not offload the 4-way
handshake from a PMK passed via nl80211, so `CONNECT` never reaches
carrier. When that happens the daemon spawns `wpa_supplicant` on the
programming interface with a minimal config (SSID + PSK) for the duration
of the job. The supplicant runs in the foreground (not `-B`) and is killed
on `release`, so the radio is freed for the next job. The nl80211 path is
tried first; the fallback only fires when both nl80211 attempts (initial +
post-rescan) fail to produce carrier.

## Firmware update

Same HTTP contract as the `rb` CLI sound upload, aimed at the decoder
**root** (not a sound-pack slot):

```
POST /upload?p=/{basename.bin}
Content-Type: multipart/form-data
Content-Length: <n>

<raw bytes>
```

The Content-Type has **no multipart boundary**; the body is the file. Do
not rewrite this as a proper multipart form.

- Soft-AP only (`mode: "ap"`). LAN and USB return a driver error.
- Cap **5 MiB**. Deadline **120 s**, not retried.
- Success is HTTP 2xx **or** a TCP RST / close after the body is fully
  written (the decoder reboots). A RST during the write is a failure.

```bash
wireless-programmer scan
wireless-programmer update-firmware --mode ap --driver rb23xx \
  --key AA:BB:CC:DD:EE:01 --file RB_Sound_1.15.1.bin
```

## Testing

Covered by `rb23xx_discovery.rs`, `rb23xx_write.rs`, HTTP close-after-write
tests in `wp-link`, and fake-mode `updateFirmware` in
`crates/wireless-programmer/tests/fake_mode_test.rs`.
