//! Spawning engine processes that outlive the application.
//!
//! `F-INS-03` requires engines to keep running after DBLocal quits, and
//! `F-INS-04` requires us to recognise them again later. `spawn_detached` is
//! therefore the only place a child process is born: it wires stdio to a log
//! file, puts the child in its own process group, and returns a
//! [`ProcessIdentity`] that [`super::terminate`] and reconciliation can prove.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use crate::DblError;
use crate::error::Result;
use crate::process::identity::ProcessIdentity;
use crate::process::probe::{ProcessObservation, ProcessProbe, SysinfoProbe};

/// Spawns `command` detached and appends its output to `log_path`.
///
/// `command` is consumed so the caller cannot accidentally reuse a builder that
/// already has our stdio and process-group settings applied.
///
/// - `stdin` is null.
/// - `stdout` and `stderr` are appended to `log_path` (created with mode
///   `0600` on Unix; parent directories are created).
/// - On Unix the child is placed in a new process group (`process_group(0)`),
///   so it does not die with the parent's controlling terminal.
/// - On Windows the child is created with `DETACHED_PROCESS`,
///   `CREATE_NEW_PROCESS_GROUP` and `CREATE_NO_WINDOW`.
///
/// The returned identity is read back from the live process table so its start
/// time is the kernel's view, the same one reconciliation will compare against.
pub fn spawn_detached(mut command: Command, log_path: &Path) -> Result<ProcessIdentity> {
    prepare_command(&mut command, log_path)?;
    let program = command.get_program().to_os_string();
    let child = command.spawn().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            DblError::Engine {
                reason: format!("engine binary not found: {}", Path::new(&program).display()),
            }
        } else {
            DblError::Io { source: err }
        }
    })?;
    let pid = child.id();
    // The process is detached: the caller must not wait for it. A tiny reaper
    // thread still calls `wait`, because an unwaited child becomes a zombie that
    // stays in the process table until DBLocal exits. The thread is harmless:
    // it only exists while the engine runs, and it never signals anything.
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });

    let probe = SysinfoProbe::new();
    observe_spawned(&probe, pid, &program)
}

fn prepare_command(command: &mut Command, log_path: &Path) -> Result<()> {
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let stdout = open_log(log_path)?;
    let stderr = stdout.try_clone()?;
    command.stdin(Stdio::null());
    command.stdout(Stdio::from(stdout));
    command.stderr(Stdio::from(stderr));
    configure_detach(command);
    Ok(())
}

fn open_log(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

#[cfg(unix)]
fn configure_detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn configure_detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
    const FLAGS: u32 = 0x0000_0008 | 0x0000_0200 | 0x0800_0000;
    command.creation_flags(FLAGS);
}

#[cfg(not(any(unix, windows)))]
fn configure_detach(_command: &mut Command) {}

