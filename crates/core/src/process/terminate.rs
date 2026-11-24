//! Stopping an engine process without ever touching a foreign one.
//!
//! The rule from invariant 7 and `F-INS-04` AC2 is simple and absolute: nothing
//! is signalled unless the identity still validates as
//! [`ProcessState::Match`]. `terminate` therefore rejects `Absent`, `Reused` and
//! `Unknown` before the first signal, and re-validates before escalating to a
//! forced kill, because the PID could have been reused while we waited.

use std::thread;
use std::time::{Duration, Instant};

use crate::DblError;
use crate::error::Result;
use crate::process::identity::{ProcessIdentity, ProcessState};
use crate::process::probe::{ProcessProbe, SysinfoProbe};

/// How often `terminate` re-checks the process while waiting for it to exit.
pub const TERMINATE_POLL: Duration = Duration::from_millis(50);

/// How long `terminate` waits for a SIGKILL to take effect before giving up.
pub const KILL_CONFIRM: Duration = Duration::from_secs(5);

/// What happened to the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TerminateOutcome {
    /// The process was already gone; nothing was signalled.
    NotRunning,
    /// The identity did not match, so nothing was signalled.
    ///
    /// The UI must treat this as "the pidfile was stale", not as a failure to
    /// stop something.
    Refused {
        /// What the process table actually showed.
        state: ProcessState,
    },
    /// SIGTERM was delivered and the process exited inside the grace period.
    Graceful,
    /// The process ignored SIGTERM and had to be killed.
    ///
    /// This is what the UI shows as "kill paksa" (`F-INS-03` AC3).
    Forced,
}

impl TerminateOutcome {
    /// Whether a signal was actually sent.
    pub const fn signalled(self) -> bool {
        matches!(self, Self::Graceful | Self::Forced)
    }

    /// Whether a forced kill was needed.
    pub const fn was_forced(self) -> bool {
        matches!(self, Self::Forced)
    }
}

/// Sends the two signals `terminate` needs.
///
/// Split out so the escalation logic can be tested without real processes and
/// without a platform `unsafe` block.
pub trait SignalSender {
    /// Requests a polite shutdown (SIGTERM; on Windows this is a termination).
    fn terminate(&self, pid: u32) -> Result<()>;
    /// Requests an unconditional kill.
    fn kill(&self, pid: u32) -> Result<()>;
}

/// Asks `identity` to exit, escalating to a forced kill after `grace`.
///
/// The process is validated before every signal. A `pid` that is alive but is
/// not the recorded process is reported as [`TerminateOutcome::Refused`] and
/// left untouched.
pub fn terminate(identity: &ProcessIdentity, grace: Duration) -> Result<TerminateOutcome> {
    terminate_with(identity, grace, &SysinfoProbe::new(), &platform_signals())
}

