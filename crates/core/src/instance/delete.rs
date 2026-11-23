//! Deleting an instance (`F-INS-05`, D-18).

use std::sync::Arc;
use std::time::SystemTime;

use crate::DblError;
use crate::error::Result;
use crate::model::{ArchivedData, DeleteMode, Instance, InstanceId, InstanceStatus};
use crate::service::{OperationId, OperationStage};

use super::helpers::{
    OpResult, blocking, db_async, dir_size, emit_changed, emit_removed, emit_stage,
    finish_operation, identity_of, load, safe_within,
};
use super::retry::remove_tree_retrying;
use super::service::Inner;

/// How many times a delete re-reads the status before giving up, which absorbs
/// a supervisor poll landing between the read and the compare-and-set.
const CLAIM_ATTEMPTS: usize = 4;

/// File-name suffix of the in-flight marker a live delete writes.
///
/// The row alone cannot distinguish "a delete is running" from "the app died
/// mid-delete", and both look like `Deleting`. The marker carries the owning
/// process's identity — PID, start time and executable — plus a per-operation
/// nonce, so a reader knows the difference:
///
/// - a process that still validates as the recorded identity owns the delete;
/// - a nonce registered in this process means a sibling service here owns it;
/// - anything else (dead process, a reused PID with a different start time, an
///   unknown nonce) is stale, i.e. interrupted.
///
/// Trusting a bare PID is exactly what AGENTS.md invariant 7 forbids: after a
/// reboot an unrelated process can hold the same number, and a marker keyed on
/// the number alone would keep a half-deleted row unrecoverable forever.
///
/// The marker is **not** inside the instance directory. A delete removes that
/// directory (data, logs, shell) before it removes the row, so a marker living
/// there would be destroyed by the very operation it is supposed to announce:
/// a sibling service reconciling at that instant would see `Deleting` with no
/// owner, take the row over and remove it while the delete is still running
/// (flake 8: `operation start: NotFound` for the re-inserted row). It lives
/// beside the instance directories, keyed by the instance **id**, whose UUID
/// form can never collide with an instance name (`^[a-z0-9][a-z0-9-]{0,30}$`).
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

/// A process-wide counter for delete nonces.
///
/// `Ops::next_id` is **per service**, so two `LocalService`s in one process (each
/// with its own `Ops`) both hand out `op-1` for their first delete; a nonce built
/// from the operation id alone would then be identical across services, and the
/// loser's marker `Drop` would remove the winner's marker. This counter is shared
/// by the whole process, so every delete gets a distinct nonce regardless of which
/// service issued it.
fn next_delete_sequence() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// Builds the in-flight marker nonce for one delete.
///
/// `sequence` is the process-wide [`next_delete_sequence`] value and `op_id` the
/// issuing service's operation id. The sequence is what makes the nonce distinct
/// across services: two services can both be at `op-1`, but never at the same
/// sequence value.
fn delete_nonce(sequence: u64, op_id: &str) -> String {
    format!("{}-{}-{}", std::process::id(), sequence, op_id)
}

/// The owner recorded by an in-flight marker.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkerOwner {
    pid: u32,
    started_at: std::time::SystemTime,
    exe: std::path::PathBuf,
    nonce: String,
}

