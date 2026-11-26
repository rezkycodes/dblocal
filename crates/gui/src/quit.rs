//! Quit-flow state machine (F-TRY-02).
//!
//! The GUI calls into this pure machine when the user asks to quit. The machine
//! tells the caller what to do next: show the dialog, stop instances through the
//! service, or exit. It never touches GPUI or the service directly.

use std::time::Duration;

use dblocal_core::model::InstanceId;
use dblocal_core::service::{InstanceView, OperationId};

use crate::format;

/// Bound on how long "Stop all, then quit" waits for every stop to
/// finish. Derived from the engine grace period (never a literal) with a margin
/// for the service to report the outcome.
pub const QUIT_STOP_TIMEOUT: Duration =
    Duration::from_secs(dblocal_core::consts::STOP_GRACE_SECS + 10);

/// Pure quit state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuitState {
    /// No quit in progress.
    Idle,
    /// Waiting for the user's choice. `running` are the instances that were
    /// running when the dialog opened.
    Confirming { running: Vec<InstanceId> },
    /// Stopping instances before exit. `pending` are stop operations still in
    /// flight (by instance id).
    StoppingAll {
        pending: Vec<InstanceId>,
        failed: Vec<InstanceId>,
    },
    /// All requested stops finished (some may have failed) and the user can
    /// decide to exit anyway or cancel.
    StopFailed { failed: Vec<InstanceId> },
    /// Ready to exit.
    Done,
}

/// Events that drive the quit machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuitEvent {
    /// User requested quit (tray Quit or window close with no tray).
    QuitRequested { running: Vec<InstanceId> },
    /// User chose to keep engines running.
    KeepRunning,
    /// User chose to stop all running instances.
    StopAll,
    /// One stop operation completed (the operation finished, not merely accepted).
    StopFinished { id: InstanceId, ok: bool },
    /// The bounded stop wait elapsed before every stop finished.
    Timeout,
    /// User cancelled the dialog.
    Cancel,
    /// User confirmed exit despite failures or pending stops.
    ForceExit,
}

/// What the caller should do after a transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuitAction {
    /// Nothing.
    None,
    /// Show the quit confirmation dialog.
    ShowDialog,
    /// Issue stop requests for these instances.
    StopInstances(Vec<InstanceId>),
    /// Exit the process/app.
    Exit,
}

/// Pure quit state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuitMachine {
    state: QuitState,
}

impl QuitMachine {
    /// Creates a machine in the idle state.
    pub fn new() -> Self {
        Self {
            state: QuitState::Idle,
        }
    }

    /// Current state.
    pub fn state(&self) -> &QuitState {
        &self.state
    }

    /// Whether the dialog should be visible.
    pub fn is_confirming(&self) -> bool {
        matches!(
            self.state,
            QuitState::Confirming { .. }
                | QuitState::StoppingAll { .. }
                | QuitState::StopFailed { .. }
        )
    }

    /// Applies an event and returns the action to take.
    pub fn apply(&mut self, event: QuitEvent) -> QuitAction {
        match (&self.state, event) {
            (QuitState::Idle, QuitEvent::QuitRequested { running }) => {
                if running.is_empty() {
                    self.state = QuitState::Done;
                    QuitAction::Exit
                } else {
                    self.state = QuitState::Confirming { running };
                    QuitAction::ShowDialog
                }
            }
            (QuitState::Confirming { running }, QuitEvent::KeepRunning) => {
                let _ = running;
                self.state = QuitState::Done;
                QuitAction::Exit
            }
            (QuitState::Confirming { running }, QuitEvent::StopAll) => {
                let pending = running.clone();
                self.state = QuitState::StoppingAll {
                    pending: pending.clone(),
                    failed: Vec::new(),
                };
                QuitAction::StopInstances(pending)
            }
            (QuitState::Confirming { .. }, QuitEvent::Cancel) => {
                self.state = QuitState::Idle;
                QuitAction::None
            }
            (QuitState::StoppingAll { pending, failed }, QuitEvent::StopFinished { id, ok }) => {
                let mut pending = pending.clone();
                let mut failed = failed.clone();
                pending.retain(|p| p != &id);
                if !ok {
                    failed.push(id);
                }
                if pending.is_empty() {
                    if failed.is_empty() {
                        self.state = QuitState::Done;
                        QuitAction::Exit
                    } else {
                        self.state = QuitState::StopFailed { failed };
                        QuitAction::ShowDialog
                    }
                } else {
                    self.state = QuitState::StoppingAll { pending, failed };
                    QuitAction::None
                }
            }
            (QuitState::StoppingAll { .. }, QuitEvent::ForceExit) => {
                self.state = QuitState::Done;
                QuitAction::Exit
            }
            (QuitState::StoppingAll { pending, failed }, QuitEvent::Timeout) => {
                let mut failed = failed.clone();
                failed.extend(pending.iter().cloned());
                self.state = QuitState::StopFailed { failed };
                QuitAction::ShowDialog
            }
            (QuitState::StoppingAll { .. }, QuitEvent::Cancel) => {
                // Cancel while stopping is not allowed; stops already issued.
                QuitAction::None
            }
            (QuitState::StopFailed { .. }, QuitEvent::ForceExit) => {
                self.state = QuitState::Done;
                QuitAction::Exit
            }
            (QuitState::StopFailed { .. }, QuitEvent::Cancel) => {
                self.state = QuitState::Idle;
                QuitAction::None
            }
            _ => QuitAction::None,
        }
    }
}

