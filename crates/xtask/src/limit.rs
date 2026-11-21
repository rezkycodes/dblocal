//! Runs a child command under a wall-clock limit and kills its whole process tree.
//!
//! `cargo test` starts test binaries as grandchildren. A binary that deadlocks
//! (a GPUI headless shutdown did, for 54 minutes, on the development host)
//! would otherwise block `cargo xtask ci` forever with no output. On timeout the
//! command and everything it started are killed, and the caller reports a
//! failed gate with the limit that was hit.

use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

/// How often the child is checked while it runs.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How a limited command ended.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// The command exited by itself, successfully or not.
    Finished(ExitStatus),
    /// The limit passed; the command and its process tree were killed.
    TimedOut,
    /// The command could not be started or waited for.
    Failed(std::io::Error),
}

/// Runs `command`, killing its process tree after `limit`.
pub(crate) fn run_with_limit(mut command: Command, limit: Duration) -> Outcome {
    isolate(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return Outcome::Failed(err),
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Outcome::Finished(status),
            Ok(None) => {}
            Err(err) => {
                kill_tree(child.id());
                let _ = child.wait();
                return Outcome::Failed(err);
            }
        }
        if started.elapsed() >= limit {
            kill_tree(child.id());
            let _ = child.wait();
            return Outcome::TimedOut;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Puts the child in its own process group, so one signal reaches the tree.
#[cfg(unix)]
fn isolate(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn isolate(_command: &mut Command) {}

/// Kills the process group led by `pid`.
#[cfg(unix)]
fn kill_tree(pid: u32) {
    let _ = Command::new("kill")
        .args(["-s", "KILL", "--", &format!("-{pid}")])
        .status();
}

/// Kills `pid` and every process it started.
#[cfg(not(unix))]
fn kill_tree(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    fn alive(pid: &str) -> bool {
        Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    fn a_command_that_finishes_reports_its_status() {
        let outcome = run_with_limit(shell("exit 0"), Duration::from_secs(10));
        assert!(matches!(outcome, Outcome::Finished(status) if status.success()));
    }

    #[test]
    fn a_failing_command_is_finished_not_timed_out() {
        let outcome = run_with_limit(shell("exit 3"), Duration::from_secs(10));
        assert!(matches!(outcome, Outcome::Finished(status) if status.code() == Some(3)));
    }

    #[test]
    fn a_command_over_the_limit_is_killed_and_reported() {
        let started = Instant::now();
        let outcome = run_with_limit(shell("sleep 30"), Duration::from_millis(300));
        assert!(matches!(outcome, Outcome::TimedOut), "{outcome:?}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the limit must stop the wait, not the command"
        );
    }

    #[test]
    fn a_grandchild_of_a_timed_out_command_is_killed_too() {
        let dir = tempfile::tempdir().expect("temp dir");
        let pid_file = dir.path().join("grandchild.pid");
        // The grandchild sleeps far longer than any test deadline, so it can only
        // be dead because the timeout killed it.
        let script = format!("sleep 120 & echo $! > '{}'; wait", pid_file.display());
        let started = Instant::now();
        let outcome = run_with_limit(shell(&script), Duration::from_millis(800));
        assert!(matches!(outcome, Outcome::TimedOut), "{outcome:?}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the timeout must not wait for the grandchild to end by itself"
        );

        let pid = std::fs::read_to_string(&pid_file).expect("grandchild pid");
        let pid = pid.trim();
        // Give the kernel a moment to reap the killed process.
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(pid), "the grandchild {pid} survived the timeout");
    }

    #[test]
    fn a_command_that_cannot_start_is_a_failure() {
        let outcome = run_with_limit(
            Command::new("/nonexistent/dblocal-xtask-test"),
            Duration::from_secs(1),
        );
        assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
    }
}
