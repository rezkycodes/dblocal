//! The `F-INS` instance state machine.
//!
//! [`InstanceStatus::apply`] is the only legal way to move an instance, and it
//! accepts exactly the transitions the PRD draws.

use std::fmt;

use crate::DblError;
use crate::error::Result;

/// Lifecycle status of one instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum InstanceStatus {
    /// The user is filling in the create form; nothing is on disk yet.
    Draft,
    /// The required runtime is being downloaded and verified.
    Provisioning,
    /// `initdb` / config generation is running.
    Initializing,
    /// Not running, ready to start.
    Stopped,
    /// Engine process spawned, health check not passed yet.
    Starting,
    /// Health check passed; accepting connections.
    Running,
    /// Clean shutdown in progress.
    Stopping,
    /// The last operation failed; only retry or delete remain.
    Failed,
    /// Instance and its data are being removed.
    Deleting,
}

impl InstanceStatus {
    /// Stable identifier used in SQLite and the CLI.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Provisioning => "provisioning",
            Self::Initializing => "initializing",
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
            Self::Deleting => "deleting",
        }
    }

    /// Whether the engine process should exist in this status.
    pub const fn holds_process(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }

    /// Whether no further event can move the instance out of this status.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Deleting)
    }

    /// Applies one event, returning the next status or `InvalidTransition`.
    pub fn apply(self, event: InstanceEvent) -> Result<Self> {
        let next = match (self, event) {
            (Self::Draft, InstanceEvent::CreateWithDownload) => Self::Provisioning,
            (Self::Draft, InstanceEvent::CreateWithRuntime) => Self::Initializing,
            (Self::Provisioning, InstanceEvent::DownloadVerified) => Self::Initializing,
            (Self::Provisioning, InstanceEvent::DownloadFailed) => Self::Failed,
            (Self::Initializing, InstanceEvent::InitSucceeded) => Self::Stopped,
            (Self::Initializing, InstanceEvent::InitFailed) => Self::Failed,
            (Self::Stopped | Self::Failed, InstanceEvent::StartRequested) => Self::Starting,
            (Self::Starting, InstanceEvent::HealthCheckPassed) => Self::Running,
            (Self::Starting, InstanceEvent::StartFailed) => Self::Failed,
            (Self::Running, InstanceEvent::StopRequested) => Self::Stopping,
            (Self::Running, InstanceEvent::ProcessExited) => Self::Failed,
            (Self::Stopping, InstanceEvent::ShutdownClean) => Self::Stopped,
            (Self::Stopping, InstanceEvent::ShutdownTimedOut) => Self::Failed,
            (Self::Stopped | Self::Failed, InstanceEvent::DeleteRequested) => Self::Deleting,
            (Self::Starting | Self::Running | Self::Stopping, InstanceEvent::ReconciledAbsent) => {
                Self::Stopped
            }
            (Self::Provisioning | Self::Initializing, InstanceEvent::Interrupted) => Self::Failed,
            _ => {
                return Err(DblError::InvalidTransition {
                    from: self.as_str(),
                    to: event.target_hint(),
                });
            }
        };
        Ok(next)
    }
}

impl fmt::Display for InstanceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Everything that can move an instance.
///
/// The variants mirror the labels on the PRD diagram, so a reader can map a
/// transition in the diagram to a single variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InstanceEvent {
    /// Create, runtime not installed yet.
    CreateWithDownload,
    /// Create, runtime already installed.
    CreateWithRuntime,
    /// Download and checksum verification succeeded.
    DownloadVerified,
    /// Download or checksum verification failed.
    DownloadFailed,
    /// `initdb` or config generation succeeded.
    InitSucceeded,
    /// `initdb` failed.
    InitFailed,
    /// Start or retry requested by the user; valid from `stopped` and `failed`.
    StartRequested,
    /// Protocol-level health check passed.
    HealthCheckPassed,
    /// Port conflict, early exit or health-check timeout.
    StartFailed,
    /// Stop requested by the user.
    StopRequested,
    /// The engine process exited without being asked to.
    ProcessExited,
    /// Clean shutdown completed.
    ShutdownClean,
    /// Shutdown grace period elapsed, a forced kill is needed.
    ShutdownTimedOut,
    /// Delete requested by the user.
    DeleteRequested,
    /// Startup reconciliation found no engine process for this instance.
    ///
    /// This is the normal result after a reboot or logout: engines do not
    /// survive a session (D-09), so a pidfile from the previous session either
    /// points at a dead PID or at a PID reused by a foreign process. PRD
    /// `F-INS-04` AC2 says such an instance is "ditandai stopped", so this is
    /// recovery, not a crash.
    ///
    /// Valid from `starting`, `running` and `stopping`.
    ReconciledAbsent,
    /// The application died in the middle of `provisioning` or `initializing`.
    ///
    /// Without this event an instance interrupted mid-operation would stay in
    /// a non-terminal status forever. PRD `F-INS-04` requires the next launch
    /// to surface it as `failed` so the user can retry or delete it.
    Interrupted,
}

