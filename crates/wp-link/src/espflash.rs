//! Invoke the `espflash` CLI to list USB serial ports and flash LongFred.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use wp_core::DriverError;

/// LongFred is ESP32-C6.
pub const CHIP: &str = "esp32c6";

/// `ota_0` offset in LongFred `partitions.csv`.
pub const OTA0_OFFSET: u32 = 0x1_0000;

/// Dual-slot table (`ota_0` + `ota_1` + metadata) needs an 8 MiB chip.
pub const FLASH_SIZE: &str = "8mb";

/// USB `espflash` deadline (erase + write of a full image).
pub const USB_FLASH_DEADLINE: Duration = Duration::from_secs(180);

/// A USB serial device that may be a LongFred UART / USB-Serial-JTAG port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbPort {
    /// Device node (e.g. `/dev/ttyACM0`).
    pub path: String,
    /// Human-readable label.
    pub label: String,
}

/// How to flash a firmware file over USB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageKind {
    /// ELF: `espflash flash --partition-table`.
    Elf,
    /// App image (`.app.bin`, magic `0xE9`): `write-bin` at [`OTA0_OFFSET`].
    AppBin {
        /// Flash offset.
        offset: u32,
    },
    /// Merged flash dump (`save-image --merge`): `write-bin` at 0x0.
    MergedBin {
        /// Flash offset.
        offset: u32,
    },
}

/// Classify an image from its path, header bytes, and length.
///
/// # Errors
///
/// Returns a message when the file is not an ELF, ESP app image, or merged dump.
pub fn classify_image(path: &Path, header: &[u8], file_len: u64) -> Result<ImageKind, String> {
    if header.len() >= 4 && header[..4] == [0x7f, b'E', b'L', b'F'] {
        return Ok(ImageKind::Elf);
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.ends_with(".app.bin") {
        return Ok(ImageKind::AppBin {
            offset: OTA0_OFFSET,
        });
    }
    if header.first() != Some(&0xE9) {
        return Err(format!(
            "{} is not an ELF or ESP32 image (expected ELF magic or 0xE9)",
            path.display()
        ));
    }
    if name.ends_with(".bin") && file_len > u64::from(OTA0_OFFSET) {
        return Ok(ImageKind::MergedBin { offset: 0 });
    }
    Ok(ImageKind::AppBin {
        offset: OTA0_OFFSET,
    })
}

/// Look for `partitions.csv` next to the image when the caller did not pass one.
#[must_use]
pub fn resolve_partition_table(image: &Path, explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    let dir = image.parent()?;
    for name in ["partitions.csv", "partition-table.csv"] {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn before_reset(port: &str) -> &'static str {
    if port.contains("ttyACM") || port.contains("usbmodem") {
        "usb-reset"
    } else {
        "default-reset"
    }
}

/// Build the `espflash` argv (not including the program name).
///
/// # Errors
///
/// ELF flashes require a partition table so LongFred dual-slot layout is used
/// instead of the bundled espflash default.
pub fn flash_argv(
    kind: &ImageKind,
    port: &str,
    image: &Path,
    partition_table: Option<&Path>,
) -> Result<Vec<String>, String> {
    let image = image.display().to_string();
    let before = before_reset(port);
    let mut args = vec![
        String::new(), // filled below
        "--non-interactive".into(),
        "--skip-update-check".into(),
        "--chip".into(),
        CHIP.into(),
        "--port".into(),
        port.into(),
        "--before".into(),
        before.into(),
    ];
    match kind {
        ImageKind::Elf => {
            let table = partition_table.ok_or_else(|| {
                "ELF USB flash needs --partition-table (LongFred partitions.csv)".to_string()
            })?;
            args[0] = "flash".into();
            args.push("--flash-size".into());
            args.push(FLASH_SIZE.into());
            args.push("--partition-table".into());
            args.push(table.display().to_string());
            args.push(image);
        }
        ImageKind::AppBin { offset } | ImageKind::MergedBin { offset } => {
            args[0] = "write-bin".into();
            args.push(format!("{offset:#x}"));
            args.push(image);
        }
    }
    Ok(args)
}

/// Parse `espflash list-ports -n` (one device path per line).
#[must_use]
pub fn parse_list_ports_output(stdout: &str) -> Vec<UsbPort> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let path = line.trim();
        if path.is_empty() || path.starts_with('#') {
            continue;
        }
        if !looks_like_serial_path(path) {
            continue;
        }
        out.push(port_from_path(path));
    }
    out
}

fn looks_like_serial_path(path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name.starts_with("ttyusb")
        || name.starts_with("ttyacm")
        || name.starts_with("cu.usb")
        || name.starts_with("cu.wch")
        || path.starts_with("/dev/")
}

fn port_from_path(path: &str) -> UsbPort {
    let label = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    UsbPort {
        path: path.to_string(),
        label,
    }
}

fn list_dev_serial_nodes() -> Vec<UsbPort> {
    let Ok(entries) = std::fs::read_dir("/dev") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ent in entries.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("ttyUSB") || name.starts_with("ttyACM")) {
            continue;
        }
        let path = format!("/dev/{name}");
        out.push(port_from_path(&path));
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

