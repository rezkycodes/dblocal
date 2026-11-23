//! Startup reconciliation (`F-INS-04`, D-02).
//!
//! Engines outlive the app, so a pidfile from the previous session either names
//! a live process that is still ours (adopt as `Running`) or a dead/reused PID
//! (mark `Stopped` and clear the identity). Nothing is signalled here.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use crate::DblError;
use crate::model::{ArchivedData, Instance, InstanceEvent, InstanceId, InstanceStatus};
use crate::process::reconcile::{ReconcileRecord, reconcile};

use super::helpers::{db_async, emit_changed, identity_of};
use super::service::Inner;

/// Test-only hooks around one reconciliation pass, keyed by state root, so the
/// "the create finished while we were listing" interleaving is forced
/// deterministically instead of waiting for it under load.
#[cfg(test)]
type ListHook = Box<dyn FnOnce(&Arc<Inner>) + Send>;

/// Runs between the status write and the identity clear of a `mark`, so a test can
/// observe a fresh connection at exactly that instant.
#[cfg(test)]
type AfterStatusHook = Box<dyn FnOnce(&Arc<Inner>, &InstanceId) + Send>;

/// Runs in phase 3 while the instance guard is held (after the fresh read, before
/// the probe), so a test can issue a user command into that window.
#[cfg(test)]
type GuardedHook = Box<dyn FnOnce(&Arc<Inner>, &InstanceId) + Send>;

#[cfg(test)]
#[derive(Default)]
struct Hooks {
    before_list: Option<ListHook>,
    after_list: Option<ListHook>,
    after_status: Option<AfterStatusHook>,
    guarded: Option<GuardedHook>,
    done: Option<std::sync::mpsc::Sender<()>>,
}

#[cfg(test)]
fn hook_table() -> &'static std::sync::Mutex<Vec<(String, Hooks)>> {
    static HOOKS: std::sync::OnceLock<std::sync::Mutex<Vec<(String, Hooks)>>> =
        std::sync::OnceLock::new();
    HOOKS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Runs `edit` on the hooks registered for `root`, creating the entry if needed.
#[cfg(test)]
fn edit_hooks(root: &str, edit: impl FnOnce(&mut Hooks)) {
    let mut table = hook_table().lock().expect("hooks");
    if !table.iter().any(|(seen, _)| seen == root) {
        table.push((root.to_owned(), Hooks::default()));
    }
    if let Some((_, hooks)) = table.iter_mut().find(|(seen, _)| seen == root) {
        edit(hooks);
    }
}

/// Registers `before` to run just before reconciliation lists the rows and
/// `after` to run just after, for the service rooted at `root`.
#[cfg(test)]
pub(crate) fn on_reconcile(
    root: &std::path::Path,
    before: impl FnOnce(&Arc<Inner>) + Send + 'static,
    after: impl FnOnce(&Arc<Inner>) + Send + 'static,
) {
    edit_hooks(&root.display().to_string(), |hooks| {
        hooks.before_list = Some(Box::new(before));
        hooks.after_list = Some(Box::new(after));
    });
}

/// Asks to be told when reconciliation for `root` finishes.
#[cfg(test)]
pub(crate) fn on_reconcile_done(root: &std::path::Path) -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    edit_hooks(&root.display().to_string(), |hooks| hooks.done = Some(tx));
    rx
}

#[cfg(test)]
fn run_before_list(inner: &Arc<Inner>) {
    let root = inner.paths.root().display().to_string();
    let hook = edit_hooks_capture(&root, |hooks| hooks.before_list.take());
    if let Some(hook) = hook {
        hook(inner);
    }
}

#[cfg(test)]
fn run_after_list(inner: &Arc<Inner>) {
    let root = inner.paths.root().display().to_string();
    let hook = edit_hooks_capture(&root, |hooks| hooks.after_list.take());
    if let Some(hook) = hook {
        hook(inner);
    }
}

#[cfg(test)]
fn fire_done(inner: &Arc<Inner>) {
    let root = inner.paths.root().display().to_string();
    let sender = edit_hooks_capture(&root, |hooks| hooks.done.take());
    if let Some(sender) = sender {
        let _ = sender.send(());
    }
}

/// Registers a hook that runs between `mark`'s status write and its identity clear.
#[cfg(test)]
pub(crate) fn on_after_status(
    root: &std::path::Path,
    hook: impl FnOnce(&Arc<Inner>, &InstanceId) + Send + 'static,
) {
    edit_hooks(&root.display().to_string(), |hooks| {
        hooks.after_status = Some(Box::new(hook));
    });
}

