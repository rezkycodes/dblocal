//! Deleting an instance (`F-INS-05`, D-18).

use std::sync::Arc;
use std::time::SystemTime;

use crate::DblError;
use crate::error::Result;
use crate::model::{ArchivedData, DeleteMode, Instance, InstanceId, InstanceStatus};
use crate::service::{OperationId, OperationStage};

use super::helpers::{
    OpResult, blocking, db_async, dir_size, emit_changed, emit_stage, finish_operation,
    identity_of, load, remove_tree, safe_within,
};
use super::service::Inner;

/// How many times a delete re-reads the status before giving up, which absorbs
/// a supervisor poll landing between the read and the compare-and-set.
const CLAIM_ATTEMPTS: usize = 4;

pub(crate) fn delete_instance(
    inner: &Arc<Inner>,
    id: &InstanceId,
    mode: DeleteMode,
) -> Result<OperationId> {
    let instance = load(inner, id)?;
    // `Deleting` is terminal, so a second delete must not try to move it again.
    if instance.status().is_terminal() {
        return Err(DblError::InvalidTransition {
            from: instance.status().as_str(),
            to: "deleting",
        });
    }
    let lock = inner.ops.lock_instance(id)?;
    let must_stop = instance.status().holds_process();

    let op = inner.ops.next_id();
    let op_id = op.clone();
    let inner = Arc::clone(inner);
    let id = id.clone();
    inner.handle().spawn(async move {
        let _lock = lock;
        emit_stage(&inner, &op, OperationStage::Queued);
        let result = run_delete(Arc::clone(&inner), &op, &id, mode, must_stop).await;
        finish_operation(&inner, &op, result);
    });
    Ok(op_id)
}

async fn run_delete(
    inner: Arc<Inner>,
    _op: &OperationId,
    id: &InstanceId,
    mode: DeleteMode,
    must_stop: bool,
) -> OpResult<()> {
    let instance = load(&inner, id)?;

    if must_stop {
        stop_for_delete(&inner, &instance).await?;
    }

    // Move to `Deleting` with a compare-and-set, which is the cross-process
    // lock: a second delete loses with a typed "busy" error instead of acting
    // twice. The status is re-read on each attempt because stopping the engine
    // can legitimately move the row to `Stopped` or, if the supervisor noticed
    // the exit first, to `Failed` — both are valid sources for a delete.
    let mut claimed = false;
    for _ in 0..CLAIM_ATTEMPTS {
        let current = load(&inner, id)?.status();
        if current.is_terminal() {
            return Err(DblError::InvalidTransition {
                from: current.as_str(),
                to: "deleting",
            }
            .into());
        }
        let id2 = id.clone();
        if db_async(&inner, move |store| {
            store.transition_status(&id2, current, InstanceStatus::Deleting)
        })
        .await?
        {
            claimed = true;
            break;
        }
    }
    if !claimed {
        return Err(DblError::InvalidTransition {
            from: "busy",
            to: "deleting",
        }
        .into());
    }
    emit_changed(&inner, id);
    // `OperationStage` has no delete-specific phase, and reusing `Starting`
    // would be a lie; a delete reports `Queued` then `Done` with the status
    // changes in between.
    let instance = load(&inner, id)?;
    let archive = match mode {
        DeleteMode::ArchiveData => Some(archive_data(&inner, &instance).await?),
        DeleteMode::PurgeData => {
            purge_data(&inner, &instance).await?;
            None
        }
    };
    if let Some(archive) = archive {
        let row = archive;
        db_async(&inner, move |store| store.add_archive(&row)).await?;
    }

    // The row goes only after the data is safely archived or gone, and the
    // secret is cleared from both backends before the cascade removes the
    // pointer that tells us where it lived.
    if let Some(reference) = inner.db(|store| store.get_credential(id))? {
        let _ = inner.secrets.delete(&reference);
    }
    let id2 = id.clone();
    let _ = db_async(&inner, move |store| {
        store.delete_instance(&id2).or_else(|err| match err {
            DblError::NotFound { .. } => Ok(()),
            other => Err(other),
        })
    })
    .await;
    inner.log_watchers.disable(id);
    emit_changed(&inner, id);
    Ok(())
}

/// Stops the engine and records `Stopped`, so the delete proceeds from a stable
/// status even if the supervisor raced it to `Failed`.
async fn stop_for_delete(inner: &Arc<Inner>, instance: &Instance) -> Result<()> {
    if let Some(identity) = identity_of(instance) {
        let terminator = Arc::clone(&inner.terminator);
        blocking(move || {
            terminator
                .terminate(
                    &identity,
                    std::time::Duration::from_secs(crate::consts::STOP_GRACE_SECS),
                )
                .map(|_| ())
        })
        .await?;
    }
    let id = instance.id().clone();
    db_async(inner, move |store| {
        store.update_process_identity(&id, None, None, None)?;
        // The delete itself writes `Deleting`; this only removes the stale
        // "running with a pid" shape a crash in between would leave behind.
        Ok(())
    })
    .await?;
    Ok(())
}

async fn purge_data(inner: &Arc<Inner>, instance: &Instance) -> Result<()> {
    let instance_dir = inner.paths.instance_dir(instance.name());
    // Re-validate before removing: a corrupt row must not delete outside home.
    safe_within(&inner.paths, &instance_dir)?;
    blocking(move || remove_tree(&instance_dir)).await
}

async fn archive_data(inner: &Arc<Inner>, instance: &Instance) -> OpResult<ArchivedData> {
    let data_dir = inner.paths.data_dir(instance.name());
    safe_within(&inner.paths, &data_dir)?;
    let instance_dir = inner.paths.instance_dir(instance.name());
    safe_within(&inner.paths, &instance_dir)?;

    let now = SystemTime::now();
    let stamp = archive_stamp(now);
    let target = inner
        .paths
        .archive_dir()
        .join(format!("{}-{stamp}", instance.name().as_str()));
    // The archive path is derived here, never from user text, and checked.
    safe_within(&inner.paths, &target)?;
    if !super::helpers::normalize(&target)
        .starts_with(super::helpers::normalize(&inner.paths.archive_dir()))
    {
        return Err(DblError::UnsafeArchive {
            entry: target.display().to_string(),
        }
        .into());
    }

    let size = dir_size(&data_dir);
    let record_id = archive_id(instance, stamp);
    let name = instance.name().clone();
    let engine = instance.engine();
    let version = instance.version().to_owned();
    let target_for_move = target.clone();
    blocking(move || {
        std::fs::create_dir_all(target_for_move.parent().unwrap_or(&target_for_move))?;
        if target_for_move.exists() {
            return Err(DblError::AlreadyExists {
                kind: "archive",
                name: target_for_move.display().to_string(),
            });
        }
        if data_dir.exists() {
            std::fs::rename(&data_dir, &target_for_move)?;
        } else {
            std::fs::create_dir_all(&target_for_move)?;
        }
        // The instance shell (logs, run) is gone once its data is archived.
        remove_tree(&instance_dir)
    })
    .await?;

    Ok(ArchivedData::new(
        record_id, name, engine, version, target, size, now,
    ))
}

fn archive_id(instance: &Instance, stamp: u64) -> String {
    let id = instance.id().as_str();
    let short = id.get(..8).unwrap_or(id);
    format!("arch-{short}-{stamp}")
}

fn archive_stamp(now: SystemTime) -> u64 {
    let elapsed = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    elapsed
        .as_secs()
        .saturating_mul(1000)
        .saturating_add(u64::from(elapsed.subsec_millis()))
}
