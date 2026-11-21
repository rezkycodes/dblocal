//! Deleting an instance (`F-INS-05`, D-18).

use std::sync::Arc;
use std::time::SystemTime;

use crate::DblError;
use crate::error::Result;
use crate::model::{ArchivedData, DeleteMode, Instance, InstanceId, InstanceStatus};
use crate::service::{OperationId, OperationStage};

use super::helpers::{
    OpResult, blocking, db_async, dir_size, emit_changed, emit_removed, emit_stage,
    finish_operation, identity_of, load, remove_tree, safe_within,
};
use super::service::Inner;

/// How many times a delete re-reads the status before giving up, which absorbs
/// a supervisor poll landing between the read and the compare-and-set.
const CLAIM_ATTEMPTS: usize = 4;

/// Name of the in-flight marker a live delete writes inside the instance
/// directory.
///
/// The row alone cannot distinguish "a delete is running" from "the app died
/// mid-delete", and both look like `Deleting`. The marker carries the owning
/// PID **and** a per-operation nonce, so a reader knows the difference:
///
/// - a different, live PID means another process is deleting it;
/// - a nonce registered in this process's [`super::registry::Ops`] means a
///   sibling service in the same process is deleting it;
/// - anything else (dead PID, unknown nonce) is stale, i.e. interrupted.
pub(crate) const IN_FLIGHT_MARKER: &str = "delete.inflight";

/// Nonces of deletes running in **this process**, keyed by nothing: membership is
/// all that matters.
///
/// A process-wide registry, not a per-service one, because two `LocalService`
/// instances in one process (each with its own `Ops`) can share a home, and each
/// must see the other's in-flight delete.
fn process_deletes() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static DELETES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    DELETES.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

fn owns_nonce(nonce: &str) -> bool {
    process_deletes()
        .lock()
        .map(|set| set.contains(nonce))
        .unwrap_or(false)
}

fn register_nonce(nonce: &str) {
    if let Ok(mut set) = process_deletes().lock() {
        set.insert(nonce.to_owned());
    }
}

fn release_nonce(nonce: &str) {
    if let Ok(mut set) = process_deletes().lock() {
        set.remove(nonce);
    }
}

/// Whether a delete currently owns this row.
///
/// Both the cross-process and the in-process case are covered, because the GUI
/// and a CLI can share one home and one process can host two services.
pub(crate) fn delete_in_flight(inner: &Arc<Inner>, instance: &Instance) -> bool {
    let marker = instance_marker(inner, instance);
    let Ok(contents) = std::fs::read_to_string(&marker) else {
        return false;
    };
    let mut parts = contents.split_whitespace();
    let pid = parts.next().and_then(|value| value.parse::<u32>().ok());
    let nonce = parts.next().unwrap_or_default();

    if nonce.is_empty() {
        return false;
    }
    if owns_nonce(nonce) {
        return true;
    }
    let Some(pid) = pid else {
        return false;
    };
    if pid == std::process::id() {
        // Our own process, but no delete here claims the nonce: a leftover from a
        // panic. Stale.
        return false;
    }
    matches!(
        inner.probe.observe(pid),
        crate::process::probe::ProcessObservation::Found { .. }
    )
}

fn instance_marker(inner: &Inner, instance: &Instance) -> std::path::PathBuf {
    inner
        .paths
        .instance_dir(instance.name())
        .join(IN_FLIGHT_MARKER)
}

/// A marker that removes its file and its nonce on drop, so a normal exit leaves
/// nothing behind and only a crash leaves a stale file reconciliation ignores.
struct InFlightMarker {
    path: std::path::PathBuf,
    nonce: String,
}

impl InFlightMarker {
    fn create(dir: &std::path::Path, nonce: &str) -> Option<Self> {
        std::fs::create_dir_all(dir).ok()?;
        let path = dir.join(IN_FLIGHT_MARKER);
        std::fs::write(&path, format!("{} {nonce}", std::process::id())).ok()?;
        register_nonce(nonce);
        Some(Self {
            path,
            nonce: nonce.to_owned(),
        })
    }
}