/// Registers a hook that runs in phase 3 while the instance guard is held.
#[cfg(test)]
pub(crate) fn on_guard_held(
    root: &std::path::Path,
    hook: impl FnOnce(&Arc<Inner>, &InstanceId) + Send + 'static,
) {
    edit_hooks(&root.display().to_string(), |hooks| {
        hooks.guarded = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_guarded(inner: &Arc<Inner>, id: &InstanceId) {
    let root = inner.paths.root().display().to_string();
    if let Some(hook) = edit_hooks_capture(&root, |hooks| hooks.guarded.take()) {
        hook(inner, id);
    }
}

#[cfg(not(test))]
#[allow(dead_code)]
fn run_guarded(_: &Arc<Inner>, _: &InstanceId) {}

#[cfg(test)]
fn run_after_status(inner: &Arc<Inner>, id: &InstanceId) -> bool {
    let root = inner.paths.root().display().to_string();
    match edit_hooks_capture(&root, |hooks| hooks.after_status.take()) {
        Some(hook) => {
            hook(inner, id);
            true
        }
        None => false,
    }
}

/// Runs `take` on the hooks for `root` and returns the value it removed.
#[cfg(test)]
fn edit_hooks_capture<T>(root: &str, take: impl FnOnce(&mut Hooks) -> Option<T>) -> Option<T> {
    let mut table = hook_table().lock().ok()?;
    let (_, hooks) = table.iter_mut().find(|(seen, _)| seen == root)?;
    take(hooks)
}

#[cfg(not(test))]
fn run_before_list(_: &Arc<Inner>) {}

#[cfg(not(test))]
fn run_after_list(_: &Arc<Inner>) {}

#[cfg(not(test))]
#[allow(dead_code)]
fn run_after_status(_: &Arc<Inner>, _: &InstanceId) -> bool {
    false
}

#[cfg(not(test))]
fn fire_done(_: &Arc<Inner>) {}

pub(crate) async fn reconcile_on_startup(inner: Arc<Inner>) {
    run_before_list(&inner);
    let instances = match db_async(&inner, |store| store.list_instances()).await {
        Ok(instances) => instances,
        Err(err) => {
            tracing::warn!(%err, "reconciliation could not list instances");
            return;
        }
    };
    run_after_list(&inner);

    // Act on a **freshly read** row under the in-process instance lock, never on
    // the startup snapshot alone. Listing the rows is an `await` point: a create
    // can insert `Initializing`, finish and release its lock before
    // reconciliation acts. Treating the stale `Initializing` snapshot row as an
    // interrupted create then ran the failed-create rollback and deleted a
    // successfully created instance. The instance lock is what closes the race:
    // a create (or a delete) holds it for its whole life, so acquiring it proves
    // no such operation is running here, and the fresh read proves the row is
    // still mid-create rather than already settled.
    for instance in &instances {
        // A `Deleting` row is terminal and must be finished — but only when the
        // delete is genuinely interrupted. The GUI and a CLI share this home, so a
        // live delete in another process has written `Deleting` and is between its
        // filesystem steps; a service starting in that window must not mistake it
        // for a crash and revert it to `Stopped` (which would let a second delete
        // run). The owner records its PID in a marker file while it works, and the
        // row counts as in-flight only while that PID is still alive. A crashed
        // owner leaves the same file, but its PID is gone, so recovery proceeds.
        if instance.status() == InstanceStatus::Deleting {
            let Some(_guard) = lock_if_idle(&inner, instance.id()) else {
                continue;
            };
            let Some(fresh) = fresh_instance(&inner, instance.id()) else {
                continue;
            };
            if fresh.status() == InstanceStatus::Deleting
                && !super::delete::delete_in_flight(&inner, &fresh)
            {
                finish_delete(&inner, &fresh).await;
            }
        }
    }

    // Finish interrupted creates by running the same cleanup a failed create
    // runs: a half-initialised data directory and a possible credential must not
    // survive as a "failed" zombie (`F-INS-01`: nothing half-created, ever).
    for instance in &instances {
        if !matches!(
            instance.status(),
            InstanceStatus::Provisioning | InstanceStatus::Initializing
        ) {
            continue;
        }
        let Some(_guard) = lock_if_idle(&inner, instance.id()) else {
            continue;
        };
        let Some(fresh) = fresh_instance(&inner, instance.id()) else {
            continue;
        };
        if matches!(
            fresh.status(),
            InstanceStatus::Provisioning | InstanceStatus::Initializing
        ) {
            clean_interrupted_create(&inner, &fresh).await;
        }
    }

    // Third phase: rows that hold a process. Like the other two phases, take the
    // instance lock and act on a **fresh** row, never on the snapshot: an
    // in-process start or stop can own the row while it is still `Starting` or
    // `Stopping`, and adopting or marking the stale snapshot would overwrite that
    // operation's claim (a `Stopping` row adopted as `Running` lets a second stop
    // be accepted; a `Starting` row adopted as `Running` claims readiness the start
    // has not reached, and one marked absent is written `Stopped` under a live
    // start). The guards are held across the probe and the persist: an operation
    // arriving meanwhile is refused as busy, which is the right answer while
    // reconciliation is deciding.
    let mut guarded: Vec<(super::registry::InstanceLock, Instance)> = Vec::new();
    for instance in &instances {
        if !instance.status().holds_process() {
            continue;
        }
        let Some(guard) = lock_if_idle(&inner, instance.id()) else {
            // An operation owns this row in this process; it settles it itself.
            continue;
        };
        let Some(fresh) = fresh_instance(&inner, instance.id()) else {
            continue;
        };
        if !fresh.status().holds_process() {
            continue;
        }
        // A `Starting` row with no recorded identity is a spawn in flight, not
        // an orphan: no PID has been persisted yet, and the process that is
        // starting it (possibly the other service sharing this home) has not
        // finished. Writing it `Stopped` here is what let a second service
        // clobber a live start. The owner finishes it, and a crashed one is
        // retaken by the next `start_instance`.
        if fresh.status() == InstanceStatus::Starting && identity_of(&fresh).is_none() {
            continue;
        }
        // Test seam: run a hook here, with the guard held, to issue a user command
        // into the window a one-shot CLI lands in.
        #[cfg(test)]
        run_guarded(&inner, instance.id());
        guarded.push((guard, fresh));
    }
    if guarded.is_empty() {
        fire_done(&inner);
        return;
    }

    let records: Vec<ReconcileRecord> = guarded
        .iter()
        .map(|(_, instance)| ReconcileRecord::new(instance.id().clone(), identity_of(instance)))
        .collect();

    // Probe outside the store lock: `reconcile` walks the process table and must
    // never hold the mutex while a probe can block. The instance guards stay held.
    let probe = Arc::clone(&inner.probe);
    let outcomes = super::helpers::blocking(move || Ok(reconcile(&records, probe.as_ref()))).await;
    let Ok(outcomes) = outcomes else {
        return;
    };

    for outcome in outcomes {
        let Some((_, instance)) = guarded.iter().find(|(_, i)| i.id() == outcome.id()) else {
            continue;
        };
        if outcome.is_adopted() {
            adopt(&inner, instance).await;
        } else {
            mark(&inner, instance, InstanceEvent::ReconciledAbsent).await;
        }
    }
    drop(guarded);
    fire_done(&inner);
}

/// Acquires the per-instance lock when no in-process operation owns it.
///
/// `Err` means busy (an operation holds it in this process); `Ok` gives a guard
/// that releases on drop. It is the only in-process proof that a create/delete is
/// not mid-flight, so every mutating reconciliation decision takes it before
/// reading fresh state.
///
/// It takes the **reconciliation** guard, not an operation guard, so a user
/// command that arrives while reconciliation is deciding waits for it bounded
/// (`Ops::lock_instance`) instead of being refused as busy.
fn lock_if_idle(inner: &Arc<Inner>, id: &InstanceId) -> Option<super::registry::InstanceLock> {
    inner.ops.lock_instance_for_reconcile(id).ok()
}

/// Re-reads one instance from the store, so a decision never acts on a stale
/// snapshot row whose operation already finished.
fn fresh_instance(inner: &Arc<Inner>, id: &InstanceId) -> Option<Instance> {
    inner.db(|store| store.get_instance(id)).ok().flatten()
}

/// Reconciles every row that was mid-operation when the app died.
///
/// **Never purges.** The delete mode is not recorded on the row, so a
/// reconciliation that removed data would silently discard a user's choice of
/// the D-18 default (`ArchiveData`). Each crash point has one safe answer:
///
/// | Data directory | Archive entry for the instance | Meaning                     | Action                    |
/// |----------------|-------------------------------|-----------------------------|---------------------------|
/// | present        | —                             | the move never happened     | abort: back to `Stopped`  |
/// | gone           | none, `archive/<name>-*` exists | rename done, row not written | `add_archive`, then remove the row |
/// | gone           | one recorded                  | rename and record done      | remove the row            |
/// | gone           | none, no archive directory    | a purge had finished        | remove the row            |
///
/// A row whose data is still present is never removed, so `ArchiveData` always
/// wins the benefit of the doubt; the user can retry the delete in either mode.
async fn finish_delete(inner: &Arc<Inner>, instance: &Instance) {
    let id = instance.id().clone();
    let instance_dir = inner.paths.instance_dir(instance.name());
    let data_dir = inner.paths.data_dir(instance.name());

    // (a) The data directory is still there: nothing was moved or removed, so
    // the delete never got past writing `Deleting`. Put the row back and stop.
    if data_dir.exists() {
        let id2 = id.clone();
        let _ = db_async(inner, move |store| {
            store.transition_status(&id2, InstanceStatus::Deleting, InstanceStatus::Stopped)
        })
        .await;
        // We just concluded this delete is not running, so its marker is stale.
        // Leaving it would keep a crashed delete's dotfile in the home forever.
        super::delete::clear_in_flight_marker(&inner.paths, &id);
        emit_changed(inner, &id);
        return;
    }

    // (b) The rename happened but `add_archive` did not: register the directory
    // we can see, so the data is not "data without a row you cannot find".
    //
    // `recorded` is decided by the stable archive id, never by the instance name:
    // a user who deletes `x`, recreates `x`, and crashes that delete must still
    // get the second archive recorded, even though an archived row for a
    // *previous* instance called `x` exists.
    let archive_id = format!("arch-{}", short_id(&id));
    let recorded = inner
        .db(|store| store.list_archives())
        .map(|archives| archives.iter().any(|archive| archive.id() == archive_id))
        .unwrap_or(false);
    if !recorded && let Some(path) = unrecorded_archive(inner, instance) {
        let row = ArchivedData::new(
            archive_id,
            instance.name().clone(),
            instance.engine(),
            instance.version().to_owned(),
            path.clone(),
            super::helpers::dir_size(&path),
            SystemTime::now(),
        );
        let row_for_store = row;
        let added = db_async(inner, move |store| {
            match store.add_archive(&row_for_store) {
                Ok(()) => Ok(true),
                Err(DblError::AlreadyExists { .. }) => Ok(false),
                Err(err) => Err(err),
            }
        })
        .await
        .unwrap_or(false);
        if added {
            tracing::info!(
                instance = %instance.name(),
                path = %path.display(),
                "registered an archive a crashed delete left behind"
            );
        }
    }

    // (c) Data is gone: the remove path is safe whatever the mode was, because
    // nothing is deleted that the user did not already lose or archive.
    if let Ok(Some(reference)) = inner.db(|store| store.get_credential(&id)) {
        let _ = inner.secrets.delete(&reference);
    }
    if super::helpers::safe_within(&inner.paths, &instance_dir).is_ok() {
        let dir = instance_dir;
        let _ = super::helpers::blocking(move || super::helpers::remove_tree(&dir)).await;
    }
    let id2 = id.clone();
    let removed = db_async(inner, move |store| match store.delete_instance(&id2) {
        Ok(()) => Ok(true),
        Err(DblError::NotFound { .. }) => Ok(false),
        Err(err) => Err(err),
    })
    .await
    .unwrap_or(false);
    inner.log_watchers.disable(&id);
    // This delete is over, so its marker no longer means anything. A crash left
    // it behind and nothing else would ever remove it.
    super::delete::clear_in_flight_marker(&inner.paths, &id);
    if removed {
        // Only announce a removal that happened.
        super::helpers::emit_removed(inner, &id);
    }
}

/// An `archive/<name>-<digits>` directory that no archive row points at.
///
/// The match is exact, not a prefix: `pg` must never claim `pg-main-...`, whose
/// remainder is not a stamp. Every directory that an existing archive row
/// already references is skipped, so a recovered directory can never shadow
/// another instance's archive. Each candidate is re-validated against the home.
fn unrecorded_archive(inner: &Arc<Inner>, instance: &Instance) -> Option<PathBuf> {
    let archive_root = inner.paths.archive_dir();
    let prefix = format!("{}-", instance.name().as_str());

    let taken: Vec<PathBuf> = inner
        .db(|store| store.list_archives())
        .unwrap_or_default()
        .into_iter()
        .map(|archive| super::helpers::normalize(archive.path()))
        .collect();

    let entries = std::fs::read_dir(&archive_root).ok()?;
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| matches_archive_name(name, &prefix))
        })
        .filter(|path| super::helpers::safe_within(&inner.paths, path).is_ok())
        .filter(|path| !taken.contains(&super::helpers::normalize(path)))
        .collect();
    found.sort();
    found.pop()
}

