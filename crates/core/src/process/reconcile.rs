//! Post-crash reconciliation, as pure logic over a process table.
//!
//! `F-INS-04`: engines outlive DBLocal and the operating system may recycle a
//! PID between launches. Reconciliation therefore never trusts a stored PID on
//! its own; it asks the [`ProcessProbe`] to re-validate every identity and maps
//! the answer onto the state machine (`InstanceEvent::ReconciledAbsent` or
//! "adopt as running").
//!
//! This module decides **outcomes** only. The service layer applies them to the
//! store and emits `ServiceEvent`s, which keeps the kernel-survey logic fully
//! unit-testable with a fake probe.

use crate::model::InstanceId;
use crate::process::identity::{ProcessIdentity, ProcessState};
use crate::process::probe::ProcessProbe;

/// One instance the supervisor wants to account for after a launch.
///
/// Only instances that were `Starting`, `Running` or `Stopping` need to be
/// reconciled: everything else is already in a stable status and has no live
/// process to look for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileRecord {
    id: InstanceId,
    identity: Option<ProcessIdentity>,
}

impl ReconcileRecord {
    /// Builds a record. `identity` is the pidfile contents; `None` means the
    /// pidfile was missing or unreadable, which is treated the same as an
    /// absent process.
    pub const fn new(id: InstanceId, identity: Option<ProcessIdentity>) -> Self {
        Self { id, identity }
    }

    /// Instance the record belongs to.
    pub const fn id(&self) -> &InstanceId {
        &self.id
    }

    /// The identity DBLocal recorded when the process was spawned.
    pub const fn identity(&self) -> Option<&ProcessIdentity> {
        match &self.identity {
            Some(identity) => Some(identity),
            None => None,
        }
    }
}

/// What the supervisor should do with one instance.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReconcileOutcome {
    /// The recorded process is still the one we started: adopt it as running.
    Adopted {
        /// Instance that is alive.
        id: InstanceId,
        /// Identity that proved it.
        identity: ProcessIdentity,
    },
    /// No process of ours is running; the instance must become `Stopped`.
    ///
    /// This covers a dead PID, a missing pidfile and a PID that was recycled by
    /// an unrelated process — the latter is the dangerous case `F-INS-04` AC2
    /// exists to prevent.
    ReconciledAbsent {
        /// Instance that must be marked stopped.
        id: InstanceId,
        /// What the probe saw, for logging and tests.
        state: ProcessState,
    },
}

impl ReconcileOutcome {
    /// Instance this outcome belongs to.
    pub const fn id(&self) -> &InstanceId {
        match self {
            Self::Adopted { id, .. } | Self::ReconciledAbsent { id, .. } => id,
        }
    }

    /// Whether the instance is still running.
    pub const fn is_adopted(&self) -> bool {
        matches!(self, Self::Adopted { .. })
    }
}

/// Reconciles every record against the current process table.
///
/// The result has the same length and order as `records`. A `Match` is adopted;
/// every other state — including `Unknown`, which we cannot prove is ours —
/// becomes [`ReconcileOutcome::ReconciledAbsent`] so the app never signals a
/// process it cannot positively identify.
pub fn reconcile(records: &[ReconcileRecord], probe: &dyn ProcessProbe) -> Vec<ReconcileOutcome> {
    records
        .iter()
        .map(|record| reconcile_one(record, probe))
        .collect()
}

