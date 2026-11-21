//! `cargo xtask e2e` — run the ignored real-engine integration tests.
//!
//! Each test binary runs under a wall-clock limit (so a hung e2e cannot block
//! forever) and its summary is echoed compactly. The tests themselves are
//! `#[ignore]`d and need network access plus cached engine binaries in
//! `$HOME/.cache/dblocal-dev/`.
//!
//! After every target the runner scans `$TMPDIR` for a `postgres` or
//! `valkey-server` process whose command line still names a `dblocal-e2e-*` temp
//! home. A test's own guard cleans up, so a survivor here means a guard did not
//! run and the run is reported failed — a leaked engine is exactly what the task
//! forbids.

use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

/// Wall-clock limit for one e2e target.
const E2E_LIMIT: Duration = Duration::from_secs(45 * 60);

/// One e2e test binary: its package and its target name.
struct Target {
    package: &'static str,
    test: &'static str,
}

/// The e2e test binaries to run.
const TARGETS: &[Target] = &[
    Target {
        package: "dblocal-core",
        test: "service_e2e",
    },
    Target {
        package: "dblocal-cli",
        test: "cli_e2e",
    },
];

/// Prefix every e2e temp home uses, so a leaked engine is identifiable.
const TEMP_HOME_PREFIX: &str = "dblocal-e2e-";

/// Runs each e2e target under a wall-clock limit.
pub fn run() -> ExitCode {
    let mut failed = false;
    for target in TARGETS {
        println!(
            "==> cargo test -p {} --test {} -- --ignored --test-threads=1 --nocapture",
            target.package, target.test
        );
        let mut command = Command::new("cargo");
        command.args([
            "test",
            "-p",
            target.package,
            "--test",
            target.test,
            "--",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ]);
        // The tests refuse to run without this switch; set it here so
        // `cargo xtask e2e` is the one command a developer needs.
        command.env("DBLOCAL_E2E", "1");
        match run_target(target.test, command) {
            Ok(true) => {}
            Ok(false) => failed = true,
            Err(failure) => {
                eprintln!("xtask e2e: {failure}");
                failed = true;
            }
        }
        if let Some(leaked) = leaked_engines() {
            eprintln!("xtask e2e: {leaked}");
            failed = true;
        }
    }
    if failed {
        eprintln!("xtask e2e: FAILED");
        ExitCode::from(crate::EXIT_FAILED)
    } else {
        println!("xtask e2e: every target passed");
        ExitCode::SUCCESS
    }
}

/// Runs one target, reporting pass/fail or why it could not be run.
fn run_target(target: &str, command: Command) -> Result<bool, String> {
    let started = Instant::now();
    match crate::limit::run_with_limit(command, E2E_LIMIT) {
        crate::limit::Outcome::Finished(status) if status.success() => {
            println!(
                "==> {target} passed in {:.1}s",
                started.elapsed().as_secs_f64()
            );
            Ok(true)
        }
        crate::limit::Outcome::Finished(status) => {
            eprintln!(
                "xtask e2e: {target} failed ({status}) after {:.1}s",
                started.elapsed().as_secs_f64()
            );
            Ok(false)
        }
        crate::limit::Outcome::TimedOut => Err(format!(
            "{target} timed out after {} s; its process tree was killed",
            E2E_LIMIT.as_secs()
        )),
        crate::limit::Outcome::Failed(err) => Err(format!("{target} could not run: {err}")),
    }
}

/// `Some(description)` when an engine process still names an e2e temp home.
fn leaked_engines() -> Option<String> {
    let tmp = std::env::temp_dir();
    let mut leaked = Vec::new();
    for entry in std::fs::read_dir(Path::new("/proc")).ok()? {
        let Ok(entry) = entry else { continue };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(bytes) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let command_line = bytes
            .split(|byte| *byte == 0)
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        if command_line.contains(&format!("{}/{}", tmp.display(), TEMP_HOME_PREFIX)) {
            leaked.push(format!("pid {pid}: {command_line}"));
        }
    }
    (!leaked.is_empty()).then(|| {
        format!(
            "an engine outlived its test; every e2e guard must kill one:\n{}",
            leaked.join("\n")
        )
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_target_list_is_not_empty() {
        assert!(!super::TARGETS.is_empty());
    }

    #[test]
    fn the_temp_home_prefix_matches_the_tests() {
        assert_eq!(super::TEMP_HOME_PREFIX, "dblocal-e2e-");
    }
}
