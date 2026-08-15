//! Client subcommand handlers.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use wp_client::{CandidateRef, JobFrame, JobStateWire};

use super::{
    build_client, resolve_socket, CliError, Command, CommonArgs, IdentifyArgs, JobAction, JobArgs,
    ProbeArgs, ScanArgs, UpdateFirmwareArgs,
};

type HandlerResult = Result<(), CliError>;

/// Dispatch a client subcommand to its handler.
pub fn run(command: Command, socket_override: Option<PathBuf>) -> ExitCode {
    let socket = resolve_socket(socket_override.as_ref());
    let result: HandlerResult = match command {
        Command::Scan(a) => scan(&socket, a),
        Command::Probe(a) => probe(&socket, a),
        Command::Program(a) => super::program::run(&socket, a),
        Command::UpdateFirmware(a) => update_firmware(&socket, a),
        Command::Identify(a) => identify(&socket, a),
        Command::LinkStatus(a) => link_status(&socket, a),
        Command::Hello(a) => hello(&socket, a),
        Command::Job(a) => job(&socket, a),
        Command::Daemon(_) => unreachable!("daemon is not a client command"),
        Command::Fake(_) => unreachable!("fake is not a client command"),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn candidate(driver: &str, key: &str) -> CandidateRef {
    CandidateRef {
        driver: driver.to_string(),
        key: key.to_string(),
    }
}

/// Print a value as pretty JSON on stdout.
pub(crate) fn print_json<T: serde::Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

/// Render one progress frame: compact JSON per line for `--json` (so a
/// consumer can read the stream incrementally), otherwise a human line on
/// stderr, leaving stdout free for results.
pub(crate) fn print_frame(frame: &JobFrame, json: bool) {
    if json {
        println!("{}", serde_json::to_string(frame).unwrap_or_default());
        return;
    }
    let detail = frame
        .detail
        .as_deref()
        .map(|d| format!(" — {d}"))
        .unwrap_or_default();
    let progress = frame.progress.map(|p| format!(" {p}%")).unwrap_or_default();
    eprintln!("job {}: {}{progress}{detail}", frame.job_id, frame.state);
}

/// Turn the terminal frame of a watch stream into a process outcome.
pub(crate) fn outcome(last: Option<JobFrame>) -> HandlerResult {
    let Some(frame) = last else {
        return Err(CliError::Truncated);
    };
    match frame.state {
        JobStateWire::Done => Ok(()),
        JobStateWire::Failed | JobStateWire::Cancelled => Err(CliError::Job {
            state: frame.state.to_string(),
            detail: frame.detail.map(|d| format!(": {d}")).unwrap_or_default(),
        }),
        _ => Err(CliError::Truncated),
    }
}

fn print_scan(candidates: &[wp_client::CandidateWire], json: bool) {
    if json {
        print_json(&candidates);
        return;
    }
    if candidates.is_empty() {
        println!("no candidates found");
        return;
    }
    println!("{:<10} {:<20} {:<8} LABEL", "DRIVER", "KEY", "RSSI");
    for c in candidates {
        let rssi = c.rssi.map_or_else(|| "-".to_string(), |r| r.to_string());
        println!("{:<10} {:<20} {:<8} {}", c.driver, c.key, rssi, c.label);
    }
}

fn scan(socket: &Path, args: ScanArgs) -> HandlerResult {
    let c = build_client(socket, args.common.timeout);
    let mode = parse_reach_mode(&args.mode);
    let candidates = c.scan_mode(mode)?;
    print_scan(&candidates, args.common.json);
    Ok(())
}

fn parse_reach_mode(mode: &str) -> wp_client::ReachMode {
    match mode {
        "lan" => wp_client::ReachMode::Lan,
        "usb" => wp_client::ReachMode::Usb,
        _ => wp_client::ReachMode::Ap,
    }
}

fn update_firmware(socket: &Path, args: UpdateFirmwareArgs) -> HandlerResult {
    if !args.file.is_file() {
        return Err(CliError::File {
            path: args.file.display().to_string(),
            message: "not a file".into(),
        });
    }
    let mode = if args.port.is_some() || args.mode == "usb" {
        wp_client::ReachMode::Usb
    } else if args.mode == "lan" || args.host.is_some() {
        wp_client::ReachMode::Lan
    } else {
        wp_client::ReachMode::Ap
    };
    let key = args
        .key
        .clone()
        .or_else(|| args.port.clone())
        .or_else(|| args.host.clone());
    if key.is_none() && mode != wp_client::ReachMode::Usb {
        return Err(CliError::Usage("provide --key and/or --host".into()));
    }
    let c = build_client(socket, args.common.timeout);
    let candidate = key.map(|key| wp_client::CandidateRef {
        driver: args.driver.clone(),
        key,
    });
    let started = c.update_firmware(
        mode,
        candidate,
        args.file.display().to_string(),
        args.host.clone(),
        args.port.clone(),
        args.partition_table
            .as_ref()
            .map(|p| p.display().to_string()),
    )?;
    if args.no_watch {
        if args.common.json {
            print_json(&started);
        } else {
            println!("job {}", started.job_id);
        }
        return Ok(());
    }
    let json = args.common.json;
    let last = c
        .job_watch(&started.job_id)?
        .drain_with(|frame| print_frame(frame, json))?;
    outcome(last)
}

fn probe(socket: &Path, args: ProbeArgs) -> HandlerResult {
    let c = build_client(socket, args.common.timeout);
    let info = c.probe(candidate(&args.driver, &args.key))?;
    if args.common.json {
        print_json(&info);
        return Ok(());
    }
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
            let addr = e.address.map_or_else(|| "-".to_string(), |a| a.to_string());
            println!("  [{i}] addr={addr}");
        }
    }
    Ok(())
}

fn identify(socket: &Path, args: IdentifyArgs) -> HandlerResult {
    let c = build_client(socket, args.common.timeout);
    c.identify(candidate(&args.driver, &args.key), args.count)?;
    println!("identify: ok");
    Ok(())
}

fn link_status(socket: &Path, args: CommonArgs) -> HandlerResult {
    let c = build_client(socket, args.common.timeout);
    let s = c.link_status()?;
    if args.common.json {
        print_json(&s);
        return Ok(());
    }
    println!("busy:            {}", s.busy);
    println!("interface:       {}", s.interface.as_deref().unwrap_or("-"));
    println!("rfkill blocked:  {}", s.rfkill_blocked);
    Ok(())
}

fn hello(socket: &Path, args: CommonArgs) -> HandlerResult {
    let c = build_client(socket, args.common.timeout);
    let h = c.hello()?;
    if args.common.json {
        print_json(&h);
        return Ok(());
    }
    println!("version:         {}", h.version);
    if let Some(commit) = &h.commit {
        println!("commit:          {commit}");
    }
    println!("drivers:");
    for d in &h.drivers {
        println!("  {} — {}", d.id, d.name);
    }
    Ok(())
}

fn job(socket: &Path, args: JobArgs) -> HandlerResult {
    let c = build_client(socket, args.common.timeout);
    match args.action {
        JobAction::Get { id } => {
            let snap = c.job_get(id)?;
            print_json(&snap);
        }
        JobAction::Watch { id } => {
            let json = args.common.json;
            let last = c
                .job_watch(id)?
                .drain_with(|frame| print_frame(frame, json))?;
            return outcome(last);
        }
        JobAction::Cancel { id } => {
            let snap = c.job_cancel(id)?;
            print_json(&snap);
        }
    }
    Ok(())
}
