//! Command-line surface and dispatch.

use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use dblocal_core::paths::DblPaths;
use dblocal_core::service::{DblService, LocalService, ServiceOptions};
use dblocal_core::DblError;

use crate::commands::{self, Ctx};
use crate::messages;
use crate::output;

/// Exit code for success, per `F-CLI`.
pub const EXIT_OK: u8 = 0;
/// Exit code for a general error, per `F-CLI`.
pub const EXIT_GENERAL: u8 = 1;
/// Exit code for bad arguments, per `F-CLI`.
pub const EXIT_BAD_ARGS: u8 = 2;
/// Exit code for an instance that does not exist, per `F-CLI`.
pub const EXIT_NOT_FOUND: u8 = 3;
/// Exit code for an engine that failed to start, per `F-CLI`.
pub const EXIT_ENGINE_FAILED: u8 = 4;

/// DBLocal command-line interface.
#[derive(Debug, Parser)]
#[command(
    name = "dblocal",
    version,
    about = "DBLocal — local database servers without containers",
    long_about = None
)]
pub struct Cli {
    /// Subcommand to run. Without one, the GUI opens.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Every subcommand in PRD §8.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Lists instances.
    List {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Prints the status of one instance.
    Status {
        /// Instance name.
        name: String,
    },
    /// Starts an instance.
    Start {
        /// Instance name.
        name: String,
        /// Seconds to wait for the instance to become healthy.
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
    /// Stops an instance.
    Stop {
        /// Instance name.
        name: String,
    },
    /// Prints a connection URI.
    Uri {
        /// Instance name.
        name: String,
        /// Emit a `.env` block instead of a bare URI.
        #[arg(long)]
        env: bool,
    },
    /// Creates an instance.
    Create {
        /// Engine key: postgresql, mysql, mariadb or valkey.
        #[arg(long)]
        engine: String,
        /// Engine version tag.
        #[arg(long)]
        version: String,
        /// Instance name.
        #[arg(long)]
        name: String,
        /// Port to bind.
        #[arg(long)]
        port: Option<u16>,
        /// Create without a password.
        #[arg(long = "no-password")]
        no_password: bool,
    },
    /// Deletes an instance.
    Delete {
        /// Instance name.
        name: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Also delete the data directory instead of archiving it.
        #[arg(long)]
        purge: bool,
    },
    /// Prints instance logs.
    Logs {
        /// Instance name.
        name: String,
        /// Follow the log file.
        #[arg(long, short)]
        follow: bool,
        /// Number of lines to show from the end.
        #[arg(long, default_value_t = 200)]
        lines: usize,
    },
}

/// Entry point used by `main`; returns the process exit code.
pub fn run(args: Vec<OsString>) -> ExitCode {
    let cli = match Cli::try_parse_from(std::iter::once(OsString::from("dblocal")).chain(args)) {
        Ok(cli) => cli,
        Err(err) => return clap_exit(err),
    };

    match cli.command {
        None => match dblocal_gui::run() {
            Ok(()) => ExitCode::from(EXIT_OK),
            Err(err) => report(err),
        },
        Some(command) => run_command(command),
    }
}

fn run_command(command: Command) -> ExitCode {
    let mut out = io::stdout().lock();
    let mut err = io::stderr().lock();
    let code = run_with_service(&command, &mut out, &mut err, output::is_interactive_stdin(), output::is_tty_stdout(), output::is_tty_stderr());
    ExitCode::from(code)
}

/// Builds the real service and dispatches a command. Testable via `run_with`.
fn run_with_service(
    command: &Command,
    out: &mut dyn Write,
    err: &mut dyn Write,
    is_interactive: bool,
    stdout_is_tty: bool,
    stderr_is_tty: bool,
) -> u8 {
    let paths = match DblPaths::from_env() {
        Ok(p) => p,
        Err(e) => {
            output::stderr_line(err, &messages::service_open_error(&e.diagnostic()));
            return EXIT_GENERAL;
        }
    };
    let service = match LocalService::new(paths, ServiceOptions::default()) {
        Ok(s) => s,
        Err(e) => {
            output::stderr_line(err, &messages::service_open_error(&e.diagnostic()));
            return EXIT_GENERAL;
        }
    };
    run_with(&service, command, out, err, is_interactive, stdout_is_tty, stderr_is_tty)
}

/// Dispatches a command against an injected service. This is the library entry
/// used by tests: it takes `&dyn DblService`, injected writers, and an
/// `is_interactive` flag.
pub fn run_with(
    service: &dyn DblService,
    command: &Command,
    out: &mut dyn Write,
    err: &mut dyn Write,
    is_interactive: bool,
    stdout_is_tty: bool,
    stderr_is_tty: bool,
) -> u8 {
    let mut ctx = Ctx {
        service,
        out,
        err,
        is_interactive,
        stdout_is_tty,
        stderr_is_tty,
    };
    commands::dispatch(&mut ctx, command)
}

fn clap_exit(err: clap::Error) -> ExitCode {
    // `--help` and `--version` are successful outcomes, not failures.
    let code = if err.use_stderr() { EXIT_BAD_ARGS } else { EXIT_OK };
    let _ = err.print();
    ExitCode::from(code)
}

fn report(err: DblError) -> ExitCode {
    eprintln!("{}", err.diagnostic());
    ExitCode::from(output::code_for(&err))
}

/// Maps an error to the exit code contract in `F-CLI`.
#[cfg(test)]
pub fn code_for(err: &DblError) -> u8 {
    output::code_for(err)
}

#[cfg(test)]
pub fn parse(args: &[&str]) -> dblocal_core::Result<Cli> {
    Cli::try_parse_from(std::iter::once("dblocal").chain(args.iter().copied())).map_err(|err| {
        DblError::InvalidInput {
            field: "arguments",
            reason: err.to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_prd_command_parses() {
        let cases: &[&[&str]] = &[
            &["list"],
            &["list", "--json"],
            &["status", "app"],
            &["start", "app"],
            &["start", "app", "--timeout", "30"],
            &["stop", "app"],
            &["uri", "app"],
            &["uri", "app", "--env"],
            &[
                "create",
                "--engine",
                "postgresql",
                "--version",
                "18.7",
                "--name",
                "app",
                "--port",
                "5432",
                "--no-password",
            ],
            &["delete", "app", "--yes", "--purge"],
            &["logs", "app", "--follow"],
            &["logs", "app", "--lines", "50"],
        ];
        for args in cases {
            parse(args).unwrap_or_else(|err| panic!("{args:?}: {err}"));
        }
    }

    #[test]
    fn no_arguments_means_gui() {
        let cli = parse(&[]).expect("parses");
        assert!(cli.command.is_none());
    }

    #[test]
    fn unknown_subcommand_is_rejected() {
        assert!(parse(&["frobnicate"]).is_err());
        assert!(parse(&["start"]).is_err(), "missing required name");
        assert!(parse(&["create", "--engine", "postgresql"]).is_err());
    }

    #[test]
    fn unknown_subcommand_exits_two() {
        let code = run(vec![OsString::from("frobnicate")]);
        assert_eq!(code, ExitCode::from(EXIT_BAD_ARGS));
    }

    #[test]
    fn help_succeeds() {
        let code = run(vec![OsString::from("--help")]);
        assert_eq!(code, ExitCode::from(EXIT_OK));
    }

    #[test]
    fn exit_codes_follow_the_contract() {
        assert_eq!(
            code_for(&DblError::NotFound {
                kind: "instance",
                name: "x".into()
            }),
            EXIT_NOT_FOUND
        );
        assert_eq!(
            code_for(&DblError::Engine {
                reason: "boom".into()
            }),
            EXIT_ENGINE_FAILED
        );
        assert_eq!(
            code_for(&DblError::InvalidInput {
                field: "name",
                reason: "bad".into()
            }),
            EXIT_BAD_ARGS
        );
        assert_eq!(
            code_for(&DblError::InvalidTransition {
                from: "stopped",
                to: "running"
            }),
            EXIT_BAD_ARGS
        );
        assert_eq!(code_for(&DblError::NotImplemented("x".into())), EXIT_GENERAL);
        assert_eq!(code_for(&DblError::Storage { reason: "x".into() }), EXIT_GENERAL);
    }
}