impl Drop for InFlightMarker {
    fn drop(&mut self) {
        // Remove the file only while it still names this operation: a delete that
        // lost the claim, or one that finished after a later delete took over,
        // must not erase the owner's marker.
        let ours = std::fs::read_to_string(&self.path)
            .map(|contents| contents.split_whitespace().nth(1) == Some(self.nonce.as_str()))
            .unwrap_or(false);
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
        release_nonce(&self.nonce);
    }
}

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

    // Write the in-flight marker **before** the status becomes observable. A
    // reader that sees `Deleting` must be able to see "a delete is running", so
    // the order is marker first, claim second; writing it after the compare-and-
    // set leaves a window in which another process's reconciliation sees a
    // `Deleting` row with no owner and mistakes it for a crash.
    let nonce = format!("{}-{}", std::process::id(), id.as_str());
    let marker = InFlightMarker::create(&inner.paths.instance_dir(instance.name()), &nonce);

    // Claim `Deleting` synchronously, like `start_instance` claims `Starting`.
    // The compare-and-set is the cross-process lock, so a second service (or a
    // second call) is refused right here with a typed busy error instead of
    // being told an operation started and failing later. The status is re-read
    // on each attempt because stopping the engine can legitimately move the row
    // to `Stopped` or, if the supervisor noticed the exit first, to `Failed` —
    // both are valid sources for a delete.
    let mut claimed = false;
    for _ in 0..CLAIM_ATTEMPTS {
        let current = load(inner, id)?.status();
        if current.is_terminal() {
            return Err(DblError::InvalidTransition {
                from: current.as_str(),
                to: "deleting",
            });
        }
        let won =
            inner.db(|store| store.transition_status(id, current, InstanceStatus::Deleting))?;
        if won {
            claimed = true;
            break;
        }
    }
    if !claimed {
        // Lost the race: drop our marker only if it is still ours, so we never
        // remove the winner's.
        drop(marker);
        return Err(DblError::InvalidTransition {
            from: "busy",
            to: "deleting",
        });
    }
    // `OperationStage` has no delete-specific phase, and reusing `Starting`
    // would be a lie; a delete reports `Queued` then `Done` with the status
    // changes in between.
    emit_changed(inner, id);

    let op = inner.ops.next_id();
    let op_id = op.clone();
    let inner = Arc::clone(inner);
    let id = id.clone();
    inner.handle().spawn(async move {
        let lock = lock;
        let marker = marker;
        emit_stage(&inner, &op, OperationStage::Queued);
        let result = run_delete(Arc::clone(&inner), &id, mode, must_stop).await;
        // Drop the marker first, then the lock, then announce completion, so a
        // caller reacting to `OperationFinished` sees a fully released instance.
        drop(marker);
        drop(lock);
        finish_operation(&inner, &op, result);
    });
    Ok(op_id)
}

async fn run_delete(
    inner: Arc<Inner>,
    id: &InstanceId,
    mode: DeleteMode,
    must_stop: bool,
) -> OpResult<()> {
    let instance = load(&inner, id)?;

    if must_stop {
        stop_for_delete(&inner, &instance).await?;
    }

    let archive = match mode {
        DeleteMode::ArchiveData => Some(archive_data(&inner, &instance).await?),
        DeleteMode::PurgeData => {
            purge_data(&inner, &instance).await?;
            None
        }
    };
    if let Some(archive) = archive {
        let row = archive;
        // `AlreadyExists` means another writer (a second process's
        // reconciliation) registered the same archive first. The data is
        // recorded either way, so that is success, not an error.
        db_async(&inner, move |store| match store.add_archive(&row) {
            Ok(()) => Ok(()),
            Err(DblError::AlreadyExists { .. }) => Ok(()),
            Err(err) => Err(err),
        })
        .await?;
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
    // The row is gone: `InstanceChanged` cannot say so, `InstanceRemoved` can.
    emit_removed(&inner, id);
    Ok(())
}

/// Stops the engine before the row is removed.
///
/// A process that survives the attempt aborts the delete: removing the row while
/// the engine still runs would orphan it with no identity left to stop it, which
/// is worse than a delete that fails cleanly and can be retried.
async fn stop_for_delete(inner: &Arc<Inner>, instance: &Instance) -> OpResult<()> {
    let identity = identity_of(instance);
    let had_identity = identity.is_some();
    let terminated = super::helpers::terminate_verified(
        inner,
        identity,
        std::time::Duration::from_secs(crate::consts::STOP_GRACE_SECS),
    )
    .await;

    if had_identity && terminated.state == super::helpers::Stopped::StillAlive {
        return Err(crate::DblError::Engine {
            reason: format!(
                "the engine for {} is still running; refusing to delete its row",
                instance.id()
            ),
        }
        .into());
    }

    if terminated.forced {
        super::helpers::emit_stop_forced(inner, instance.id());
    }

    // The delete writes `Deleting` next; clearing the identity here only removes
    // the stale "running with a pid" shape a crash in between would leave.
    let id = instance.id().clone();
    db_async(inner, move |store| {
        store.update_process_identity(&id, None, None, None)?;
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
        if data_dir.exists() {
            if target_for_move.exists() {
                // Another process already moved this data (it crashed after the
                // rename). Keep the data where it is rather than refusing.
                tracing::warn!(
                    path = %target_for_move.display(),
                    "archive target already exists; keeping the existing copy"
                );
            } else {
                std::fs::rename(&data_dir, &target_for_move)?;
            }
        } else if !target_for_move.exists() {
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

fn archive_id(instance: &Instance, _stamp: u64) -> String {
    // Deliberately stable, not time-derived: an instance is deleted once, and a
    // stable id makes registering the archive idempotent. That matters because a
    // second process's reconciliation can find the renamed directory in the
    // window before this delete records it, and both writers must land on the
    // same row instead of producing two archive entries.
    let id = instance.id().as_str();
    let short = id.get(..8).unwrap_or(id);
    format!("arch-{short}")
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