fn reconcile_one(record: &ReconcileRecord, probe: &dyn ProcessProbe) -> ReconcileOutcome {
    let Some(identity) = record.identity() else {
        return ReconcileOutcome::ReconciledAbsent {
            id: record.id.clone(),
            state: ProcessState::Absent,
        };
    };
    match probe.validate(identity) {
        ProcessState::Match => ReconcileOutcome::Adopted {
            id: record.id.clone(),
            identity: identity.clone(),
        },
        state => ReconcileOutcome::ReconciledAbsent {
            id: record.id.clone(),
            state,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::probe::ProcessObservation;
    use crate::process::probe::fake::FakeProbe;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn id(s: &str) -> InstanceId {
        // Deterministic UUID-like ids for tests.
        let suffix = format!("{s:0>12}");
        let uuid = format!("00000000-0000-0000-0000-{suffix}");
        InstanceId::parse(uuid).expect("test uuid")
    }

    fn identity(pid: u32, secs: u64) -> ProcessIdentity {
        ProcessIdentity::new(
            pid,
            "/opt/engines/postgres/bin/postgres",
            SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
        )
        .expect("identity")
    }

    fn found(pid_exe: &str, secs: u64) -> ProcessObservation {
        ProcessObservation::Found {
            exe: Some(PathBuf::from(pid_exe)),
            started_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
        }
    }

    #[test]
    fn a_live_match_is_adopted() {
        let probe = FakeProbe::new();
        probe.set(100, found("/opt/engines/postgres/bin/postgres", 1000));
        let records = vec![ReconcileRecord::new(id("1"), Some(identity(100, 1000)))];
        let outcomes = reconcile(&records, &probe);
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].is_adopted());
        assert_eq!(outcomes[0].id(), &id("1"));
    }

    #[test]
    fn a_dead_pid_is_reconciled_absent() {
        let probe = FakeProbe::new();
        let records = vec![ReconcileRecord::new(id("2"), Some(identity(101, 1000)))];
        let outcomes = reconcile(&records, &probe);
        assert_eq!(
            outcomes[0],
            ReconcileOutcome::ReconciledAbsent {
                id: id("2"),
                state: ProcessState::Absent
            }
        );
    }

    #[test]
    fn a_reused_pid_is_reconciled_absent_never_adopted() {
        let probe = FakeProbe::new();
        // The PID is alive but belongs to `sleep` now: W0-B C-08 in miniature.
        probe.set(102, found("/usr/bin/sleep", 2000));
        let records = vec![ReconcileRecord::new(id("3"), Some(identity(102, 1000)))];
        let outcomes = reconcile(&records, &probe);
        assert_eq!(
            outcomes[0],
            ReconcileOutcome::ReconciledAbsent {
                id: id("3"),
                state: ProcessState::Reused
            }
        );
    }

    #[test]
    fn a_missing_pidfile_is_reconciled_absent() {
        let probe = FakeProbe::new();
        let records = vec![ReconcileRecord::new(id("4"), None)];
        let outcomes = reconcile(&records, &probe);
        assert_eq!(
            outcomes[0],
            ReconcileOutcome::ReconciledAbsent {
                id: id("4"),
                state: ProcessState::Absent
            }
        );
    }

    #[test]
    fn an_unprovable_identity_is_never_adopted() {
        let probe = FakeProbe::new();
        probe.set(
            103,
            ProcessObservation::Found {
                exe: None,
                started_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1000)),
            },
        );
        let records = vec![ReconcileRecord::new(id("5"), Some(identity(103, 1000)))];
        let outcomes = reconcile(&records, &probe);
        assert_eq!(
            outcomes[0],
            ReconcileOutcome::ReconciledAbsent {
                id: id("5"),
                state: ProcessState::Unknown
            }
        );
    }

    #[test]
    fn a_mixed_table_preserves_order_and_is_fully_accounted() {
        let probe = FakeProbe::new();
        probe.set(200, found("/opt/engines/postgres/bin/postgres", 10)); // match
        probe.set(201, found("/usr/bin/vim", 20)); // reused
        // 202 absent, 203 missing pidfile
        let records = vec![
            ReconcileRecord::new(id("a"), Some(identity(200, 10))),
            ReconcileRecord::new(id("b"), Some(identity(201, 10))),
            ReconcileRecord::new(id("c"), Some(identity(202, 10))),
            ReconcileRecord::new(id("d"), None),
        ];
        let outcomes = reconcile(&records, &probe);
        assert_eq!(outcomes.len(), 4);
        assert!(outcomes[0].is_adopted());
        assert!(!outcomes[1].is_adopted());
        assert!(!outcomes[2].is_adopted());
        assert!(!outcomes[3].is_adopted());
        assert_eq!(
            outcomes
                .iter()
                .map(ReconcileOutcome::id)
                .collect::<Vec<_>>(),
            vec![&id("a"), &id("b"), &id("c"), &id("d")]
        );
    }
}
