//! Developer tasks, run through `cargo xtask`.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Exit code for a failed task.
const EXIT_FAILED: u8 = 1;

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
    /// Rebuilds the bundled engine catalog.
    Catalog,
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
        Task::Catalog => {
            println!("not implemented");
            ExitCode::from(EXIT_FAILED)
        }
    }
}

/// Runs the same gates as CI, in order, stopping at the first failure.
fn ci() -> ExitCode {
    let steps: [(&str, Vec<&str>); 3] = [
        ("fmt check", vec!["fmt", "--all", "--", "--check"]),
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
        ),
        ("tests", vec!["test", "--workspace"]),
    ];

    for (label, args) in steps {
        println!("==> cargo {}", args.join(" "));
        let status = std::process::Command::new("cargo").args(&args).status();
        match status {
            Ok(status) if status.success() => {}
            Ok(_) => {
                eprintln!("xtask ci: {label} failed");
                return ExitCode::from(EXIT_FAILED);
            }
            Err(err) => {
                eprintln!("xtask ci: could not run cargo ({err})");
                return ExitCode::from(EXIT_FAILED);
            }
        }
    }
    println!("xtask ci: all gates passed");
    ExitCode::from(0)
}
