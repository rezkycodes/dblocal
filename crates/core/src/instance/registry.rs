//! In-process operation registry: one operation per instance at a time.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::DblError;
use crate::download::CancellationToken;
use crate::error::Result;
use crate::model::InstanceId;
use crate::service::{OperationId, UserFacingError};

/// How many finished-operation ids are retained.
///
/// The set exists only so a consumer that subscribed after an operation finished
/// can still notice it completed. Nothing cares about an operation that finished
/// long ago, so a small window is plenty; the cap stops a long-lived process from
/// growing one entry per operation forever.
pub const FINISHED_HISTORY: usize = 1024;

/// Who owns an instance's guard.
///
/// The distinction is what lets a real operation wait for startup reconciliation
/// instead of being refused: reconciliation holds the guard only for the brief
/// startup pass, and a user command that arrives then did nothing wrong, so it
/// waits bounded rather than failing with "busy". A guard held by a real operation
/// still refuses at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LockKind {
    /// A user-visible operation (create/start/stop/delete/...).
    Operation,
    /// The startup reconciliation pass.
    Reconciliation,
}

/// Outcome of a finished operation, kept so a late subscriber can still observe it.
pub type OperationOutcome = std::result::Result<(), UserFacingError>;

/// How long an operation waits for a reconciliation guard before reporting busy.
pub const RECONCILE_LOCK_WAIT_MS: u64 = 5_000;
/// Poll interval while waiting for a reconciliation guard.
const RECONCILE_LOCK_POLL_MS: u64 = 10;

/// Operation ids and per-instance exclusivity.
#[derive(Clone, Debug, Default)]
pub struct Ops {
    inner: Arc<OpsInner>,
    next: Arc<AtomicU64>,
}

#[derive(Debug, Default)]
struct OpsInner {
    locked: Mutex<HashMap<InstanceId, LockKind>>,
    cancels: Mutex<HashMap<OperationId, CancellationToken>>,
    /// Operations that have reported `OperationFinished`, oldest first.
    ///
    /// Kept so a consumer that subscribed after the operation finished (a test
    /// harness, or a late UI) can still learn that it completed instead of
    /// waiting forever on an event it never saw. Bounded to the most recent
    /// [`FINISHED_HISTORY`] ids: a `VecDeque` in insertion order evicts the oldest
    /// when a new one arrives, so a process that runs for a week does not retain
    /// one id per operation. The outcome is stored alongside the id so a consumer
    /// that missed the broadcast can still observe success or failure.
    finished: Mutex<VecDeque<(OperationId, OperationOutcome)>>,
    /// Test-only hook fired by [`Ops::finish`] the instant the durable result is
    /// recorded, before `finish_operation` returns to its caller. It lives here,
    /// not in `finish_operation`, so a test that swaps the two statements there
    /// (registry write vs. broadcast) is caught: with the registry write first, the
    /// hook runs with nothing yet in the channel.
    #[cfg(test)]
    finish_hook: Mutex<Option<FinishHookBox>>,
}

/// Test-only seam closure, wrapped so [`OpsInner`] can keep deriving `Debug`.
#[cfg(test)]
struct FinishHookBox(Box<dyn FnOnce(&OperationId) + Send>);

#[cfg(test)]
impl std::fmt::Debug for FinishHookBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FinishHook")
    }
}

impl Ops {
    /// Builds an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates a fresh operation id.
    pub fn next_id(&self) -> OperationId {
        let number = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        OperationId::new(format!("op-{number}"))
    }