impl InstanceEvent {
    /// Status this event normally leads to, used to build error messages.
    pub const fn target_hint(self) -> &'static str {
        match self {
            Self::CreateWithDownload => "provisioning",
            Self::CreateWithRuntime => "initializing",
            Self::DownloadVerified => "initializing",
            Self::DownloadFailed => "failed",
            Self::InitSucceeded => "stopped",
            Self::InitFailed => "failed",
            Self::StartRequested => "starting",
            Self::HealthCheckPassed => "running",
            Self::StartFailed => "failed",
            Self::StopRequested => "stopping",
            Self::ProcessExited => "failed",
            Self::ShutdownClean => "stopped",
            Self::ShutdownTimedOut => "failed",
            Self::DeleteRequested => "deleting",
            Self::ReconciledAbsent => "stopped",
            Self::Interrupted => "failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InstanceEvent;
    use super::InstanceStatus;
    use super::InstanceStatus::{
        Deleting, Draft, Failed, Initializing, Provisioning, Running, Starting, Stopped, Stopping,
    };
    use crate::DblError;

    const ALL_STATUSES: [InstanceStatus; 9] = [
        Draft,
        Provisioning,
        Initializing,
        Stopped,
        Starting,
        Running,
        Stopping,
        Failed,
        Deleting,
    ];

    const ALL_EVENTS: [InstanceEvent; 16] = [
        InstanceEvent::CreateWithDownload,
        InstanceEvent::CreateWithRuntime,
        InstanceEvent::DownloadVerified,
        InstanceEvent::DownloadFailed,
        InstanceEvent::InitSucceeded,
        InstanceEvent::InitFailed,
        InstanceEvent::StartRequested,
        InstanceEvent::HealthCheckPassed,
        InstanceEvent::StartFailed,
        InstanceEvent::StopRequested,
        InstanceEvent::ProcessExited,
        InstanceEvent::ShutdownClean,
        InstanceEvent::ShutdownTimedOut,
        InstanceEvent::DeleteRequested,
        InstanceEvent::ReconciledAbsent,
        InstanceEvent::Interrupted,
    ];

    /// Every arrow in the PRD `F-INS` diagram, with its mermaid label.
    fn diagram_edges() -> Vec<(InstanceStatus, InstanceEvent, InstanceStatus, &'static str)> {
        use InstanceEvent as E;
        vec![
            (
                Draft,
                E::CreateWithDownload,
                Provisioning,
                "draft -> provisioning",
            ),
            (
                Draft,
                E::CreateWithRuntime,
                Initializing,
                "draft -> initializing",
            ),
            (
                Provisioning,
                E::DownloadVerified,
                Initializing,
                "provisioning -> initializing",
            ),
            (
                Provisioning,
                E::DownloadFailed,
                Failed,
                "provisioning -> failed",
            ),
            (
                Initializing,
                E::InitSucceeded,
                Stopped,
                "initializing -> stopped",
            ),
            (
                Initializing,
                E::InitFailed,
                Failed,
                "initializing -> failed",
            ),
            (Stopped, E::StartRequested, Starting, "stopped -> starting"),
            (
                Starting,
                E::HealthCheckPassed,
                Running,
                "starting -> running",
            ),
            (Starting, E::StartFailed, Failed, "starting -> failed"),
            (Running, E::StopRequested, Stopping, "running -> stopping"),
            (Running, E::ProcessExited, Failed, "running -> failed"),
            (Stopping, E::ShutdownClean, Stopped, "stopping -> stopped"),
            (Stopping, E::ShutdownTimedOut, Failed, "stopping -> failed"),
            (
                Failed,
                E::StartRequested,
                Starting,
                "failed -> starting (retry)",
            ),
            (Stopped, E::DeleteRequested, Deleting, "stopped -> deleting"),
            (Failed, E::DeleteRequested, Deleting, "failed -> deleting"),
            // PRD clarifications of F-INS-04: crash and reboot recovery.
            (
                Starting,
                E::ReconciledAbsent,
                Stopped,
                "starting -> stopped (reconciled absent)",
            ),
            (
                Running,
                E::ReconciledAbsent,
                Stopped,
                "running -> stopped (reconciled absent)",
            ),
            (
                Stopping,
                E::ReconciledAbsent,
                Stopped,
                "stopping -> stopped (reconciled absent)",
            ),
            (
                Provisioning,
                E::Interrupted,
                Failed,
                "provisioning -> failed (interrupted)",
            ),
            (
                Initializing,
                E::Interrupted,
                Failed,
                "initializing -> failed (interrupted)",
            ),
        ]
    }

    #[test]
    fn every_diagram_edge_is_accepted() {
        for (from, event, to, label) in diagram_edges() {
            assert_eq!(
                from.apply(event).ok(),
                Some(to),
                "valid transition rejected: {label}"
            );
        }
    }

    #[test]
    fn every_other_pair_is_rejected() {
        let valid: Vec<(InstanceStatus, InstanceEvent)> = diagram_edges()
            .into_iter()
            .map(|(from, event, _, _)| (from, event))
            .collect();
        for from in ALL_STATUSES {
            for event in ALL_EVENTS {
                if valid.contains(&(from, event)) {
                    continue;
                }
                let result = from.apply(event);
                assert!(
                    matches!(result, Err(DblError::InvalidTransition { .. })),
                    "expected InvalidTransition for {from:?} + {event:?}, got {result:?}"
                );
            }
        }
    }

    #[test]
    fn killing_a_process_observed_alive_still_fails() {
        // `ProcessExited` and `ReconciledAbsent` differ on purpose: a death
        // observed while the app is watching is a failure (F-INS-03 AC4),
        // while the same absence found at startup is normal recovery.
        assert_eq!(
            Running.apply(InstanceEvent::ProcessExited).ok(),
            Some(Failed)
        );
        assert_eq!(
            Running.apply(InstanceEvent::ReconciledAbsent).ok(),
            Some(Stopped)
        );
        assert!(matches!(
            Stopped.apply(InstanceEvent::ProcessExited),
            Err(DblError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn recovery_events_have_stable_hints() {
        assert_eq!(InstanceEvent::ReconciledAbsent.target_hint(), "stopped");
        assert_eq!(InstanceEvent::Interrupted.target_hint(), "failed");
    }

    #[test]
    fn deleting_is_terminal() {
        assert!(Deleting.is_terminal());
        for event in ALL_EVENTS {
            assert!(Deleting.apply(event).is_err());
        }
        for status in ALL_STATUSES.iter().filter(|s| **s != Deleting) {
            assert!(!status.is_terminal(), "{status:?} must not be terminal");
        }
    }

    #[test]
    fn error_names_both_sides() {
        match Stopped.apply(InstanceEvent::StopRequested) {
            Err(DblError::InvalidTransition { from, to }) => {
                assert_eq!(from, "stopped");
                assert_eq!(to, "stopping");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn process_is_held_only_while_transitional_or_running() {
        for status in ALL_STATUSES {
            let expected = matches!(status, Starting | Running | Stopping);
            assert_eq!(status.holds_process(), expected, "{status:?}");
        }
    }

    #[test]
    fn as_str_round_trips_through_tags() {
        let tags = ALL_STATUSES.map(InstanceStatus::as_str);
        assert_eq!(
            tags,
            [
                "draft",
                "provisioning",
                "initializing",
                "stopped",
                "starting",
                "running",
                "stopping",
                "failed",
                "deleting"
            ]
        );
    }
}
