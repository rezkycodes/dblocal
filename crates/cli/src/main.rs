//! `dblocal` — one binary, two modes.
//!
//! With no arguments it opens the GUI. With a subcommand it runs the CLI. Every
//! subcommand is a stub in Phase 0 and returns `DblError::NotImplemented`.

mod cli;

use std::process::ExitCode;

fn main() -> ExitCode {
    cli::run(std::env::args_os().skip(1).collect())
}