    /// Reserves `id` for one operation, or reports it as busy.
    ///
    /// A guard held by a **real** operation refuses at once: that is a genuine
    /// busy instance. A guard held by **startup reconciliation** is waited on,
    /// bounded, because a user command issued within milliseconds of process start
    /// (a one-shot `dblocal stop`, a tray click right after launch) did nothing
    /// wrong and the reconciliation pass is short. If reconciliation does not
    /// release within [`RECONCILE_LOCK_WAIT_MS`], the call is refused as busy just
    /// like any other.
    pub fn lock_instance(&self, id: &InstanceId) -> Result<InstanceLock> {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(RECONCILE_LOCK_WAIT_MS);
        loop {
            let waited = {
                let mut locked = self.inner.locked.lock().map_err(|_| poisoned())?;
                match locked.get(id) {
                    None => {
                        locked.insert(id.clone(), LockKind::Operation);
                        return Ok(InstanceLock {
                            inner: Arc::clone(&self.inner),
                            id: id.clone(),
                            kind: LockKind::Operation,
                        });
                    }
                    // A real operation owns it: refuse, never wait.
                    Some(LockKind::Operation) => false,
                    // Reconciliation owns it briefly: wait for it.
                    Some(LockKind::Reconciliation) => true,
                }
            };
            if !waited {
                return Err(DblError::InvalidTransition {
                    from: "busy",
                    to: "operation",
                });
            }
            if std::time::Instant::now() >= deadline {
                return Err(DblError::InvalidTransition {
                    from: "busy",
                    to: "operation",
                });
            }
            std::thread::sleep(std::time::Duration::from_millis(RECONCILE_LOCK_POLL_MS));
        }
    }

    /// Reserves `id` for the startup reconciliation pass.
    ///
    /// Refuses when any guard is already held (a real operation owns the row, or a
    /// concurrent reconciliation is deciding it): reconciliation must not wait, it
    /// simply skips a row it cannot own right now.
    pub fn lock_instance_for_reconcile(&self, id: &InstanceId) -> Result<InstanceLock> {
        let mut locked = self.inner.locked.lock().map_err(|_| poisoned())?;
        if locked.contains_key(id) {
            return Err(DblError::InvalidTransition {
                from: "busy",
                to: "reconcile",
            });
        }
        locked.insert(id.clone(), LockKind::Reconciliation);
        Ok(InstanceLock {
            inner: Arc::clone(&self.inner),
            id: id.clone(),
            kind: LockKind::Reconciliation,
        })
    }

    /// Whether an operation currently owns `id` in this process.
    ///
    /// The supervisor uses this to skip a row an operation holds. Reconciliation
    /// does **not** use it: it takes the instance lock itself
    /// (`lock_instance_for_reconcile`) and re-reads the row, which both excludes an
    /// in-flight operation and proves the row it acts on is fresh, so a snapshot it
    /// took earlier cannot be overwritten. A caller that needs the exclusion *and*
    /// a fresh row must lock, not read this.
    pub fn is_busy(&self, id: &InstanceId) -> bool {
        self.inner
            .locked
            .lock()
            .map(|locked| locked.contains_key(id))
            .unwrap_or(true)
    }

    /// Starts tracking a cancellable operation.
    pub fn register(&self, id: &OperationId, token: CancellationToken) -> Result<()> {
        let mut cancels = self.inner.cancels.lock().map_err(|_| poisoned())?;
        cancels.insert(id.clone(), token);
        Ok(())
    }

