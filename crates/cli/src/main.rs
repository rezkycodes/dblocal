//! `dblocal` — one binary, two modes.
//!
//! With no arguments it opens the GUI. With a subcommand it runs the CLI.

mod cli;
mod commands;
mod messages;
mod output;

use std::process::ExitCode;

fn main() -> ExitCode {
    cli::run(std::env::args_os().skip(1).collect())
}