/// [`terminate`] with an explicit probe and signal sender.
pub fn terminate_with(
    identity: &ProcessIdentity,
    grace: Duration,
    probe: &dyn ProcessProbe,
    signals: &dyn SignalSender,
) -> Result<TerminateOutcome> {
    match probe.validate(identity) {
        ProcessState::Match => {}
        ProcessState::Absent => return Ok(TerminateOutcome::NotRunning),
        state => return Ok(TerminateOutcome::Refused { state }),
    }

    // Re-validate immediately before acting: the window between the check above
    // and the first signal is small, but a fast PID recycle is exactly the case
    // invariant 7 exists for. Bind the observation once so the state we report
    // is the state that made us refuse.
    let state = probe.validate(identity);
    if state != ProcessState::Match {
        return Ok(TerminateOutcome::Refused { state });
    }

    let delivered = match signals.terminate(identity.pid()) {
        Ok(()) => true,
        // The engine exited between the check above and `kill(2)`. That is not
        // an I/O failure: nothing is left to stop.
        Err(err) if is_process_gone(&err) => false,
        Err(err) => return Err(err),
    };
    match wait_for_exit(identity, probe, grace) {
        // Let the wait decide: a signal we could not deliver means the process
        // was already gone, which is `NotRunning`, not a graceful stop.
        Exit::Gone if delivered => return Ok(TerminateOutcome::Graceful),
        Exit::Gone => return Ok(TerminateOutcome::NotRunning),
        // We cannot prove the process is still ours, so escalating would risk
        // signalling an unreadable PID.
        Exit::Unverifiable => {
            return Ok(TerminateOutcome::Refused {
                state: ProcessState::Unknown,
            });
        }
        // The kernel said the PID does not exist, so the probe must not be
        // believed either: never act on a contradicted observation.
        Exit::StillOurs if !delivered => {
            return Ok(TerminateOutcome::Refused {
                state: ProcessState::Unknown,
            });
        }
        Exit::StillOurs => {}
    }

    // Still there. Kill it only if it is *still ours*.
    match probe.validate(identity) {
        ProcessState::Match => {}
        ProcessState::Absent | ProcessState::Reused => {
            return Ok(TerminateOutcome::Graceful);
        }
        ProcessState::Unknown => {
            return Ok(TerminateOutcome::Refused {
                state: ProcessState::Unknown,
            });
        }
    }
    let killed = match signals.kill(identity.pid()) {
        Ok(()) => true,
        Err(err) if is_process_gone(&err) => false,
        Err(err) => return Err(err),
    };
    match wait_for_exit(identity, probe, KILL_CONFIRM) {
        Exit::Gone if killed => Ok(TerminateOutcome::Forced),
        // It exited on its own in the instant between the final validation and
        // the forced kill, or the PID vanished before `kill(2)` delivered. No
        // forced kill reached it, so do not claim one in the UI.
        Exit::Gone => Ok(TerminateOutcome::Graceful),
        Exit::Unverifiable => Ok(TerminateOutcome::Refused {
            state: ProcessState::Unknown,
        }),
        Exit::StillOurs => Err(DblError::Engine {
            reason: format!("process {} survived SIGKILL", identity.pid()),
        }),
    }
}

/// Waits up to `limit` for `identity` to leave the process table on its own.
///
/// Used after an engine was *asked* to shut itself down (an SQL `SHUTDOWN`,
/// `mysqladmin shutdown`, `pg_ctl stop`): the request can return before the
/// process has finished flushing and exited, and on Windows the next step,
/// `terminate`, is an immediate `TerminateProcess`, so signalling right away
/// cuts a graceful shutdown short (a MySQL `stop` returned in 340 ms while
/// `mysqld.exe` ran another second; the instance directory was still locked).
///
/// Never signals anything. Returns `true` when the process is gone (absent or
/// replaced by another), `false` when it still matches after `limit` or when its
/// identity cannot be verified (the caller then escalates through `terminate`,
/// which has its own refusal rules).
pub fn await_exit(identity: &ProcessIdentity, limit: Duration) -> bool {
    await_exit_with(identity, &SysinfoProbe::new(), limit)
}

/// [`await_exit`] with an explicit probe.
pub fn await_exit_with(
    identity: &ProcessIdentity,
    probe: &dyn ProcessProbe,
    limit: Duration,
) -> bool {
    matches!(wait_for_exit(identity, probe, limit), Exit::Gone)
}

/// `true` when a failed signal means "no such process".
///
/// The engine can exit between our validation and `kill(2)`, which makes the
/// kernel answer `ESRCH`. That is a normal race, not an error: the caller turns
/// it into `NotRunning`/`Graceful` instead of surfacing `DblError::Io`.
fn is_process_gone(err: &DblError) -> bool {
    match err {
        DblError::Io { source } => source.raw_os_error().is_some_and(is_gone_errno),
        _ => false,
    }
}

#[cfg(unix)]
fn is_gone_errno(errno: i32) -> bool {
    errno == nix::errno::Errno::ESRCH as i32
}

/// Windows reports a vanished process as `ERROR_INVALID_PARAMETER` or
/// `ERROR_NOT_FOUND` depending on the API used; `sysinfo` returns `false`
/// instead of an errno, so this arm only matters for an injected sender.
#[cfg(windows)]
fn is_gone_errno(errno: i32) -> bool {
    const ERROR_INVALID_PARAMETER: i32 = 87;
    const ERROR_NOT_FOUND: i32 = 1168;
    errno == ERROR_INVALID_PARAMETER || errno == ERROR_NOT_FOUND
}

#[cfg(not(any(unix, windows)))]
fn is_gone_errno(_errno: i32) -> bool {
    false
}

/// What the process table showed while waiting for an exit.
enum Exit {
    /// The identity no longer matches because the process is gone or the PID
    /// was recycled: in either case the process we signalled is no longer
    /// running under our identity.
    Gone,
    /// The process still runs under our identity after the deadline.
    StillOurs,
    /// The process may still exist but we cannot prove its identity, so we must
    /// not escalate.
    Unverifiable,
}

