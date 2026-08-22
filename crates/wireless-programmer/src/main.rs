//! `wireless-programmer` — daemon and CLI client for BigFred device programming.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::Parser;
use wireless_programmer::cli::{self, Cli, Command};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Daemon(mut args)) => {
            if args.interface.is_none() {
                args.interface = cli.interface;
            }
            if !args.verbose {
                args.verbose = cli.verbose;
            }
            if args.log_level.is_none() {
                args.log_level = cli.log_level;
            }
            if !args.require_auth {
                args.require_auth = cli.require_auth;
            }
            if args.allow_users.is_none() {
                args.allow_users = cli.allow_users;
            }
            cli::run_daemon(args, cli.socket)
        }
        Some(Command::Fake(mut args)) => {
            if !args.verbose {
                args.verbose = cli.verbose;
            }
            if args.log_level.is_none() {
                args.log_level = cli.log_level;
            }
            cli::run_fake(args)
        }
        Some(command) => cli::run_client(command, cli.socket),
        None => cli::run_daemon(
            cli::DaemonArgs {
                verbose: cli.verbose,
                log_level: cli.log_level,
                interface: cli.interface,
                require_auth: cli.require_auth,
                allow_users: cli.allow_users,
                fake_webserver_port: None,
            },
            cli.socket,
        ),
    }
}
