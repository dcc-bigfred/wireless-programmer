//! Client subcommand handlers.

use std::path::PathBuf;
use std::process::ExitCode;

use wp_client::{CandidateRef, ClientError};

use super::{
    build_client, resolve_socket, Command, IdentifyArgs, JobAction, JobArgs, ProbeArgs, ScanArgs,
};

type HandlerResult = Result<(), ClientError>;

/// Dispatch a client subcommand to its handler.
pub fn run(command: Command, socket_override: Option<PathBuf>) -> ExitCode {
    let socket = resolve_socket(socket_override.as_ref());
    let result: HandlerResult = match command {
        Command::Scan(a) => scan(&socket, a),
        Command::Probe(a) => probe(&socket, a),
        Command::Program(a) => super::program::run(&socket, a),
        Command::Identify(a) => identify(&socket, a),
        Command::LinkStatus => link_status(&socket),
        Command::Hello => hello(&socket),
        Command::Job(a) => job(&socket, a),
        Command::Daemon(_) => unreachable!("daemon is not a client command"),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn client(socket: &PathBuf, timeout: Option<humantime::Duration>) -> wp_client::Client {
    build_client(socket, timeout)
}

fn candidate(driver: &str, key: &str) -> CandidateRef {
    CandidateRef {
        driver: driver.to_string(),
        key: key.to_string(),
    }
}

fn print_scan(candidates: &[wp_client::CandidateWire], json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(candidates).unwrap_or_default()
        );
        return;
    }
    if candidates.is_empty() {
        println!("no candidates found");
        return;
    }
    println!("{:<10} {:<20} {:<8} LABEL", "DRIVER", "KEY", "RSSI");
    for c in candidates {
        let rssi = c.rssi.map(|r| r.to_string()).unwrap_or('-'.to_string());
        println!("{:<10} {:<20} {:<8} {}", c.driver, c.key, rssi, c.label);
    }
}

fn scan(socket: &PathBuf, args: ScanArgs) -> HandlerResult {
    let c = client(socket, args.common.timeout);
    let candidates = c.scan()?;
    print_scan(&candidates, args.common.json);
    Ok(())
}

fn probe(socket: &PathBuf, args: ProbeArgs) -> HandlerResult {
    let c = client(socket, args.common.timeout);
    let info = c.probe(candidate(&args.driver, &args.key))?;
    if args.common.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&info).unwrap_or_default()
        );
    } else {
        println!("driver:          {}", info.driver);
        println!("key:             {}", info.key);
        if let Some(fw) = &info.firmware_revision {
            println!("firmware:        {fw}");
        }
        if let Some(id) = &info.identity {
            println!("identity:        {id}");
        }
        if let Some(bat) = info.battery_mv {
            println!("battery:         {bat} mV");
        }
        if !info.roster.is_empty() {
            println!("roster:");
            for (i, e) in info.roster.iter().enumerate() {
                let addr = e.address.map(|a| a.to_string()).unwrap_or('-'.to_string());
                println!("  [{i}] addr={addr}");
            }
        }
    }
    Ok(())
}

fn identify(socket: &PathBuf, args: IdentifyArgs) -> HandlerResult {
    let c = client(socket, args.common.timeout);
    c.identify(candidate(&args.driver, &args.key), args.count)?;
    println!("identify: ok");
    Ok(())
}

fn link_status(socket: &PathBuf) -> HandlerResult {
    let c = client(socket, None);
    let s = c.link_status()?;
    println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default());
    Ok(())
}

fn hello(socket: &PathBuf) -> HandlerResult {
    let c = client(socket, None);
    let h = c.hello()?;
    println!("{}", serde_json::to_string_pretty(&h).unwrap_or_default());
    Ok(())
}

fn job(socket: &PathBuf, args: JobArgs) -> HandlerResult {
    let c = client(socket, args.common.timeout);
    match args.action {
        JobAction::Get { id } => {
            let snap = c.job_get(id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&snap).unwrap_or_default()
            );
        }
        JobAction::Watch { id } => {
            let stream = c.job_watch(id)?;
            let last = stream.drain()?;
            if let Some(f) = last {
                if args.common.json {
                    println!("{}", serde_json::to_string_pretty(&f).unwrap_or_default());
                } else {
                    eprintln!("job {}: {:?}", f.job_id, f.state);
                }
                use wp_client::JobStateWire;
                if !f.state.is_terminal() {
                    return Err(ClientError::UnexpectedResponse(
                        "stream ended before terminal state".into(),
                    ));
                }
                if matches!(f.state, JobStateWire::Failed | JobStateWire::Cancelled) {
                    return Err(ClientError::Server {
                        code: f.state.to_string(),
                        message: f.detail.unwrap_or_default(),
                    });
                }
            }
        }
        JobAction::Cancel { id } => {
            let snap = c.job_cancel(id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&snap).unwrap_or_default()
            );
        }
    }
    Ok(())
}
