//! In-process operation registry: one operation per instance at a time.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::DblError;
use crate::download::CancellationToken;
use crate::error::Result;
use crate::model::InstanceId;
use crate::service::OperationId;

/// How many finished-operation ids are retained.
///
/// The set exists only so a consumer that subscribed after an operation finished
/// can still notice it completed. Nothing cares about an operation that finished
/// long ago, so a small window is plenty; the cap stops a long-lived process from
/// growing one entry per operation forever.
pub const FINISHED_HISTORY: usize = 1024;

/// Operation ids and per-instance exclusivity.
#[derive(Clone, Debug, Default)]
pub struct Ops {
    inner: Arc<OpsInner>,
    next: Arc<AtomicU64>,
}

#[derive(Debug, Default)]
struct OpsInner {
    locked: Mutex<HashSet<InstanceId>>,
    cancels: Mutex<HashMap<OperationId, CancellationToken>>,
    /// Operations that have reported `OperationFinished`, oldest first.
    ///
    /// Kept so a consumer that subscribed after the operation finished (a test
    /// harness, or a late UI) can still learn that it completed instead of
    /// waiting forever on an event it never saw. Bounded to the most recent
    /// [`FINISHED_HISTORY`] ids: a `VecDeque` in insertion order evicts the oldest
    /// when a new one arrives, so a process that runs for a week does not retain
    /// one id per operation.
    finished: Mutex<VecDeque<OperationId>>,
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
    pub fn lock_instance(&self, id: &InstanceId) -> Result<InstanceLock> {
        let mut locked = self.inner.locked.lock().map_err(|_| poisoned())?;
        if !locked.insert(id.clone()) {
            return Err(DblError::InvalidTransition {
                from: "busy",
                to: "operation",
            });
        }
        Ok(InstanceLock {
            inner: Arc::clone(&self.inner),
            id: id.clone(),
        })
    }

    /// Whether an operation currently owns `id` in this process.
    ///
    /// Reconciliation uses this so it never mistakes a create that is still
    /// running for one interrupted by a crash.
    pub fn is_busy(&self, id: &InstanceId) -> bool {
        self.inner
            .locked
            .lock()
            .map(|locked| locked.contains(id))
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
    /// Idempotent: a repeated `finish` for the same id must not append a second
    /// copy, because that would evict a still-recent operation from the window
    /// early.
    pub fn finish(&self, id: &OperationId) {
        if let Ok(mut cancels) = self.inner.cancels.lock() {
            cancels.remove(id);
        }
        if let Ok(mut finished) = self.inner.finished.lock() {
            if finished.contains(id) {
                return;
            }
            finished.push_back(id.clone());
            // Evict the oldest beyond the window: a consumer only ever asks
            // about an operation it just missed, so old ids are worthless and
            // must not accumulate.
            while finished.len() > FINISHED_HISTORY {
                finished.pop_front();
            }
        }
    }

    /// Whether `id` has reported `OperationFinished` and is still in the window.
    ///
    /// Lets a caller that may have missed the broadcast (it subscribed after the
    /// operation completed) detect completion by polling, instead of blocking on
    /// an event that will never be re-delivered.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_finished(&self, id: &OperationId) -> bool {
        self.inner
            .finished
            .lock()
            .map(|finished| finished.contains(id))
            .unwrap_or(false)
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
#[derive(Debug)]
pub struct InstanceLock {
    inner: Arc<OpsInner>,
    id: InstanceId,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        if let Ok(mut locked) = self.inner.locked.lock() {
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
            ops.finish(&id);
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
        ops.finish(&id);
        ops.finish(&id);
        assert!(ops.is_finished(&id));
        assert_eq!(
            ops.finished_len(),
            1,
            "a repeated finish must not append a second copy that could evict a live id"
        );
    }
}