/// Polls until the identity stops matching, or `limit` elapses.
fn wait_for_exit(identity: &ProcessIdentity, probe: &dyn ProcessProbe, limit: Duration) -> Exit {
    let deadline = Instant::now() + limit;
    loop {
        match probe.validate(identity) {
            ProcessState::Match => {}
            ProcessState::Absent | ProcessState::Reused => return Exit::Gone,
            ProcessState::Unknown => return Exit::Unverifiable,
        }
        if Instant::now() >= deadline {
            return Exit::StillOurs;
        }
        thread::sleep(TERMINATE_POLL);
    }
}

#[cfg(unix)]
fn platform_signals() -> impl SignalSender {
    signals::UnixSignals
}

#[cfg(windows)]
fn platform_signals() -> impl SignalSender {
    signals::WindowsSignals
}

#[cfg(unix)]
mod signals {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    use super::SignalSender;
    use crate::error::Result;

    /// POSIX signals: SIGTERM first, SIGKILL when the grace period expires.
    ///
    /// `ESRCH` means the process is already gone. Reporting it as an error would
    /// surface a normal exit race as `DblError::Io`, so it is treated as a
    /// successful "nothing left to signal" and the caller's wait decides the
    /// outcome.
    pub(super) struct UnixSignals;

    impl SignalSender for UnixSignals {
        fn terminate(&self, pid: u32) -> Result<()> {
            send(pid, Signal::SIGTERM)
        }

        fn kill(&self, pid: u32) -> Result<()> {
            send(pid, Signal::SIGKILL)
        }
    }

    fn send(pid: u32, signal: Signal) -> Result<()> {
        let raw = i32::try_from(pid).map_err(|_| crate::DblError::InvalidInput {
            field: "pid",
            reason: "does not fit in a pid_t".to_owned(),
        })?;
        match kill(Pid::from_raw(raw), signal) {
            Ok(()) => Ok(()),
            Err(nix::errno::Errno::ESRCH) => Ok(()),
            Err(err) => Err(crate::DblError::Io {
                source: std::io::Error::from_raw_os_error(err as i32),
            }),
        }
    }
}

#[cfg(windows)]
mod signals {
    use super::SignalSender;
    use crate::DblError;
    use crate::error::Result;

    /// Windows has no SIGTERM. The only portable, safe action is `sysinfo`'s
    /// terminating kill, which needs no raw Win32 call and therefore no
    /// `unsafe` block.
    pub(super) struct WindowsSignals;

    impl SignalSender for WindowsSignals {
        fn terminate(&self, pid: u32) -> Result<()> {
            self.kill(pid)
        }

        fn kill(&self, pid: u32) -> Result<()> {
            let mut system = sysinfo::System::new().map_err(|_| DblError::Unsupported {
                reason: "cannot read the process table on this host".to_owned(),
            })?;
            system.refresh_processes(
                sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
                true,
            );
            let Some(process) = system.process(sysinfo::Pid::from_u32(pid)) else {
                // Already gone: nothing to signal.
                return Ok(());
            };
            if process.kill() {
                Ok(())
            } else {
                Err(DblError::Engine {
                    reason: format!("process {pid} refused the termination request"),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::process::probe::ProcessObservation;
    use crate::process::probe::fake::FakeProbe;
    use std::path::PathBuf;
    use std::time::SystemTime;

    #[derive(Debug, Default)]
    struct RecordingSignals {
        term: Mutex<Vec<u32>>,
        kill: Mutex<Vec<u32>>,
    }

    impl RecordingSignals {
        fn terminated(&self) -> Vec<u32> {
            self.term.lock().expect("term lock").clone()
        }
        fn killed(&self) -> Vec<u32> {
            self.kill.lock().expect("kill lock").clone()
        }
    }

    impl SignalSender for RecordingSignals {
        fn terminate(&self, pid: u32) -> Result<()> {
            self.term.lock().expect("term lock").push(pid);
            Ok(())
        }

        fn kill(&self, pid: u32) -> Result<()> {
            self.kill.lock().expect("kill lock").push(pid);
            Ok(())
        }
    }

    fn identity(pid: u32) -> ProcessIdentity {
        ProcessIdentity::new(pid, "/opt/engine/bin/postgres", SystemTime::UNIX_EPOCH)
            .expect("identity")
    }

    fn process(_pid: u32) -> ProcessObservation {
        ProcessObservation::Found {
            exe: Some(PathBuf::from("/opt/engine/bin/postgres")),
            started_at: Some(SystemTime::UNIX_EPOCH),
        }
    }

    #[test]
    fn await_exit_returns_true_once_the_process_leaves() {
        let probe = FakeProbe::new();
        probe.set(20, process(20));
        let gone = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(120));
                probe.remove(20);
            });
            await_exit_with(&identity(20), &probe, Duration::from_secs(5))
        });
        assert!(gone, "the wait must end as soon as the process is absent");
    }