impl Default for QuitMachine {
    fn default() -> Self {
        Self::new()
    }
}

/// Ordering-safe ledger of quit-issued stop operations (R2).
///
/// `stop_instance` returns its operation id on the background executor, while
/// `OperationFinished` travels bridge thread -> flume -> event pump. Nothing
/// orders the two, so a fast stop can finish before it is registered. The
/// ledger remembers such an outcome (`early`) and hands it back when the
/// registration arrives. `early` is bounded by the number of stops the quit
/// flow issues and is cleared whenever the flow leaves `StoppingAll`.
#[derive(Debug, Default)]
pub struct StopLedger {
    pending: std::collections::HashMap<OperationId, InstanceId>,
    early: std::collections::HashMap<OperationId, bool>,
}

impl StopLedger {
    /// An empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears both maps (the flow left `StoppingAll`).
    pub fn clear(&mut self) {
        self.pending.clear();
        self.early.clear();
    }

    /// Registers a stop. Returns the outcome when it already arrived.
    pub fn register(&mut self, op: OperationId, id: InstanceId) -> Option<(InstanceId, bool)> {
        match self.early.remove(&op) {
            Some(ok) => {
                self.pending.remove(&op);
                Some((id, ok))
            }
            None => {
                self.pending.insert(op, id);
                None
            }
        }
    }

    /// Records an outcome. Returns the instance id when the stop was already
    /// registered; otherwise the outcome is kept for a later registration.
    pub fn finished(&mut self, op: &OperationId, ok: bool) -> Option<(InstanceId, bool)> {
        match self.pending.remove(op) {
            Some(id) => Some((id, ok)),
            None => {
                self.early.insert(op.clone(), ok);
                None
            }
        }
    }

    /// Stops still waiting for their `OperationFinished`.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Pending operation ids.
    pub fn pending_ops(&self) -> Vec<OperationId> {
        self.pending.keys().cloned().collect()
    }

    /// Outcomes kept for a registration that has not arrived yet.
    pub fn early_len(&self) -> usize {
        self.early.len()
    }

    /// Whether nothing is in flight.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.early.is_empty()
    }
}

