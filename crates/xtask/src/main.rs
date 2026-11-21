//! Developer tasks, run through `cargo xtask`.

mod catalog;
mod limit;

use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

/// Exit code for a failed task.
const EXIT_FAILED: u8 = 1;

/// Wall-clock limits of the CI gates. They are several times the warm run (about
/// seven minutes in total) so a cold GPUI build on a slow runner still passes,
/// while a hung test cannot block the pipeline for an hour.
const FMT_LIMIT: Duration = Duration::from_secs(5 * 60);
const CLIPPY_LIMIT: Duration = Duration::from_secs(40 * 60);
const TESTS_LIMIT: Duration = Duration::from_secs(50 * 60);

/// The same failed exit code, as an [`ExitCode`], for submodules.
pub(crate) const EXIT_FAILED_CODE: ExitCode = ExitCode::FAILURE;

/// Build, packaging and CI helpers.
#[derive(Debug, Parser)]
#[command(name = "xtask", about = "DBLocal developer tasks")]
struct Xtask {
    #[command(subcommand)]
    task: Task,
}

/// Available tasks.
#[derive(Debug, Subcommand)]
enum Task {
    /// Runs fmt check, clippy and tests, exactly as CI does.
    Ci,
    /// Builds or validates the bundled engine catalog.
    Catalog {
        #[command(subcommand)]
        command: catalog::CatalogCommand,
    },
}

fn main() -> ExitCode {
    let xtask = match Xtask::try_parse() {
        Ok(xtask) => xtask,
        Err(err) => {
            let _ = err.print();
            return ExitCode::from(if err.use_stderr() { EXIT_FAILED } else { 0 });
        }
    };

    match xtask.task {
        Task::Ci => ci(),
        Task::Catalog { command } => catalog::run(command),
    }
}

/// Runs the same gates as CI, in order, stopping at the first failure.
fn ci() -> ExitCode {
    let steps: [(&str, Vec<&str>, Duration); 3] = [
        (
            "fmt check",
            vec!["fmt", "--all", "--", "--check"],
            FMT_LIMIT,
        ),
        (
            "clippy",
            vec![
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
            CLIPPY_LIMIT,
        ),
        ("tests", vec!["test", "--workspace"], TESTS_LIMIT),
    ];

    for (label, args, limit) in steps {
        println!("==> cargo {}", args.join(" "));
        let mut command = std::process::Command::new("cargo");
        command.args(&args);
        match limit::run_with_limit(command, limit) {
            limit::Outcome::Finished(status) if status.success() => {}
            limit::Outcome::Finished(_) => {
                eprintln!("xtask ci: {label} failed");
                return ExitCode::from(EXIT_FAILED);
            }
            limit::Outcome::TimedOut => {
                eprintln!(
                    "xtask ci: {label} timed out after {} s; a test is probably hung \
                     (the whole process tree was killed)",
                    limit.as_secs()
                );
                return ExitCode::from(EXIT_FAILED);
            }
            limit::Outcome::Failed(err) => {
                eprintln!("xtask ci: could not run cargo ({err})");
                return ExitCode::from(EXIT_FAILED);
            }
        }
    }
    println!("xtask ci: all gates passed");
    ExitCode::SUCCESS
}
