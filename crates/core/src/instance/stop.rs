//! Stopping an instance (`F-INS-03` AC3).

use std::sync::Arc;
use std::time::Duration;

use crate::DblError;
use crate::error::Result;
use crate::model::{InstanceEvent, InstanceId, InstanceStatus};
use crate::process::terminate::TerminateOutcome;
use crate::service::{OperationId, OperationStage};

use super::helpers::{
    OpResult, blocking, claim, db_async, emit_changed, emit_stage, finish_operation, identity_of,
    load, read_credential, run_context,
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
    // `Stopping` is persisted before any signal is sent.
    let claimed = claim(inner, id, InstanceEvent::StopRequested)?;

    let op = inner.ops.next_id();
    let op_id = op.clone();
    let inner = Arc::clone(inner);
    let id = id.clone();
    inner.handle().spawn(async move {
        let _lock = lock;
        emit_stage(&inner, &op, OperationStage::Queued);
        let result = run_stop(Arc::clone(&inner), &op, &id, claimed).await;
        // Whatever happened, the row must not stay in `Stopping`: a stuck
        // transitional status blocks start, stop and delete alike.
        settle_stopped(&inner, &id).await;
        finish_operation(&inner, &op, result);
    });
    Ok(op_id)
}

/// Finishes a stop that could not complete: the process is gone or
/// unidentifiable, so the honest end state is `Stopped` with no identity.
///
/// Only emits a change when it actually settled a row, so a clean stop does not
/// produce a duplicate `InstanceChanged` for the same status.
async fn settle_stopped(inner: &Arc<Inner>, id: &InstanceId) {
    let id2 = id.clone();
    let settled = db_async(inner, move |store| {
        let Some(current) = store.get_instance(&id2)? else {
            return Ok(false);
        };
        if current.status() != InstanceStatus::Stopping {
            return Ok(false);
        }
        store.update_process_identity(&id2, None, None, None)?;
        store.update_status(&id2, InstanceStatus::Stopped)?;
        Ok(true)
    })
    .await
    .unwrap_or(false);
    if settled {
        emit_changed(inner, id);
    }
}

async fn run_stop(
    inner: Arc<Inner>,
    _op: &OperationId,
    id: &InstanceId,
    instance: crate::model::Instance,
) -> OpResult<()> {
    // Stopping has no separate long phase from the caller's view: the engine is
    // asked to shut down and the process layer escalates inside the grace
    // period. `Starting` would be misleading here, so the operation goes
    // `Queued -> Done` with the status changes in between.
    // Ask the engine for a clean shutdown through its adapter, then escalate
    // through the process layer, which re-validates before every signal.
    let credential = read_credential(&inner, &instance)?;
    let ctx = run_context(&inner.paths, &instance, credential);
    let engine = instance.engine();
    let adapter = inner.adapter(engine);
    let ctx_for_shutdown = ctx.clone();
    let graceful = blocking(move || adapter.shutdown(&ctx_for_shutdown)).await;

    let forced = match identity_of(&instance) {
        Some(identity) => {
            let terminator = Arc::clone(&inner.terminator);
            let outcome = blocking(move || {
                terminator.terminate(
                    &identity,
                    Duration::from_secs(crate::consts::STOP_GRACE_SECS),
                )
            })
            .await?;
            matches!(outcome, TerminateOutcome::Forced)
        }
        // No recorded process: a successful adapter shutdown is the whole story.
        None => graceful.is_err(),
    };

    // Clear the identity first, then announce `Stopped`, so a crash in between
    // leaves no pidfile pointing at a process we no longer own.
    let id2 = id.clone();
    let _ = db_async(&inner, move |store| {
        store.update_process_identity(&id2, None, None, None)
    })
    .await;
    let id2 = id.clone();
    db_async(&inner, move |store| {
        let current = store
            .get_instance(&id2)?
            .ok_or_else(|| crate::DblError::NotFound {
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

    if forced {
        tracing::info!(instance = %id, "engine ignored the graceful stop and was force-killed");
    }
    // The "kill paksa" report is the log line plus the `Stopped` event; the
    // frozen `InstanceView` carries no per-stop flag.
    emit_changed(&inner, id);
    Ok(())
}