fn observe_spawned(
    probe: &dyn ProcessProbe,
    pid: u32,
    program: &std::ffi::OsStr,
) -> Result<ProcessIdentity> {
    // The child may briefly report the parent's executable between fork and
    // exec. Wait until we have seen the same post-exec executable twice in a
    // row (not our own binary), which means exec finished and the identity is
    // stable. If the process exits early, return the last observation we saw so
    // a short-lived helper can still be accounted for.
    const ATTEMPTS: u32 = 200;
    let own_exe = std::env::current_exe().ok();
    let mut last: Option<(Option<PathBuf>, Option<SystemTime>)> = None;
    let mut previous: Option<(Option<PathBuf>, Option<SystemTime>)> = None;

    for attempt in 0..ATTEMPTS {
        match probe.observe(pid) {
            ProcessObservation::Found { exe, started_at } => {
                let pre_exec = match (&exe, &own_exe) {
                    (Some(observed), Some(own)) => observed == own,
                    _ => false,
                };
                let settled = !pre_exec
                    && previous.as_ref().is_some_and(|(prev_exe, prev_start)| {
                        *prev_exe == exe && *prev_start == started_at
                    });
                if settled {
                    let exe = exe.unwrap_or_else(|| PathBuf::from(program));
                    let started_at = started_at.unwrap_or_else(SystemTime::now);
                    return ProcessIdentity::new(pid, exe, started_at);
                }
                previous = Some((exe.clone(), started_at));
                last = Some((exe, started_at));
                if attempt + 1 < ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            ProcessObservation::Missing => {
                if let Some((exe, started_at)) = last {
                    let exe = exe.unwrap_or_else(|| PathBuf::from(program));
                    let started_at = started_at.unwrap_or_else(SystemTime::now);
                    return ProcessIdentity::new(pid, exe, started_at);
                }
                if attempt + 1 < ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                } else {
                    return Err(DblError::Engine {
                        reason: format!("engine process {pid} exited immediately after spawn"),
                    });
                }
            }
        }
    }

    if let Some((exe, started_at)) = last {
        let exe = exe.unwrap_or_else(|| PathBuf::from(program));
        let started_at = started_at.unwrap_or_else(SystemTime::now);
        return ProcessIdentity::new(pid, exe, started_at);
    }
    Err(DblError::Engine {
        reason: format!("engine process {pid} could not be observed after spawn"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::identity::ProcessState;
    use crate::process::probe::SysinfoProbe;
    use crate::process::terminate::{TerminateOutcome, terminate};
    use std::time::Duration;
    use tempfile::tempdir;

    /// Waits until `log` holds both marker lines, or panics at the deadline.
    ///
    /// A child needs time to be scheduled and for the kernel to drain its stdio
    /// into the file, and under load that can be far longer than any fixed sleep
    /// a test would pick: a 250 ms sleep lost to four burners made this test fail
    /// with `log was ""` while nothing was wrong with the spawn. Polling to a
    /// generous deadline keeps the assertions exactly as strong (both markers
    /// must appear) while removing the timing guess, and the failure message
    /// says which marker was missing and what the file held at the deadline.
    #[cfg(unix)]
    fn wait_for_log_content(log: &std::path::Path) -> String {
        const DEADLINE: Duration = Duration::from_secs(10);
        const STEP: Duration = Duration::from_millis(10);
        let deadline = std::time::Instant::now() + DEADLINE;
        loop {
            let content = std::fs::read_to_string(log).unwrap_or_default();
            if content.contains("from-child") && content.contains("to-stderr") {
                return content;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the child's output never reached {}; missing {} after {DEADLINE:?}, the log held {content:?}",
                log.display(),
                missing_marker(&content),
            );
            std::thread::sleep(STEP);
        }
    }

    /// Names the marker [`wait_for_log_content`] has not seen yet.
    #[cfg(unix)]
    fn missing_marker(content: &str) -> &'static str {
        if !content.contains("from-child") {
            "the stdout marker `from-child`"
        } else {
            "the stderr marker `to-stderr`"
        }
    }

    fn sleep_cmd(secs: u32) -> Command {
        #[cfg(unix)]
        {
            let mut cmd = Command::new("/bin/sleep");
            cmd.arg(secs.to_string());
            cmd
        }
        #[cfg(not(unix))]
        {
            let mut cmd = Command::new("cmd");
            cmd.args(["/C", "timeout", &secs.to_string()]);
            cmd
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_child_survives_dropping_every_handle() {
        // `F-INS-03` AC2 / D-10: the engine must outlive the app. Spawn, drop
        // everything `spawn_detached` owned, wait, and prove it is still ours.
        let dir = tempdir().expect("tempdir");
        let log = dir.path().join("logs").join("sleep.log");
        let identity = spawn_detached(sleep_cmd(30), &log).expect("spawn detached");
        assert!(identity.pid() > 0);

        // Nothing from the spawn is retained; only the identity remains.
        std::thread::sleep(Duration::from_millis(300));
        let probe = SysinfoProbe::new();
        assert_eq!(
            probe.validate(&identity),
            ProcessState::Match,
            "detached child must still be our process after the parent dropped it"
        );

        let outcome = terminate(&identity, Duration::from_secs(3)).expect("terminate");
        assert!(
            outcome.signalled(),
            "detached child must still be stoppable"
        );
        assert_eq!(
            probe.validate(&identity),
            ProcessState::Absent,
            "no process may be left behind"
        );
    }

    #[cfg(unix)]
    #[test]
    fn stdout_and_stderr_are_appended_to_the_log() {
        let dir = tempdir().expect("tempdir");
        let log = dir.path().join("logs").join("engine.log");
        // The shell keeps running after `sleep`, so it does not exec and its
        // executable path stays stable for the identity check below.
        let mut cmd = Command::new("/bin/sh");
        cmd.args([
            "-c",
            "echo from-child; echo to-stderr >&2; sleep 5; echo done",
        ]);
        let identity = spawn_detached(cmd, &log).expect("spawn");
        // Poll rather than sleep: a fixed 250 ms was lost to load and failed with
        // `log was ""` (see `wait_for_log_content`).
        let content = wait_for_log_content(&log);
        assert!(content.contains("from-child"), "log was {content:?}");
        assert!(
            content.contains("to-stderr"),
            "stderr must be captured too: {content:?}"
        );

        let outcome = terminate(&identity, Duration::from_secs(3)).expect("terminate");
        assert!(outcome.signalled() || matches!(outcome, TerminateOutcome::NotRunning));
    }

    #[cfg(unix)]
    #[test]
    fn the_log_file_is_created_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().expect("tempdir");
        let log = dir.path().join("nested").join("engine.log");
        let identity = spawn_detached(sleep_cmd(30), &log).expect("spawn");
        let mode = std::fs::metadata(&log).expect("log").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "log mode was {mode:o}");
        let _ = terminate(&identity, Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn appending_keeps_earlier_content() {
        let dir = tempdir().expect("tempdir");
        let log = dir.path().join("engine.log");
        std::fs::write(&log, "previous run\n").expect("seed log");
        let identity = spawn_detached(sleep_cmd(30), &log).expect("spawn");
        let _ = terminate(&identity, Duration::from_secs(3));
        let content = std::fs::read_to_string(&log).expect("log readable");
        assert!(content.starts_with("previous run"), "log was {content:?}");
    }

    #[test]
    fn observe_spawned_retries_and_fails_for_a_vanishing_pid() {
        // A child that exits before we can observe it must not yield an identity
        // that later validates as `Match` against something else.
        let probe = crate::process::probe::fake::FakeProbe::new();
        let err = observe_spawned(&probe, 424_242, std::ffi::OsStr::new("/bin/sleep"))
            .expect_err("a vanished child must be an error");
        assert!(matches!(err, DblError::Engine { .. }), "got {err:?}");
    }

    #[test]
    fn observe_spawned_falls_back_when_facts_are_missing() {
        use crate::process::probe::ProcessObservation;
        use crate::process::probe::fake::FakeProbe;
        let probe = FakeProbe::new();
        probe.set(
            777,
            ProcessObservation::Found {
                exe: None,
                started_at: None,
            },
        );
        let identity = observe_spawned(&probe, 777, std::ffi::OsStr::new("/bin/sleep"))
            .expect("identity from partial facts");
        assert_eq!(identity.pid(), 777);
        assert_eq!(identity.exe(), Path::new("/bin/sleep"));
    }

    #[test]
    fn a_missing_binary_is_reported_as_an_engine_error() {
        let dir = tempdir().expect("tempdir");
        let log = dir.path().join("engine.log");
        let cmd = Command::new("/this/binary/does/not/exist-dblocal-test");
        let err = spawn_detached(cmd, &log).expect_err("must fail");
        assert!(
            matches!(err, DblError::Engine { .. }) || matches!(err, DblError::Io { .. }),
            "unexpected error {err:?}"
        );
    }
}