/// Whether `name` is `<prefix><stamp>` with a stamp of ASCII digits only.
///
/// `prefix` already ends in `-`, so `pg-main-1700` does not match the prefix of
/// `pg` (`pg-`); it starts with `pg-` but the remainder `main-1700` is not a
/// stamp. The digits check is what enforces that.
fn matches_archive_name(name: &str, prefix: &str) -> bool {
    let Some(stamp) = name.strip_prefix(prefix) else {
        return false;
    };
    !stamp.is_empty() && stamp.bytes().all(|byte| byte.is_ascii_digit())
}

fn short_id(id: &InstanceId) -> String {
    let value = id.as_str();
    value.get(..8).unwrap_or(value).to_owned()
}

/// Cleans up a create the app died in the middle of.
///
/// The same cleanup a failed create runs (`helpers::rollback_create`): data and
/// instance directories, both credential backends and the row. The verified
/// runtime is kept because it is reusable.
///
/// The caller holds the instance lock and passes a row it just re-read, so this
/// only ever removes a row that is genuinely still mid-create; it re-checks the
/// status anyway, because the row could have been settled by a writer in another
/// process in the meantime.
async fn clean_interrupted_create(inner: &Arc<Inner>, instance: &Instance) {
    if !matches!(
        instance.status(),
        InstanceStatus::Provisioning | InstanceStatus::Initializing
    ) {
        return;
    }

    let removed = super::helpers::rollback_create(inner, instance.id(), instance.name()).await;
    if removed {
        super::helpers::emit_removed(inner, instance.id());
    } else {
        emit_changed(inner, instance.id());
    }
}