    /// Cancels an operation if it is still cancellable.
    pub fn cancel(&self, id: &OperationId) -> bool {
        let token = self
            .inner
            .cancels
            .lock()
            .ok()
            .and_then(|mut cancels| cancels.remove(id));
        match token {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Records that an operation finished and drops its cancellation token.
    ///
    /// Stores the final outcome so a consumer that missed the broadcast can still
    /// observe success or failure. Idempotent: a repeated `finish` for the same id
    /// must not append a second copy, because that would evict a still-recent
    /// operation from the window early.
    pub fn finish(&self, id: &OperationId, result: OperationOutcome) {
        if let Ok(mut cancels) = self.inner.cancels.lock() {
            cancels.remove(id);
        }
        if let Ok(mut finished) = self.inner.finished.lock() {
            if finished.iter().any(|(finished_id, _)| finished_id == id) {
                return;
            }
            finished.push_back((id.clone(), result));
            // Evict the oldest beyond the window: a consumer only ever asks
            // about an operation it just missed, so old ids are worthless and
            // must not accumulate.
            while finished.len() > FINISHED_HISTORY {
                finished.pop_front();
            }
        }
        // Fire the seam here, at the exact instant the durable result is recorded.
        // A consumer that stops as soon as the registry says "finished" relies on
        // `OperationFinished` already being in its channel, so this hook observing an
        // empty channel means the broadcast had not happened yet.
        #[cfg(test)]
        if let Some(FinishHookBox(hook)) = self
            .inner
            .finish_hook
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
        {
            hook(id);
        }
    }

    /// Registers the one-shot test hook fired by [`Ops::finish`] the instant the
    /// durable result is recorded.
    #[cfg(test)]
    pub(crate) fn on_finish_recorded(&self, hook: impl FnOnce(&OperationId) + Send + 'static) {
        if let Ok(mut slot) = self.inner.finish_hook.lock() {
            *slot = Some(FinishHookBox(Box::new(hook)));
        }
    }

    /// Whether `id` has reported `OperationFinished` and is still in the window.
    ///
    /// Lets a caller that may have missed the broadcast (it subscribed after the
    /// operation completed) detect completion by polling, instead of blocking on
    /// an event that will never be re-delivered.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_finished(&self, id: &OperationId) -> bool {
        self.operation_result(id).is_some()
    }

    /// Returns the final outcome of `id` when it finished inside the retained
    /// window.
    ///
    /// This is the durable completion path for callers that may have missed the
    /// `OperationFinished` broadcast (a late subscriber, or a harness running
    /// under load where broadcast lag dropped the event). Presence means the
    /// operation completed; the contained `Result` tells whether it succeeded.
    pub fn operation_result(&self, id: &OperationId) -> Option<OperationOutcome> {
        self.inner.finished.lock().ok().and_then(|finished| {
            finished
                .iter()
                .rev()
                .find_map(|(finished_id, result)| (finished_id == id).then(|| result.clone()))
        })
    }

    /// How many finished-operation ids are currently retained, for the bound
    /// test.
    #[cfg(test)]
    pub fn finished_len(&self) -> usize {
        self.inner
            .finished
            .lock()
            .map(|finished| finished.len())
            .unwrap_or(0)
    }
}

fn poisoned() -> DblError {
    DblError::Storage {
        reason: "the operation registry lock was poisoned".to_owned(),
    }
}

/// Released on drop, so an operation that panics or returns early unlocks.
///
/// The `kind` is what the guard was taken with, so `Drop` never removes a guard a
/// different owner installed meanwhile (in practice none can, since the map entry
/// is single-owner, but recording it keeps the intent explicit).
#[derive(Debug)]
pub struct InstanceLock {
    inner: Arc<OpsInner>,
    id: InstanceId,
    kind: LockKind,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        if let Ok(mut locked) = self.inner.locked.lock()
            && locked.get(&self.id) == Some(&self.kind)
        {
            locked.remove(&self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_finished_history_is_bounded_and_keeps_the_newest() {
        let ops = Ops::new();
        // One more than the window, so eviction must have happened.
        let total = FINISHED_HISTORY + 76;
        let mut ids = Vec::with_capacity(total);
        for _ in 0..total {
            let id = ops.next_id();
            ops.finish(&id, Ok(()));
            ids.push(id);
        }

        assert_eq!(
            ops.finished_len(),
            FINISHED_HISTORY,
            "the history never grows past its bound"
        );
        // The newest are present; the oldest are gone.
        assert!(
            ops.is_finished(ids.last().expect("at least one")),
            "the most recent finish is retained"
        );
        assert!(
            ops.is_finished(&ids[total - FINISHED_HISTORY]),
            "the oldest id still inside the window is retained"
        );
        assert!(!ops.is_finished(&ids[0]), "the first id has been evicted");
        assert!(
            !ops.is_finished(&ids[total - FINISHED_HISTORY - 1]),
            "one past the window is evicted"
        );
    }

    #[test]
    fn finishing_twice_does_not_double_count() {
        let ops = Ops::new();
        let id = ops.next_id();
        ops.finish(&id, Ok(()));
        ops.finish(&id, Ok(()));
        assert!(ops.is_finished(&id));
        assert_eq!(
            ops.finished_len(),
            1,
            "a repeated finish must not append a second copy that could evict a live id"
        );
    }

    #[test]
    fn a_missed_broadcast_still_yields_the_result() {
        let ops = Ops::new();
        let ok = ops.next_id();
        ops.finish(&ok, Ok(()));
        assert_eq!(ops.operation_result(&ok), Some(Ok(())));

        let err = ops.next_id();
        let failure = crate::service::UserFacingError::new(
            crate::error::CODE_ENGINE,
            crate::strings::ERR_ENGINE,
        );
        ops.finish(&err, Err(failure.clone()));
        assert_eq!(ops.operation_result(&err), Some(Err(failure)));
    }
}