impl MarkerOwner {
    /// Serialises the owner as `key=value` lines.
    ///
    /// Lines, not space-separated fields, because an executable path can contain
    /// spaces and a naive split would corrupt it.
    fn render(&self) -> String {
        let started = self
            .started_at
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| format!("{}.{}", elapsed.as_secs(), elapsed.subsec_nanos()))
            .unwrap_or_else(|_| "0.0".to_owned());
        format!(
            "pid={}\nstarted={started}\nexe={}\nnonce={}\n",
            self.pid,
            self.exe.display(),
            self.nonce
        )
    }

    fn parse(contents: &str) -> Option<Self> {
        let mut pid = None;
        let mut started_at = None;
        let mut exe = None;
        let mut nonce = None;
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "pid" => pid = value.trim().parse::<u32>().ok(),
                "started" => {
                    let (secs, nanos) = value.split_once('.').unwrap_or((value, "0"));
                    started_at = secs.parse::<u64>().ok().map(|secs| {
                        std::time::UNIX_EPOCH
                            + std::time::Duration::new(
                                secs,
                                nanos.parse::<u32>().unwrap_or_default(),
                            )
                    });
                }
                "exe" => exe = Some(std::path::PathBuf::from(value)),
                "nonce" => nonce = Some(value.to_owned()),
                _ => {}
            }
        }
        Some(Self {
            pid: pid?,
            started_at: started_at?,
            exe: exe?,
            nonce: nonce?,
        })
    }

    /// The identity the process probe validates against.
    fn identity(&self) -> Option<crate::process::identity::ProcessIdentity> {
        crate::process::identity::ProcessIdentity::new(self.pid, self.exe.clone(), self.started_at)
            .ok()
    }
}

/// Whether a delete currently owns this row.
///
/// Both the cross-process and the in-process case are covered, because the GUI
/// and a CLI can share one home and one process can host two services.
pub(crate) fn delete_in_flight(inner: &Arc<Inner>, instance: &Instance) -> bool {
    let Ok(contents) = std::fs::read_to_string(marker_path(&inner.paths, instance.id())) else {
        return false;
    };
    let Some(owner) = MarkerOwner::parse(&contents) else {
        return false;
    };

    if owns_nonce(&owner.nonce) {
        return true;
    }
    // Validate the full identity, not just the PID: after a reboot an unrelated
    // process can hold the same number, and only a matching start time and
    // executable prove it is the delete that wrote this marker.
    let Some(identity) = owner.identity() else {
        return false;
    };
    inner.probe.validate(&identity) == crate::process::identity::ProcessState::Match
}

/// Where a live delete announces itself.
///
/// Beside `instances/<name>/`, not inside it, because the delete removes that
/// directory before it removes the row (see [`IN_FLIGHT_MARKER`]). The file is a
/// dotfile keyed by the instance **id**, so it can never be mistaken for an
/// instance directory (a name must start with `[a-z0-9]`) and a purge or archive
/// of the instance cannot take it away from the delete that owns it.
pub(crate) fn marker_path(paths: &crate::paths::DblPaths, id: &InstanceId) -> std::path::PathBuf {
    paths
        .instances_dir()
        .join(format!(".{}.{IN_FLIGHT_MARKER}", id.as_str()))
}

/// Removes the marker of a delete that is known to be over.
///
/// A delete's own `Drop` removes its file; this covers the other exit: a process
/// that crashed mid-delete leaves the file behind, and reconciliation is what
/// decides that delete is finished. Without this the home would keep one stale
/// dotfile per crashed delete forever.
pub(crate) fn clear_in_flight_marker(paths: &crate::paths::DblPaths, id: &InstanceId) {
    let _ = std::fs::remove_file(marker_path(paths, id));
}

/// The identity of this process, read from the same probe every other identity
/// check uses, so the marker records the kernel's start time and not a guess.
fn own_identity(inner: &Inner) -> Option<crate::process::identity::ProcessIdentity> {
    let pid = std::process::id();
    let exe = std::env::current_exe().ok()?;
    match inner.probe.observe(pid) {
        crate::process::probe::ProcessObservation::Found {
            started_at: Some(started_at),
            ..
        } => crate::process::identity::ProcessIdentity::new(pid, exe, started_at).ok(),
        // The probe cannot see this process (a scripted probe in a test, or a
        // hardened kernel). Fall back to our own executable and the current
        // instant: a cross-process reader then validates on executable plus a
        // start-time tolerance, and an in-process sibling is still caught by the
        // nonce.
        _ => crate::process::identity::ProcessIdentity::new(pid, exe, std::time::SystemTime::now())
            .ok(),
    }
}

/// A marker that removes its file and its nonce on drop, so a normal exit leaves
/// nothing behind and only a crash leaves a stale file reconciliation ignores.
struct InFlightMarker {
    path: std::path::PathBuf,
    nonce: String,
}

