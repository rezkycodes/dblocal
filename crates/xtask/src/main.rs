//! Developer tasks, run through `cargo xtask`.

mod catalog;
mod e2e;
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
    /// Runs ignored real-engine end-to-end tests (network + cached binaries).
    ///
    /// Each test binary runs under a wall-clock limit so a hung e2e cannot block
    /// the run forever.
    E2e,
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
        Task::E2e => e2e::run(),
    }
}

/// The CI gates, in order, with their wall-clock limits.
///
/// Extracted so a test can assert the exact cargo arguments without running
/// them: the `tests` step must carry `--no-fail-fast` so one Windows CI run
/// reports every failing crate, not just the first.
fn steps() -> [(&'static str, Vec<&'static str>, Duration); 3] {
    [
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
        (
            "tests",
            // `--no-fail-fast` keeps the whole workspace run going past the
            // first failing test binary. Cargo otherwise stops at the first
            // failing crate, so a Windows run that fails in `dblocal-cli` never
            // reaches `dblocal-core` or `dblocal-gui`; one expensive CI run then
            // hides every other failure behind the first.
            vec!["test", "--workspace", "--no-fail-fast"],
            TESTS_LIMIT,
        ),
    ]
}

/// Runs the same gates as CI, in order, stopping at the first failure.
fn ci() -> ExitCode {
    let steps = steps();

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests gate must keep `--no-fail-fast`: stopping at the first failing
    /// crate hid the `dblocal-core` and `dblocal-gui` failures behind a
    /// `dblocal-cli` failure on Windows, wasting a full CI iteration each time.
    #[test]
    fn the_tests_gate_does_not_fail_fast() {
        let (_, tests_args, _) = steps()
            .into_iter()
            .find(|(label, _, _)| *label == "tests")
            .expect("a tests gate");
        assert_eq!(tests_args.first(), Some(&"test"));
        assert!(tests_args.contains(&"--workspace"));
        assert!(
            tests_args.contains(&"--no-fail-fast"),
            "the tests gate must not stop at the first failing crate: {tests_args:?}"
        );
    }
}
