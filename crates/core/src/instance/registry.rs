//! In-process operation registry: one operation per instance at a time.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::DblError;
use crate::download::CancellationToken;
use crate::error::Result;
use crate::model::InstanceId;
use crate::service::OperationId;

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

    /// Forgets an operation when it finishes.
    pub fn finish(&self, id: &OperationId) {
        if let Ok(mut cancels) = self.inner.cancels.lock() {
            cancels.remove(id);
        }
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