impl InFlightMarker {
    fn create(inner: &Arc<Inner>, id: &InstanceId, nonce: &str) -> Option<Self> {
        let path = marker_path(&inner.paths, id);
        let identity = own_identity(inner)?;
        let owner = MarkerOwner {
            pid: identity.pid(),
            started_at: identity.started_at(),
            exe: identity.exe().to_path_buf(),
            nonce: nonce.to_owned(),
        };
        // Write beside the instance directories, creating the parent if this is
        // a fresh home; never validate through the instance name (the directory
        // may already be gone when a delete writes or replaces its marker).
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        std::fs::write(&path, owner.render()).ok()?;
        register_nonce(nonce);
        // Record the nonce the production code actually wrote, so a test observes the
        // real call-site value (not a helper's) and catches a call-site mutation.
        #[cfg(test)]
        record_created_nonce(&path.display().to_string(), nonce);
        Some(Self {
            path,
            nonce: nonce.to_owned(),
        })
    }
}

/// Marker nonces written by [`InFlightMarker::create`], keyed by the marker file
/// they were written to, for the item-5(b) test.
#[cfg(test)]
fn created_nonce_log() -> &'static std::sync::Mutex<Vec<(String, String)>> {
    static LOG: std::sync::OnceLock<std::sync::Mutex<Vec<(String, String)>>> =
        std::sync::OnceLock::new();
    LOG.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

#[cfg(test)]
fn record_created_nonce(marker: &str, nonce: &str) {
    if let Ok(mut log) = created_nonce_log().lock() {
        log.push((marker.to_owned(), nonce.to_owned()));
    }
}

/// Every marker nonce [`InFlightMarker::create`] wrote to `marker`, in order.
#[cfg(test)]
pub(crate) fn take_created_nonces(marker: &std::path::Path) -> Vec<String> {
    let marker = marker.display().to_string();
    let Ok(mut log) = created_nonce_log().lock() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    log.retain(|(seen, nonce)| {
        if seen == &marker {
            out.push(nonce.clone());
            false
        } else {
            true
        }
    });
    out
}

impl Drop for InFlightMarker {
    fn drop(&mut self) {
        // Remove the file only while it still names this operation: a delete that
        // lost the claim, or one that finished after a later delete took over,
        // must not erase the owner's marker.
        let ours = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|contents| MarkerOwner::parse(&contents))
            .is_some_and(|owner| owner.nonce == self.nonce);
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
    //
    // The nonce must be unique per delete *process-wide*. It is not enough to build
    // it from the operation id: two `LocalService`s in one process each have their
    // own `Ops`, so both hand out `op-1` for their first delete, the nonces match,
    // and the loser's marker `Drop` would remove the winner's marker. A process-wide
    // sequence guarantees a distinct nonce for every delete regardless of service.
    let op_id = inner.ops.next_id();
    let nonce = delete_nonce(next_delete_sequence(), op_id.as_str());
    let marker = InFlightMarker::create(inner, id, &nonce);