/// Enumerate USB serial ports (`espflash list-ports`, then `/dev/ttyUSB*` / `ttyACM*`).
///
/// # Errors
///
/// Returns [`io::Error`] only when spawning `espflash` fails for a reason other
/// than a missing binary (missing binary falls back to `/dev`).
pub fn list_usb_ports() -> io::Result<Vec<UsbPort>> {
    match Command::new("espflash")
        .args(["list-ports", "-n", "-S"])
        .env("ESPFLASH_SKIP_UPDATE_CHECK", "true")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        Ok(out) if out.status.success() => {
            let parsed = parse_list_ports_output(&String::from_utf8_lossy(&out.stdout));
            if parsed.is_empty() {
                Ok(list_dev_serial_nodes())
            } else {
                Ok(parsed)
            }
        }
        Ok(_) | Err(_) => Ok(list_dev_serial_nodes()),
    }
}

/// Flash `image` onto `port` with the `espflash` CLI.
///
/// # Errors
///
/// Returns [`DriverError`] when the file cannot be classified, `espflash` is
/// missing, or the process fails / times out.
pub fn flash(port: &str, image: &Path, partition_table: Option<&Path>) -> Result<(), DriverError> {
    let mut header = [0u8; 16];
    let mut f = std::fs::File::open(image).map_err(|e| DriverError::Other(e.to_string()))?;
    let n = f
        .read(&mut header)
        .map_err(|e| DriverError::Other(e.to_string()))?;
    let file_len = f
        .metadata()
        .map(|m| m.len())
        .unwrap_or(0)
        .max(u64::try_from(n).unwrap_or(0));
    let kind = classify_image(image, &header[..n], file_len).map_err(DriverError::Other)?;
    let table = resolve_partition_table(image, partition_table);
    let argv = flash_argv(&kind, port, image, table.as_deref()).map_err(DriverError::Other)?;
    run_espflash(&argv)
}

fn run_espflash(argv: &[String]) -> Result<(), DriverError> {
    let Some((sub, rest)) = argv.split_first() else {
        return Err(DriverError::Other("empty espflash argv".into()));
    };
    let mut cmd = Command::new("espflash");
    cmd.arg(sub)
        .args(rest)
        .env("ESPFLASH_SKIP_UPDATE_CHECK", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            DriverError::Other("espflash not found in PATH".into())
        } else {
            DriverError::Other(format!("spawn espflash: {e}"))
        }
    })?;
    let deadline = Instant::now() + USB_FLASH_DEADLINE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(());
                }
                let mut stderr = String::new();
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_string(&mut stderr);
                }
                let msg = stderr.trim();
                return Err(DriverError::Other(if msg.is_empty() {
                    format!("espflash {sub} failed ({status})")
                } else {
                    format!("espflash {sub} failed: {msg}")
                }));
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DriverError::Other(format!(
                    "espflash timed out after {}s",
                    USB_FLASH_DEADLINE.as_secs()
                )));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(DriverError::Other(format!("wait espflash: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_elf_magic() {
        let k = classify_image(Path::new("fw.elf"), b"\x7fELF\x01\x01", 100).unwrap();
        assert_eq!(k, ImageKind::Elf);
    }

    #[test]
    fn classifies_app_bin_suffix() {
        let k = classify_image(Path::new("longfred.app.bin"), &[0xE9, 0, 0, 0], 1024).unwrap();
        assert_eq!(
            k,
            ImageKind::AppBin {
                offset: OTA0_OFFSET
            }
        );
    }

    #[test]
    fn classifies_merged_bin_by_size() {
        let k = classify_image(Path::new("longfred.bin"), &[0xE9, 0, 0, 0], 0x20_0000).unwrap();
        assert_eq!(k, ImageKind::MergedBin { offset: 0 });
    }

    #[test]
    fn small_e9_without_app_suffix_is_app() {
        let k = classify_image(Path::new("fw.bin"), &[0xE9, 0], 4096).unwrap();
        assert_eq!(
            k,
            ImageKind::AppBin {
                offset: OTA0_OFFSET
            }
        );
    }

    #[test]
    fn rejects_unknown() {
        assert!(classify_image(Path::new("fw.txt"), b"hello", 5).is_err());
    }

    #[test]
    fn elf_argv_requires_partition_table() {
        let kind = ImageKind::Elf;
        assert!(flash_argv(&kind, "/dev/ttyUSB0", Path::new("a.elf"), None).is_err());
        let argv = flash_argv(
            &kind,
            "/dev/ttyUSB0",
            Path::new("a.elf"),
            Some(Path::new("partitions.csv")),
        )
        .unwrap();
        assert_eq!(argv[0], "flash");
        assert!(argv.contains(&"--partition-table".into()));
        assert!(argv.contains(&"partitions.csv".into()));
        assert!(argv.contains(&"--flash-size".into()));
        assert!(argv.contains(&"default-reset".into()));
    }

    #[test]
    fn acm_uses_usb_reset() {
        let argv = flash_argv(
            &ImageKind::AppBin {
                offset: OTA0_OFFSET,
            },
            "/dev/ttyACM0",
            Path::new("a.app.bin"),
            None,
        )
        .unwrap();
        assert_eq!(argv[0], "write-bin");
        assert!(argv.contains(&"usb-reset".into()));
        assert!(argv.contains(&"0x10000".into()));
    }

    #[test]
    fn parse_name_only_ports() {
        let ports = parse_list_ports_output("/dev/ttyUSB0\n/dev/ttyACM0\n\n");
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].path, "/dev/ttyUSB0");
        assert_eq!(ports[1].label, "ttyACM0");
    }
}
