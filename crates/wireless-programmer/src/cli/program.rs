//! `program` subcommand: build a request, start a job, and stream progress.

use std::io::Read;
use std::path::Path;

use wp_client::{ProgramRequestWire, RosterEntryWire, ThrottleServerWire, WifiCredentialsWire};

use super::client::{outcome, print_frame};
use super::{build_client, CliError, ProgramArgs};

/// Default wiThrottle port, used when `--server-automatic` leaves the port
/// unspecified (the device discovers the host but still stores a port).
const DEFAULT_WITHROTTLE_PORT: u16 = 12090;

/// Run the `program` subcommand.
pub fn run(socket: &Path, args: ProgramArgs) -> Result<(), CliError> {
    let client = build_client(socket, args.common.timeout);
    let request = build_request(&args)?;
    let candidate = wp_client::CandidateRef {
        driver: args.driver.clone(),
        key: args.key.clone(),
    };

    let result = client.program(candidate, request)?;
    eprintln!("job started: {}", result.job_id);

    if args.no_watch {
        println!("{}", result.job_id);
        return Ok(());
    }

    let json = args.common.json;
    let last = client
        .job_watch(result.job_id)?
        .drain_with(|frame| print_frame(frame, json))?;
    outcome(last)
}

/// Construct the wire request from `--request-file` or the individual flags.
fn build_request(args: &ProgramArgs) -> Result<ProgramRequestWire, CliError> {
    if let Some(path) = &args.request_file {
        return read_json(path);
    }

    let roster = match &args.roster_file {
        Some(path) => read_json::<Vec<RosterEntryWire>>(path)?,
        None => Vec::new(),
    };

    let wifi = WifiCredentialsWire {
        ssid: required(args.wifi_ssid.clone(), "--wifi-ssid")?,
        psk: wifi_psk(args)?,
    };

    // With mDNS discovery the device finds the host itself, so a fixed host
    // and port stop being mandatory.
    let server = if args.server_automatic {
        ThrottleServerWire {
            host: args.server_host.clone().unwrap_or_default(),
            port: args.server_port.unwrap_or(DEFAULT_WITHROTTLE_PORT),
            automatic: Some(true),
        }
    } else {
        ThrottleServerWire {
            host: required(args.server_host.clone(), "--server-host")?,
            port: required(args.server_port, "--server-port")?,
            automatic: None,
        }
    };

    Ok(ProgramRequestWire {
        identity: required(args.identity.clone(), "--identity")?,
        wifi,
        server,
        roster,
    })
}

/// Resolve the PSK from `--wifi-psk`, `--wifi-psk-file`, or neither (an open
/// network).
fn wifi_psk(args: &ProgramArgs) -> Result<Option<String>, CliError> {
    if let Some(psk) = &args.wifi_psk {
        return Ok(Some(psk.clone()));
    }
    let Some(path) = &args.wifi_psk_file else {
        return Ok(None);
    };
    let raw = if path.as_os_str() == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| CliError::File {
                path: "<stdin>".into(),
                message: e.to_string(),
            })?;
        buf
    } else {
        std::fs::read_to_string(path).map_err(|e| CliError::File {
            path: path.display().to_string(),
            message: e.to_string(),
        })?
    };
    let psk = raw.trim_end_matches(['\n', '\r']).to_string();
    if psk.is_empty() {
        return Err(CliError::Usage(
            "PSK source is empty; omit --wifi-psk/--wifi-psk-file for an open network".into(),
        ));
    }
    Ok(Some(psk))
}

/// Require a flag that only `--request-file` can substitute for.
fn required<T>(value: Option<T>, flag: &str) -> Result<T, CliError> {
    value.ok_or_else(|| CliError::Usage(format!("{flag} is required (or use --request-file)")))
}

/// Read a JSON file and decode it.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, CliError> {
    let bytes = std::fs::read(path).map_err(|e| CliError::File {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    serde_json::from_slice(&bytes).map_err(|e| CliError::File {
        path: path.display().to_string(),
        message: e.to_string(),
    })
}
