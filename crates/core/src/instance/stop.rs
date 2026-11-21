//! Stopping an instance (`F-INS-03` AC3).
//!
//! The row's final status is decided by what the process table says *after* the
//! attempt, never by whether a signal "succeeded": a stop that cannot prove the
//! engine is gone leaves the row `Running` with its identity intact, so the
//! engine stays controllable instead of becoming an untracked orphan.

use std::sync::Arc;
use std::time::Duration;

use crate::DblError;
use crate::error::Result;
use crate::model::{InstanceEvent, InstanceId, InstanceStatus};
use crate::service::{OperationId, OperationStage};

use super::helpers::{
    OpFailure, OpResult, Stopped, blocking, claim_from, credential_for_stop, db_async,
    emit_changed, emit_stage, emit_stop_forced, finish_operation, identity_of, load, run_context,
    terminate_verified,
};
use super::service::Inner;

pub(crate) fn stop_instance(inner: &Arc<Inner>, id: &InstanceId) -> Result<OperationId> {
    let instance = load(inner, id)?;
    if !instance.status().holds_process() {
        return Err(DblError::InvalidTransition {
            from: instance.status().as_str(),
            to: "stopping",
        });
    }

    let lock = inner.ops.lock_instance(id)?;
    // `Stopping` is persisted before any signal is sent, claimed from the status
    // just read so a competing writer that moved the row makes this call fail as
    // busy rather than double-stopping.
    let claimed = claim_from(inner, id, instance.status(), InstanceEvent::StopRequested)?;

    let op = inner.ops.next_id();
    let op_id = op.clone();
    let inner = Arc::clone(inner);
    let id = id.clone();
    inner.handle().spawn(async move {
        let lock = lock;
        emit_stage(&inner, &op, OperationStage::Queued);
        let result = run_stop(Arc::clone(&inner), &id, claimed).await;
        drop(lock);
        finish_operation(&inner, &op, result);
    });
    Ok(op_id)
}

async fn run_stop(
    inner: Arc<Inner>,
    id: &InstanceId,
    instance: crate::model::Instance,
) -> OpResult<()> {
    // A stop has no separate long phase from the caller's view: the engine is
    // asked to shut down and the process layer escalates inside the grace
    // period. The operation goes `Queued -> Done` with the status change in
    // between.

    // The graceful path is best-effort. An unreadable credential only removes
    // the adapter's ability to authenticate; escalation below still runs.
    let credential = credential_for_stop(&inner, &instance);
    let ctx = run_context(&inner.paths, &instance, credential);
    let engine = instance.engine();
    let adapter = inner.adapter(engine);
    let ctx_for_shutdown = ctx.clone();
    let graceful = blocking(move || adapter.shutdown(&ctx_for_shutdown)).await;
    let graceful_ok = graceful.is_ok();

    let identity = identity_of(&instance);
    let had_identity = identity.is_some();
    let terminated = terminate_verified(
        &inner,
        identity,
        Duration::from_secs(crate::consts::STOP_GRACE_SECS),
    )
    .await;

    // No recorded process at all: a successful adapter shutdown is the whole
    // story. Otherwise the termination probe decided it.
    let state = if had_identity {
        terminated.state
    } else if graceful_ok {
        Stopped::Gone
    } else {
        // Nothing was recorded and the engine could not be asked to stop, so
        // nothing proves it is gone.
        Stopped::StillAlive
    };

    match state {
        Stopped::Gone => {
            // Clear the identity first, then announce `Stopped`, so a crash in
            // between leaves no pidfile pointing at a process we no longer own.
            clear_identity_and_stop(&inner, id).await?;
            // A forced kill is reported once, before the final change, and only
            // when the terminator really escalated.
            if terminated.forced {
                emit_stop_forced(&inner, id);
            }
            emit_changed(&inner, id);
            Ok(())
        }
        Stopped::StillAlive => {
            // The engine survives: restore `Running` and keep the identity so a
            // retried stop (or reconciliation) can still act on it.
            let id2 = id.clone();
            let restored = db_async(&inner, move |store| {
                store.transition_status(&id2, InstanceStatus::Stopping, InstanceStatus::Running)
            })
            .await
            .unwrap_or(false);
            if !restored {
                tracing::warn!(instance = %id, "could not restore Running after a failed stop");
            }
            emit_changed(&inner, id);
            Err(OpFailure::new(DblError::Engine {
                reason: format!("the engine for {id} is still running"),
            }))
        }
    }
}

/// Clears the identity and moves `Stopping` to `Stopped` in one store step.
async fn clear_identity_and_stop(inner: &Arc<Inner>, id: &InstanceId) -> OpResult<()> {
    let id2 = id.clone();
    db_async(inner, move |store| {
        store.update_process_identity(&id2, None, None, None)?;
        let current = store
            .get_instance(&id2)?
            .ok_or_else(|| DblError::NotFound {
                kind: "instance",
                name: id2.as_str().to_owned(),
            })?;
        // Only persist the terminal state when we still own the transition; if
        // the supervisor already moved the row, leave that decision alone.
        if current.status() == InstanceStatus::Stopping {
            store.update_status(&id2, InstanceStatus::Stopped)?;
        }
        Ok(())
    })
    .await?;
    Ok(())
}
