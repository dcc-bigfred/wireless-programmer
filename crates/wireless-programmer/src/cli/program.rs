//! `program` subcommand: build a request, start a job, and stream progress.

use std::path::PathBuf;

use wp_client::{
    ClientError, JobStateWire, ProgramRequestWire, RosterEntryWire, ThrottleServerWire,
    WifiCredentialsWire,
};

use super::{build_client, ProgramArgs};

/// Run the `program` subcommand.
pub fn run(socket: &PathBuf, args: ProgramArgs) -> Result<(), ClientError> {
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

    let stream = client.job_watch(result.job_id.clone())?;
    let last = stream.drain()?;
    match last {
        None => Err(ClientError::UnexpectedResponse(
            "stream closed before completion".into(),
        )),
        Some(frame) => {
            if args.common.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&frame).unwrap_or_default()
                );
            } else {
                eprintln!(
                    "job {}: {:?}{}",
                    frame.job_id,
                    frame.state,
                    frame
                        .detail
                        .as_deref()
                        .map(|d| format!(" — {d}"))
                        .unwrap_or_default()
                );
            }
            match frame.state {
                JobStateWire::Done => Ok(()),
                JobStateWire::Failed | JobStateWire::Cancelled => Err(ClientError::Server {
                    code: frame.state.to_string(),
                    message: frame.detail.unwrap_or_default(),
                }),
                _ => Err(ClientError::UnexpectedResponse(
                    "stream ended before terminal state".into(),
                )),
            }
        }
    }
}

/// Construct the wire request from `--request-file` or the individual flags.
fn build_request(args: &ProgramArgs) -> Result<ProgramRequestWire, ClientError> {
    if let Some(path) = &args.request_file {
        let body = read_json(path)?;
        return Ok(body);
    }

    let roster = match &args.roster_file {
        Some(path) => read_json::<Vec<RosterEntryWire>>(path)?,
        None => Vec::new(),
    };

    let wifi = WifiCredentialsWire {
        ssid: args.wifi_ssid.clone().ok_or_else(|| {
            ClientError::UnexpectedResponse(
                "--wifi-ssid is required (or use --request-file)".into(),
            )
        })?,
        psk: args.wifi_psk.clone(),
    };

    let automatic = if args.server_automatic {
        Some(true)
    } else {
        None
    };
    let server = ThrottleServerWire {
        host: args.server_host.clone().ok_or_else(|| {
            ClientError::UnexpectedResponse(
                "--server-host is required (or use --request-file)".into(),
            )
        })?,
        port: args.server_port.ok_or_else(|| {
            ClientError::UnexpectedResponse(
                "--server-port is required (or use --request-file)".into(),
            )
        })?,
        automatic,
    };

    Ok(ProgramRequestWire {
        identity: args.identity.clone().ok_or_else(|| {
            ClientError::UnexpectedResponse("--identity is required (or use --request-file)".into())
        })?,
        wifi,
        server,
        roster,
    })
}

/// Read a JSON file and decode it.
fn read_json<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T, ClientError> {
    let bytes = std::fs::read(path)
        .map_err(|e| ClientError::UnexpectedResponse(format!("read {}: {e}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| ClientError::UnexpectedResponse(format!("decode {}: {e}", path.display())))
}
