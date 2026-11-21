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
    let marker = instance_marker(inner, instance);
    let Ok(contents) = std::fs::read_to_string(&marker) else {
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

fn instance_marker(inner: &Inner, instance: &Instance) -> std::path::PathBuf {
    inner
        .paths
        .instance_dir(instance.name())
        .join(IN_FLIGHT_MARKER)
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
    fn create(inner: &Arc<Inner>, dir: &std::path::Path, nonce: &str) -> Option<Self> {
        std::fs::create_dir_all(dir).ok()?;
        let path = dir.join(IN_FLIGHT_MARKER);
        let identity = own_identity(inner)?;
        let owner = MarkerOwner {
            pid: identity.pid(),
            started_at: identity.started_at(),
            exe: identity.exe().to_path_buf(),
            nonce: nonce.to_owned(),
        };
        std::fs::write(&path, owner.render()).ok()?;
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
    let nonce = format!("{}-{}", std::process::id(), id.as_str());
    let marker = InFlightMarker::create(inner, &inner.paths.instance_dir(instance.name()), &nonce);

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
