//! `wireless-programmer` — daemon and CLI client for BigFred device programming.
//!
//! The same binary acts both as the long-running daemon (`wireless-programmer
//! daemon`, the default when no subcommand is given) and as a one-shot client
//! of that daemon (`wireless-programmer scan`, `wireless-programmer program`,
//! ...). The client subcommands are thin wrappers over [`wp_client`].

#![forbid(unsafe_code)]
#![allow(dead_code)]

mod cli;
mod config;
mod drivers;
mod ipc;
mod jobs;

use std::process::ExitCode;

use clap::Parser;
use cli::{Cli, Command};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Daemon(args)) => cli::run_daemon(args, cli.socket),
        Some(command) => cli::run_client(command, cli.socket),
        None => cli::run_daemon(
            cli::DaemonArgs {
                verbose: cli.verbose,
            },
            cli.socket,
        ),
    }
}
