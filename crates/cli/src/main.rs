//! `dblocal` — one binary, two modes.
//!
//! With no arguments it opens the GUI. With a subcommand it runs the CLI.

use std::process::ExitCode;

fn main() -> ExitCode {
    dblocal_cli::cli::run(std::env::args_os().skip(1).collect())
}
