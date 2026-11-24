//! Operating-system process and port handling, engine-independent.
//!
//! Owned by W1-D. Everything an engine adapter needs to start, prove, stop and
//! re-find a child process lives here:
//!
//! - [`identity`] — what we record about a process and how to prove it is ours.
//! - [`probe`] — the [`ProcessProbe`] trait, its `sysinfo` implementation and a
//!   fake used by tests.
//! - [`spawn`] — [`spawn_detached`], the only place an engine child is born.
//! - [`terminate`] — [`terminate`], which validates before every signal.
//! - [`reconcile`] — pure post-crash recovery logic.
//! - [`logs`] — tail, follow and rotate the per-instance log (`F-LOG-01`).
//!
//! Ports live in [`crate::ports`] so a caller that only needs `F-PRT-02` does not
//! pull in the process table.

pub mod identity;
pub mod logs;
pub mod probe;
pub mod reconcile;
pub mod spawn;
pub mod terminate;

#[cfg(test)]
mod evidence;

pub use identity::{ProcessIdentity, ProcessState, START_TOLERANCE};
pub use logs::{LogFollower, follow_forever, tail_lines};
pub use probe::{ProcessObservation, ProcessProbe, SysinfoProbe};
pub use reconcile::{ReconcileOutcome, ReconcileRecord, reconcile};
pub use spawn::spawn_detached;
pub use terminate::{
    KILL_CONFIRM, SignalSender, TERMINATE_POLL, TerminateOutcome, await_exit, await_exit_with,
    terminate, terminate_with,
};