    // Claim `Deleting` synchronously, like `start_instance` claims `Starting`.
    // The compare-and-set is the cross-process lock, so a second service (or a
    // second call) is refused right here with a typed busy error instead of
    // being told an operation started and failing later. The status is re-read
    // on each attempt because stopping the engine can legitimately move the row
    // to `Stopped` or, if the supervisor noticed the exit first, to `Failed` —
    // both are valid sources for a delete.
    let mut claimed = false;
    for _ in 0..CLAIM_ATTEMPTS {
        // Claim from the status observed at the top of *this* attempt. Claiming
        // from that observation (not from a fresh read) is what makes the CAS the
        // lock: a competing writer that moved the row between our read and the
        // write makes this attempt lose, and the next attempt re-reads.
        let current = load(inner, id)?.status();
        if current.is_terminal() {
            return Err(DblError::InvalidTransition {
                from: current.as_str(),
                to: "deleting",
            });
        }
        if super::helpers::claim_status(inner, id, current, InstanceStatus::Deleting)? {
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

    let op = op_id;
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

/// Test-only seam fired by [`run_delete`] once the data is gone but the row is
/// still there, i.e. exactly the window in which the old marker lived inside a
/// directory that no longer exists.
///
/// It exists so a test can park a real delete at that instant, start a second
/// service over the same home and prove reconciliation does not take the row
/// over. The hook is one-shot, like the reconcile hooks.
#[cfg(test)]
fn data_gone_hook() -> &'static std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>> {
    static HOOK: std::sync::OnceLock<std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>> =
        std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(None))
}

/// Registers a one-shot hook fired when a delete has removed the data and not
/// yet the row.
#[cfg(test)]
pub(crate) fn on_data_gone(hook: impl FnOnce() + Send + 'static) {
    if let Ok(mut slot) = data_gone_hook().lock() {
        *slot = Some(Box::new(hook));
    }
}

#[cfg(test)]
fn run_data_gone() {
    if let Some(hook) = data_gone_hook().lock().ok().and_then(|mut slot| slot.take()) {
        hook();
    }
}

#[cfg(not(test))]
fn run_data_gone() {}

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
    // The instance directory (with its data) is gone; the row still says
    // `Deleting`. A sibling service reconciling now must still see this delete
    // as in flight, which is what the marker outside that directory is for.
    run_data_gone();
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
    blocking(move || remove_tree_retrying(&instance_dir)).await
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
    let archive_root = inner.paths.archive_dir();
    blocking(move || {
        super::helpers::ensure_private_dir(&archive_root)?;
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
            super::helpers::ensure_private_dir(&target_for_move)?;
        }
        // The instance shell (logs, run) is gone once its data is archived.
        remove_tree_retrying(&instance_dir)
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn owner(nonce: &str) -> String {
        format!("pid=1\nstarted=1.0\nexe=/x\nnonce={nonce}\n")
    }

    /// The loser's marker `Drop` must not remove the winner's marker: a marker whose
    /// recorded nonce differs from the file's is somebody else's and is left alone.
    ///
    /// This is the property the per-operation nonce exists for; without distinct
    /// nonces the loser's `Drop` deletes the winner's in-flight file and a concurrent
    /// reconciliation then mistakes a live delete for a crash.
    #[test]
    fn a_marker_drop_leaves_another_writers_file_alone() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join(IN_FLIGHT_MARKER);
        // The winner has written its marker.
        std::fs::write(&path, owner("winner-nonce")).expect("write");

        // The loser's guard names a different nonce; dropping it must not remove the
        // file.
        drop(InFlightMarker {
            path: path.clone(),
            nonce: "loser-nonce".to_owned(),
        });
        assert!(
            path.exists(),
            "the loser must not remove the winner's marker"
        );

        // The owner's own drop does remove it.
        drop(InFlightMarker {
            path: path.clone(),
            nonce: "winner-nonce".to_owned(),
        });
        assert!(!path.exists(), "the owner removes its own marker");
    }

    /// The process-wide sequence gives every delete a distinct nonce even though
    /// `Ops::next_id` is per service.
    #[test]
    fn delete_sequence_is_unique_process_wide() {
        let first = next_delete_sequence();
        let second = next_delete_sequence();
        assert_ne!(first, second, "the process-wide sequence never repeats");
    }

    /// Item 5(b): two services in one process can both be at `op-1` for their first
    /// delete (`Ops::next_id` is per service). The nonce must still differ, or the
    /// loser's marker `Drop` removes the winner's marker.
    ///
    /// This pins the exact collision without racing two threads: the same `op_id`
    /// with two different process-wide sequences must yield two different nonces.
    /// Reverting to `{pid}-{op_id}` (or any per-(process,instance) shape) makes them
    /// equal and fails here.
    #[test]
    fn a_nonce_differs_for_two_services_both_at_op_1() {
        let a = delete_nonce(next_delete_sequence(), "op-1");
        let b = delete_nonce(next_delete_sequence(), "op-1");
        assert_ne!(
            a, b,
            "two deletes with the same per-service op id must still get distinct nonces: {a:?} / {b:?}"
        );
    }
}