    #[test]
    fn await_exit_gives_up_after_the_limit_and_signals_nothing() {
        let probe = FakeProbe::new();
        probe.set(21, process(21));
        let started = Instant::now();
        assert!(!await_exit_with(
            &identity(21),
            &probe,
            Duration::from_millis(150)
        ));
        assert!(started.elapsed() >= Duration::from_millis(150));
    }

    #[test]
    fn await_exit_does_not_claim_exit_for_an_unverifiable_process() {
        let probe = FakeProbe::new();
        probe.set(
            22,
            ProcessObservation::Found {
                exe: None,
                started_at: None,
            },
        );
        assert!(!await_exit_with(
            &identity(22),
            &probe,
            Duration::from_millis(100)
        ));
    }

    #[test]
    fn await_exit_on_an_already_absent_process_is_immediate() {
        let probe = FakeProbe::new();
        let started = Instant::now();
        assert!(await_exit_with(
            &identity(23),
            &probe,
            Duration::from_secs(5)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn absent_process_is_not_signalled() {
        let probe = FakeProbe::new();
        let signals = RecordingSignals::default();
        let outcome = terminate_with(&identity(10), Duration::from_millis(10), &probe, &signals)
            .expect("terminate");
        assert_eq!(outcome, TerminateOutcome::NotRunning);
        assert!(signals.terminated().is_empty());
        assert!(signals.killed().is_empty());
    }

    #[test]
    fn a_live_foreign_pid_is_never_signalled() {
        let probe = FakeProbe::new();
        // Same PID, different executable: a reused PID.
        probe.set(
            11,
            ProcessObservation::Found {
                exe: Some(PathBuf::from("/usr/bin/sleep")),
                started_at: Some(SystemTime::UNIX_EPOCH),
            },
        );
        let signals = RecordingSignals::default();
        let outcome = terminate_with(&identity(11), Duration::from_millis(10), &probe, &signals)
            .expect("terminate");
        assert_eq!(
            outcome,
            TerminateOutcome::Refused {
                state: ProcessState::Reused
            }
        );
        assert!(signals.terminated().is_empty(), "no SIGTERM must be sent");
        assert!(signals.killed().is_empty(), "no SIGKILL must be sent");
    }

    #[test]
    fn unknown_identity_is_refused() {
        let probe = FakeProbe::new();
        probe.set(
            12,
            ProcessObservation::Found {
                exe: None,
                started_at: None,
            },
        );
        let signals = RecordingSignals::default();
        let outcome = terminate_with(&identity(12), Duration::from_millis(10), &probe, &signals)
            .expect("terminate");
        assert_eq!(
            outcome,
            TerminateOutcome::Refused {
                state: ProcessState::Unknown
            }
        );
        assert!(signals.terminated().is_empty());
    }

    #[test]
    fn graceful_exit_after_sigterm() {
        let probe = FakeProbe::new();
        probe.set(13, process(13));
        let signals = RecordingSignals::default();
        // The process disappears as soon as SIGTERM is sent.
        struct Disappearing<'a> {
            probe: &'a FakeProbe,
        }
        impl SignalSender for Disappearing<'_> {
            fn terminate(&self, pid: u32) -> Result<()> {
                self.probe.remove(pid);
                Ok(())
            }
            fn kill(&self, _pid: u32) -> Result<()> {
                Ok(())
            }
        }
        let outcome = terminate_with(
            &identity(13),
            Duration::from_millis(50),
            &probe,
            &Disappearing { probe: &probe },
        )
        .expect("terminate");
        assert_eq!(outcome, TerminateOutcome::Graceful);
        assert!(outcome.signalled());
        assert!(!outcome.was_forced());
        assert!(signals.terminated().is_empty(), "unused sender");
    }

    #[test]
    fn escalates_to_forced_kill_and_reports_it() {
        let probe = FakeProbe::new();
        probe.set(14, process(14));
        let signals = RecordingSignals::default();
        // Kill removes the process from the table, proving escalation worked.
        struct KillsForReal<'a> {
            probe: &'a FakeProbe,
            signals: &'a RecordingSignals,
        }
        impl SignalSender for KillsForReal<'_> {
            fn terminate(&self, pid: u32) -> Result<()> {
                self.signals.terminate(pid)
            }
            fn kill(&self, pid: u32) -> Result<()> {
                let result = self.signals.kill(pid);
                self.probe.remove(pid);
                result
            }
        }
        let outcome = terminate_with(
            &identity(14),
            Duration::from_millis(30),
            &probe,
            &KillsForReal {
                probe: &probe,
                signals: &signals,
            },
        )
        .expect("terminate");
        assert_eq!(outcome, TerminateOutcome::Forced);
        assert!(outcome.was_forced());
        assert_eq!(signals.terminated(), vec![14]);
        assert_eq!(signals.killed(), vec![14]);
    }

    #[test]
    fn does_not_escalate_when_the_pid_was_reused_during_the_grace_period() {
        let probe = FakeProbe::new();
        probe.set(15, process(15));
        struct Reusing<'a> {
            probe: &'a FakeProbe,
        }
        impl SignalSender for Reusing<'_> {
            fn terminate(&self, pid: u32) -> Result<()> {
                // PID now belongs to somebody else.
                self.probe.set(
                    pid,
                    ProcessObservation::Found {
                        exe: Some(PathBuf::from("/usr/bin/ssh")),
                        started_at: Some(SystemTime::UNIX_EPOCH),
                    },
                );
                Ok(())
            }
            fn kill(&self, _pid: u32) -> Result<()> {
                panic!("must never kill a reused PID");
            }
        }
        let outcome = terminate_with(
            &identity(15),
            Duration::from_millis(30),
            &probe,
            &Reusing { probe: &probe },
        )
        .expect("terminate");
        // Our process is gone, so this is a graceful result; the essential
        // property is that no SIGKILL reached the new owner of the PID (the
        // `panic!` in `Reusing::kill` proves that).
        assert_eq!(outcome, TerminateOutcome::Graceful);
        assert!(!outcome.was_forced());
    }

    #[test]
    fn does_not_escalate_when_the_identity_becomes_unreadable() {
        let probe = FakeProbe::new();
        probe.set(16, process(16));
        struct Unreadable<'a> {
            probe: &'a FakeProbe,
        }
        impl SignalSender for Unreadable<'_> {
            fn terminate(&self, pid: u32) -> Result<()> {
                self.probe.set(
                    pid,
                    ProcessObservation::Found {
                        exe: None,
                        started_at: None,
                    },
                );
                Ok(())
            }
            fn kill(&self, _pid: u32) -> Result<()> {
                panic!("must never kill an unreadable identity");
            }
        }
        let outcome = terminate_with(
            &identity(16),
            Duration::from_millis(30),
            &probe,
            &Unreadable { probe: &probe },
        )
        .expect("terminate");
        assert_eq!(
            outcome,
            TerminateOutcome::Refused {
                state: ProcessState::Unknown
            }
        );
    }

    /// The error a signal call returns for a PID that is no longer there.
    ///
    /// POSIX answers `ESRCH`; Windows reports `ERROR_NOT_FOUND` for the same
    /// race, and `is_gone_errno` maps both. Using the host's own errno keeps the
    /// "already gone is not an I/O error" tests meaningful on both.
    fn vanished() -> DblError {
        #[cfg(unix)]
        let errno = nix::errno::Errno::ESRCH as i32;
        #[cfg(windows)]
        let errno = 1168; // ERROR_NOT_FOUND
        #[cfg(not(any(unix, windows)))]
        let errno = 0;
        DblError::Io {
            source: std::io::Error::from_raw_os_error(errno),
        }
    }

    #[test]
    fn a_process_that_exits_before_the_signal_is_not_running() {
        // The engine exits between our validation and `kill(2)`: the kernel
        // answers ESRCH. That is "already gone", not `DblError::Io`.
        let probe = FakeProbe::new();
        probe.set(17, process(17));
        struct Vanished<'a> {
            probe: &'a FakeProbe,
        }
        impl SignalSender for Vanished<'_> {
            fn terminate(&self, pid: u32) -> Result<()> {
                self.probe.remove(pid);
                Err(vanished())
            }
            fn kill(&self, _pid: u32) -> Result<()> {
                panic!("must not escalate after the process vanished");
            }
        }
        let outcome = terminate_with(
            &identity(17),
            Duration::from_millis(50),
            &probe,
            &Vanished { probe: &probe },
        )
        .expect("an ESRCH race must not surface as an I/O error");
        assert_eq!(outcome, TerminateOutcome::NotRunning);
        assert!(!outcome.signalled());
        assert!(!outcome.was_forced());
    }

    #[test]
    fn a_kill_that_races_an_exit_is_graceful_not_forced() {
        // SIGTERM is ignored, the process exits by itself during the grace
        // period, and the forced kill then hits a dead PID. The UI must not say
        // "kill paksa" for a signal that never landed.
        let probe = FakeProbe::new();
        probe.set(18, process(18));
        let signals = RecordingSignals::default();
        struct ExitsLate<'a> {
            probe: &'a FakeProbe,
            signals: &'a RecordingSignals,
        }
        impl SignalSender for ExitsLate<'_> {
            fn terminate(&self, pid: u32) -> Result<()> {
                self.signals.terminate(pid)
            }
            fn kill(&self, pid: u32) -> Result<()> {
                // Record the attempt first; `DblError` has no `PartialEq`, so
                // assert on `is_ok` instead of comparing results.
                assert!(self.signals.kill(pid).is_ok(), "kill attempt recorded");
                // The process exited just before the signal arrived, so the
                // kernel answers ESRCH.
                self.probe.remove(pid);
                Err(vanished())
            }
        }
        let outcome = terminate_with(
            &identity(18),
            Duration::from_millis(20),
            &probe,
            &ExitsLate {
                probe: &probe,
                signals: &signals,
            },
        )
        .expect("terminate");
        assert_eq!(outcome, TerminateOutcome::Graceful);
        assert!(!outcome.was_forced(), "no forced kill actually happened");
        assert_eq!(signals.killed(), vec![18], "the kill was attempted");
    }

    #[test]
    fn a_refusal_reports_the_state_that_caused_it() {
        // The second validation must bind one observation: with a probe that
        // changes its answer per call, the reported state has to be the one the
        // decision was made on, not a later, different one.
        struct ScriptedProbe {
            states: Mutex<std::collections::VecDeque<ProcessState>>,
            calls: Mutex<usize>,
        }
        impl ProcessProbe for ScriptedProbe {
            fn observe(&self, _pid: u32) -> ProcessObservation {
                unreachable!("validate is overridden")
            }
            fn validate(&self, _identity: &ProcessIdentity) -> ProcessState {
                let mut calls = self.calls.lock().expect("calls lock");
                *calls += 1;
                self.states
                    .lock()
                    .expect("states lock")
                    .pop_front()
                    .expect("scripted state")
            }
        }
        let probe = ScriptedProbe {
            states: Mutex::new(
                [
                    ProcessState::Match,
                    ProcessState::Reused,
                    ProcessState::Unknown,
                ]
                .into_iter()
                .collect(),
            ),
            calls: Mutex::new(0),
        };
        let signals = RecordingSignals::default();
        let outcome = terminate_with(&identity(19), Duration::from_millis(20), &probe, &signals)
            .expect("terminate");
        assert_eq!(
            outcome,
            TerminateOutcome::Refused {
                state: ProcessState::Reused
            },
            "the reported state must be the bound decision, not a later probe"
        );
        assert_eq!(*probe.calls.lock().expect("calls lock"), 2);
        assert!(signals.terminated().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_unix_sender_treats_esrch_as_delivered() {
        // `kill(2)` against a PID above `pid_max` cannot reach a process, so this
        // only exercises the errno mapping. No process is signalled, and the
        // "already gone" answer must not surface as an I/O error.
        super::signals::UnixSignals
            .kill(999_999_999)
            .expect("ESRCH from the sender must be treated as nothing to signal");
    }

    #[test]
    fn other_signal_errors_are_still_errors() {
        let denied = DblError::Io {
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        assert!(!is_process_gone(&denied));
        assert!(!is_process_gone(&DblError::Engine {
            reason: "boom".to_owned()
        }));
    }
}
