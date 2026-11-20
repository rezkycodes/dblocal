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
    // invariant 7 exists for.
    if probe.validate(identity) != ProcessState::Match {
        return Ok(TerminateOutcome::Refused {
            state: probe.validate(identity),
        });
    }

    signals.terminate(identity.pid())?;
    match wait_for_exit(identity, probe, grace) {
        Exit::Gone => return Ok(TerminateOutcome::Graceful),
        // We cannot prove the process is still ours, so escalating would risk
        // signalling an unreadable PID.
        Exit::Unverifiable => {
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
    signals.kill(identity.pid())?;
    match wait_for_exit(identity, probe, KILL_CONFIRM) {
        Exit::Gone => Ok(TerminateOutcome::Forced),
        Exit::Unverifiable => Ok(TerminateOutcome::Refused {
            state: ProcessState::Unknown,
        }),
        Exit::StillOurs => Err(DblError::Engine {
            reason: format!("process {} survived SIGKILL", identity.pid()),
        }),
    }
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
        kill(Pid::from_raw(raw), signal).map_err(|err| crate::DblError::Io {
            source: std::io::Error::from_raw_os_error(err as i32),
        })
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
}