/// Collects ids of instances that are currently running (or starting/stopping
/// in a way that means an engine process may still be alive).
pub fn running_instance_ids(views: &[InstanceView]) -> Vec<InstanceId> {
    views
        .iter()
        .filter(|v| format::can_stop(v.status) || v.status.as_str() == "starting")
        .map(|v| v.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dblocal_core::model::{
        AuthMode, EngineKey, Instance, InstanceName, InstanceSpec, InstanceStatus, Port,
    };
    use std::time::SystemTime;

    fn id(n: u128) -> InstanceId {
        InstanceId::parse(format!("00000000-0000-4000-8000-{n:012x}")).expect("id")
    }
    fn view(status: InstanceStatus) -> InstanceView {
        let spec = InstanceSpec::new(
            InstanceName::new("app").expect("n"),
            EngineKey::Postgresql,
            "17.4",
            Some(Port::new(5432).expect("p")),
            AuthMode::Password,
        )
        .expect("s");
        let inst = Instance::new(&spec, InstanceId::new(), status, SystemTime::UNIX_EPOCH);
        InstanceView::from_instance(&inst)
    }

    #[test]
    fn quit_with_no_running_instances_exits() {
        let mut m = QuitMachine::new();
        let action = m.apply(QuitEvent::QuitRequested { running: vec![] });
        assert_eq!(action, QuitAction::Exit);
        assert_eq!(m.state(), &QuitState::Done);
    }

    #[test]
    fn quit_with_running_shows_dialog_and_keep_running_exits() {
        let mut m = QuitMachine::new();
        let a = id(1);
        let action = m.apply(QuitEvent::QuitRequested {
            running: vec![a.clone()],
        });
        assert_eq!(action, QuitAction::ShowDialog);
        assert!(m.is_confirming());
        let action = m.apply(QuitEvent::KeepRunning);
        assert_eq!(action, QuitAction::Exit);
        assert_eq!(m.state(), &QuitState::Done);
    }

    #[test]
    fn stop_all_stops_then_exits_when_all_ok() {
        let mut m = QuitMachine::new();
        let a = id(1);
        let b = id(2);
        m.apply(QuitEvent::QuitRequested {
            running: vec![a.clone(), b.clone()],
        });
        let action = m.apply(QuitEvent::StopAll);
        assert_eq!(
            action,
            QuitAction::StopInstances(vec![a.clone(), b.clone()])
        );
        assert_eq!(
            m.apply(QuitEvent::StopFinished { id: a, ok: true }),
            QuitAction::None
        );
        let action = m.apply(QuitEvent::StopFinished { id: b, ok: true });
        assert_eq!(action, QuitAction::Exit);
    }

    #[test]
    fn stop_failure_reports_and_allows_cancel_or_force() {
        let mut m = QuitMachine::new();
        let a = id(1);
        m.apply(QuitEvent::QuitRequested {
            running: vec![a.clone()],
        });
        m.apply(QuitEvent::StopAll);
        let action = m.apply(QuitEvent::StopFinished {
            id: a.clone(),
            ok: false,
        });
        assert_eq!(action, QuitAction::ShowDialog);
        assert!(matches!(m.state(), QuitState::StopFailed { .. }));
        // cancel returns to idle
        let mut m2 = QuitMachine::new();
        m2.apply(QuitEvent::QuitRequested {
            running: vec![a.clone()],
        });
        m2.apply(QuitEvent::StopAll);
        m2.apply(QuitEvent::StopFinished {
            id: a.clone(),
            ok: false,
        });
        assert_eq!(m2.apply(QuitEvent::Cancel), QuitAction::None);
        assert_eq!(m2.state(), &QuitState::Idle);
        // force exits
        let mut m3 = QuitMachine::new();
        m3.apply(QuitEvent::QuitRequested {
            running: vec![a.clone()],
        });
        m3.apply(QuitEvent::StopAll);
        m3.apply(QuitEvent::StopFinished { id: a, ok: false });
        assert_eq!(m3.apply(QuitEvent::ForceExit), QuitAction::Exit);
    }

    fn op(name: &str) -> OperationId {
        OperationId::new(name)
    }

    #[test]
    fn stop_ledger_keeps_an_early_outcome_for_its_registration() {
        let mut ledger = StopLedger::new();
        let a = id(1);
        // The event arrives before the request continuation registered the stop.
        assert_eq!(ledger.finished(&op("op-1"), true), None);
        assert_eq!(ledger.early_len(), 1);
        assert_eq!(ledger.pending_len(), 0);
        assert_eq!(
            ledger.register(op("op-1"), a.clone()),
            Some((a.clone(), true)),
            "the buffered outcome must be consumed by the registration"
        );
        assert!(ledger.is_empty());
    }

    #[test]
    fn stop_ledger_hands_back_an_outcome_for_a_registered_stop() {
        let mut ledger = StopLedger::new();
        let b = id(2);
        assert_eq!(ledger.register(op("op-2"), b.clone()), None);
        assert_eq!(ledger.pending_len(), 1);
        assert_eq!(
            ledger.finished(&op("op-2"), false),
            Some((b, false)),
            "a registered stop reports its error outcome"
        );
        assert!(ledger.is_empty());
    }

    #[test]
    fn stop_ledger_clear_drops_both_sides() {
        let mut ledger = StopLedger::new();
        let a = id(1);
        ledger.register(op("pending"), a);
        let _ = ledger.finished(&op("early"), true);
        assert!(!ledger.is_empty());
        ledger.clear();
        assert!(ledger.is_empty());
        assert_eq!(ledger.pending_len(), 0);
        assert_eq!(ledger.early_len(), 0);
    }

    #[test]
    fn running_instance_ids_include_running_and_starting() {
        let views = vec![
            view(InstanceStatus::Running),
            view(InstanceStatus::Starting),
            view(InstanceStatus::Stopped),
            view(InstanceStatus::Failed),
        ];
        let ids = running_instance_ids(&views);
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn timeout_while_stopping_fails_pending_stops() {
        let mut m = QuitMachine::new();
        let a = id(1);
        let b = id(2);
        m.apply(QuitEvent::QuitRequested {
            running: vec![a.clone(), b.clone()],
        });
        m.apply(QuitEvent::StopAll);
        // b finished ok, a still pending when timeout fires.
        m.apply(QuitEvent::StopFinished { id: b, ok: true });
        let action = m.apply(QuitEvent::Timeout);
        assert_eq!(action, QuitAction::ShowDialog);
        assert_eq!(m.state(), &QuitState::StopFailed { failed: vec![a] });
    }

    #[test]
    fn timeout_ignored_outside_stopping() {
        let mut m = QuitMachine::new();
        assert_eq!(m.apply(QuitEvent::Timeout), QuitAction::None);
        let a = id(1);
        m.apply(QuitEvent::QuitRequested { running: vec![a] });
        assert_eq!(m.apply(QuitEvent::Timeout), QuitAction::None);
    }
}
