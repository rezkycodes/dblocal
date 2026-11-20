//! Reading the live process table behind a trait, so reconciliation is a pure,
//! fully unit-tested function.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::process::identity::{ProcessIdentity, ProcessState, classify};

/// What the operating system says about one PID right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessObservation {
    /// No process exists with that PID.
    Missing,
    /// Something exists, plus whatever facts could be read about it.
    ///
    /// Both fields are `None` when the process exists but cannot be inspected,
    /// for example because it belongs to another user. That is not the same as
    /// "not our process" and becomes [`ProcessState::Unknown`], which is also
    /// never signalled.
    Found {
        /// Resolved executable path, tolerating Linux's ` (deleted)` suffix.
        exe: Option<PathBuf>,
        /// Wall clock start time derived from the kernel's boot-relative value.
        started_at: Option<SystemTime>,
    },
}

/// A source of process facts.
///
/// Production code uses [`SysinfoProbe`]; tests use
/// [`crate::process::probe::FakeProbe`] or their own table.
pub trait ProcessProbe {
    /// Observes one PID. Implementations must not signal anything.
    fn observe(&self, pid: u32) -> ProcessObservation;

    /// Validates a recorded identity against the process table.
    ///
    /// The default implementation is the only place identity checks are
    /// assembled, so a fake probe cannot accidentally weaken them.
    fn validate(&self, identity: &ProcessIdentity) -> ProcessState {
        classify(identity, &self.observe(identity.pid()))
    }
}

/// `sysinfo`-backed probe used in production.
///
/// `System::new()` is cheap; refreshing only the PIDs we ask about keeps a
/// 20-instance reconciliation well inside the 2 s budget of `F-INS-04` AC3.
#[derive(Debug)]
pub struct SysinfoProbe {
    system: std::sync::Mutex<Option<sysinfo::System>>,
}

impl SysinfoProbe {
    /// Builds a probe with an empty process cache.
    pub fn new() -> Self {
        Self {
            system: std::sync::Mutex::new(None),
        }
    }
}

impl Default for SysinfoProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessProbe for SysinfoProbe {
    fn observe(&self, pid: u32) -> ProcessObservation {
        let mut guard = match self.system.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_none() {
            *guard = sysinfo::System::new().ok();
        }
        let Some(system) = guard.as_mut() else {
            // No handle available: refuse to claim the process is ours.
            return ProcessObservation::Found {
                exe: None,
                started_at: None,
            };
        };

        let pid = sysinfo::Pid::from_u32(pid);
        let kind = sysinfo::ProcessRefreshKind::nothing()
            .with_exe(sysinfo::UpdateKind::Always)
            .with_cmd(sysinfo::UpdateKind::Never)
            .with_cwd(sysinfo::UpdateKind::Never);
        system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[pid]), true, kind);
        let Some(process) = system.process(pid) else {
            return ProcessObservation::Missing;
        };
        if matches!(
            process.status(),
            sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
        ) {
            return ProcessObservation::Missing;
        }
        let started_at = {
            let secs = process.start_time();
            (secs > 0).then(|| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        };
        ProcessObservation::Found {
            exe: process.exe().map(std::path::Path::to_path_buf),
            started_at,
        }
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::{ProcessObservation, ProcessProbe};

    /// In-memory process table for unit tests.
    ///
    /// A PID that is not in the table is [`ProcessObservation::Missing`],
    /// which mirrors the real world: the kernel forgets dead PIDs.
    #[derive(Debug, Default)]
    pub struct FakeProbe {
        table: Mutex<HashMap<u32, ProcessObservation>>,
    }

    impl FakeProbe {
        /// Empty table: every PID is absent.
        pub fn new() -> Self {
            Self::default()
        }

        /// Adds or replaces one PID.
        pub fn set(&self, pid: u32, observation: ProcessObservation) {
            let mut table = self.table.lock().expect("fake probe lock");
            table.insert(pid, observation);
        }

        /// Removes one PID.
        pub fn remove(&self, pid: u32) {
            let mut table = self.table.lock().expect("fake probe lock");
            table.remove(&pid);
        }
    }

    impl ProcessProbe for FakeProbe {
        fn observe(&self, pid: u32) -> ProcessObservation {
            let table = self.table.lock().expect("fake probe lock");
            table
                .get(&pid)
                .cloned()
                .unwrap_or(ProcessObservation::Missing)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::probe::fake::FakeProbe;
    use std::time::Duration;

    #[test]
    fn fake_probe_reports_missing_pids() {
        let probe = FakeProbe::new();
        assert_eq!(probe.observe(1), ProcessObservation::Missing);
        probe.set(9, ProcessObservation::Missing);
        assert_eq!(probe.observe(9), ProcessObservation::Missing);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sysinfo_observes_a_real_process() {
        let probe = SysinfoProbe::new();
        let own = std::process::id();
        let observed = probe.observe(own);
        match observed {
            ProcessObservation::Found { exe, started_at } => {
                assert!(exe.is_some(), "own executable should resolve");
                assert!(started_at.is_some(), "own start time should resolve");
            }
            ProcessObservation::Missing => panic!("own process must exist"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sysinfo_sees_an_absent_pid() {
        let probe = SysinfoProbe::new();
        // A PID far above any plausible limit. Linux caps pid_max well below this.
        assert_eq!(probe.observe(999_999_999), ProcessObservation::Missing);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn validate_matches_our_own_identity() {
        let probe = SysinfoProbe::new();
        let own = std::process::id();
        let observed = probe.observe(own);
        let ProcessObservation::Found {
            exe: Some(exe),
            started_at: Some(started_at),
        } = observed
        else {
            panic!("cannot observe own process");
        };
        let identity =
            ProcessIdentity::new(own, exe, started_at).expect("identity from live process");
        assert_eq!(probe.validate(&identity), ProcessState::Match);
        // A start time 10 s off the recorded one must not match.
        let stale = ProcessIdentity::new(own, identity.exe(), started_at + Duration::from_secs(10))
            .expect("identity");
        assert_eq!(probe.validate(&stale), ProcessState::Reused);
    }
}