async fn adopt(inner: &Arc<Inner>, instance: &Instance) {
    // A live `Stopping` row is adopted as `Running`, not written `Stopped`:
    // nothing was signalled here, so the engine is still there, and pretending
    // otherwise leaves a live process under a "stopped" label that collides on
    // the port at the next start. The user can press Stop again.
    let next = match instance.status() {
        InstanceStatus::Starting | InstanceStatus::Running | InstanceStatus::Stopping => {
            InstanceStatus::Running
        }
        other => other,
    };
    if next != instance.status() {
        persist(inner, instance.id(), next).await;
    }
    emit_changed(inner, instance.id());
}

async fn mark(inner: &Arc<Inner>, instance: &Instance, event: InstanceEvent) {
    let clears_identity = matches!(
        event,
        InstanceEvent::ReconciledAbsent | InstanceEvent::Interrupted
    );
    let Ok(next) = instance.status().apply(event) else {
        emit_changed(inner, instance.id());
        return;
    };
    // A terminal status must never be observable while the identity of the process
    // we just declared gone is still recorded: the next start, a stop, the
    // supervisor and `identity_of` all read it. Both writes go in one store
    // transaction, so no reader on any connection can see the half-updated row.
    let id = instance.id().clone();
    if clears_identity {
        let from = instance.status();
        let _ = db_async(inner, move |store| {
            store.transition_status_and_clear_identity(&id, from, next)
        })
        .await;
    } else {
        let id_for_store = id.clone();
        let _ = db_async(inner, move |store| store.update_status(&id_for_store, next)).await;
    }
    // A test observation point placed exactly where the old two-write code had its
    // window (status written, identity not yet cleared). With both writes in one
    // transaction there is nothing half-written to see; with the old order a fresh
    // reader here catches `Stopped` with the identity still recorded.
    #[cfg(test)]
    if clears_identity {
        run_after_status(inner, instance.id());
    }
    emit_changed(inner, instance.id());
}

async fn persist(inner: &Arc<Inner>, id: &InstanceId, next: InstanceStatus) {
    let id = id.clone();
    let _ = db_async(inner, move |store| store.update_status(&id, next)).await;
}
