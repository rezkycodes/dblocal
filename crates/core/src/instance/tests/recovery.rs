//! Delete crash-point recovery and the interrupted-create cleanup, driven through
//! a real service restart.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::sync::Arc;

use crate::model::{DeleteMode, EngineKey, InstanceId, InstanceStatus};
use crate::service::DblService;

use super::harness::{Harness, test_port};

/// Crash point inside a delete, used by the table-driven recovery test.
#[derive(Clone, Copy, Debug)]
enum CrashPoint {
    /// `Deleting` was written and nothing else happened.
    DeletingWritten,
    /// Archive mode: the rename into `archive/` happened, `add_archive` did not.
    Renamed,
    /// Archive mode: the record exists, the row and credential were not removed.
    Recorded,
    /// Purge mode: the data directory is gone, the row is not.
    Purged,
    /// The credential was deleted, the row was not.
    CredentialDeleted,
}

impl CrashPoint {
    fn label(self) -> &'static str {
        match self {
            Self::DeletingWritten => "after Deleting written",
            Self::Renamed => "after rename into archive/",
            Self::Recorded => "after add_archive",
            Self::Purged => "after purge removed the data",
            Self::CredentialDeleted => "after the credential was deleted",
        }
    }
}

#[test]
fn reconciliation_never_loses_data_for_an_interrupted_delete() {
    // A delete writes `Deleting` before it moves anything and the mode is not
    // recorded on the row, so reconciliation must never purge: each crash point
    // has exactly one safe answer, tested for both delete modes.
    for mode in [DeleteMode::ArchiveData, DeleteMode::PurgeData] {
        for point in [
            CrashPoint::DeletingWritten,
            CrashPoint::Renamed,
            CrashPoint::Recorded,
            CrashPoint::Purged,
            CrashPoint::CredentialDeleted,
        ] {
            // Points that only exist for one mode are skipped for the other.
            match (mode, point) {
                (DeleteMode::PurgeData, CrashPoint::Renamed)
                | (DeleteMode::PurgeData, CrashPoint::Recorded)
                | (DeleteMode::ArchiveData, CrashPoint::Purged) => continue,
                _ => {}
            }
            run_crash_point(mode, point);
        }
    }
}

fn run_crash_point(mode: DeleteMode, point: CrashPoint) {
    let label = format!("{mode:?} / {}", point.label());
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("crash", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let name = instance.name().clone();
    let data_dir = h.paths.data_dir(&name);
    let instance_dir = h.paths.instance_dir(&name);
    // The harness adapter wrote a data directory with content.
    std::fs::write(data_dir.join("marker.txt"), b"precious").expect("marker");

    // Drive the delete up to the chosen crash point, then stop, simulating the
    // app dying there. The real order is: `Deleting`, move/purge the data,
    // record the archive (archive mode), delete the credential, delete the row.
    let store = h.store();
    store
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Deleting)
        .expect("claim");

    // `AfterCredentialDeleted` happens after the data is archived and recorded,
    // so it replays the full archive path first, then the credential step.
    let mode_is_archive_step =
        matches!(mode, DeleteMode::ArchiveData) && !matches!(point, CrashPoint::DeletingWritten);
    // In purge mode the only pre-credential state is "data removed"; both
    // `AfterPurgeRenamed` and `AfterCredentialDeleted` have done that.
    let purge_step =
        matches!(mode, DeleteMode::PurgeData) && !matches!(point, CrashPoint::DeletingWritten);

    let mut archive_path = None;
    if mode_is_archive_step {
        let target = h
            .paths
            .archive_dir()
            .join(format!("{}-999999", name.as_str()));
        std::fs::create_dir_all(h.paths.archive_dir()).expect("archive dir");
        std::fs::rename(&data_dir, &target).expect("rename data");
        if matches!(point, CrashPoint::Recorded | CrashPoint::CredentialDeleted) {
            // The stable id the service derives, so reconciliation's recovery
            // (which uses the same id) is idempotent against this row.
            let short = id.as_str().get(..8).unwrap_or(id.as_str()).to_owned();
            store
                .add_archive(&crate::model::ArchivedData::new(
                    format!("arch-{short}"),
                    name.clone(),
                    EngineKey::Postgresql,
                    "18.7",
                    target.clone(),
                    7,
                    std::time::SystemTime::UNIX_EPOCH,
                ))
                .expect("record archive");
        }
        archive_path = Some(target);
    }
    if purge_step {
        std::fs::remove_dir_all(&data_dir).expect("purge data");
    }
    if matches!(point, CrashPoint::CredentialDeleted | CrashPoint::Recorded)
        && let Some(reference) = store.get_credential(&id).expect("credential")
    {
        use crate::secrets::SecretBackend;
        h.secrets.file.delete(&reference).ok();
    }

    // App restart: reconciliation must finish this safely.
    let (restarted, _startup) = super::harness::restart_with(
        &h.paths,
        Arc::clone(&h.procs),
        Arc::clone(&h.terminator),
        Arc::clone(&h.secrets),
    );
    wait_until_settled(&restarted, &id);

    match point {
        // The move never happened: the delete is aborted, the row is back.
        CrashPoint::DeletingWritten => {
            let row = restarted
                .instance(&id)
                .unwrap_or_else(|| panic!("{label}: the row must survive an aborted delete"));
            assert_eq!(
                row.status(),
                InstanceStatus::Stopped,
                "{label}: an aborted delete returns to Stopped"
            );
            assert!(
                data_dir.join("marker.txt").exists(),
                "{label}: the data must survive an aborted delete"
            );
        }
        // The data was archived: whether the record existed or is recovered, the
        // archive entry and its content must survive. `AfterCredentialDeleted`
        // reached the same point in archive mode, so the same answer applies.
        CrashPoint::Renamed | CrashPoint::Recorded => {
            assert!(
                restarted.instance(&id).is_none(),
                "{label}: the row is removed once the data is archived"
            );
            let archives = restarted.service.list_archives().expect("archives");
            assert_eq!(
                archives.len(),
                1,
                "{label}: exactly one archive must be recorded, saw {archives:?}"
            );
            assert!(
                archives[0].path().join("marker.txt").exists(),
                "{label}: the archived data must survive"
            );
        }
        // A purge had removed the data: nothing is left to preserve, so the row
        // goes. In archive mode the data was archived first, and `archive_path`
        // carries where it landed.
        CrashPoint::Purged | CrashPoint::CredentialDeleted => {
            assert!(
                restarted.instance(&id).is_none(),
                "{label}: the row is removed after its data is gone"
            );
            let archives = restarted.service.list_archives().expect("archives");
            if mode == DeleteMode::ArchiveData {
                // The data was archived before the credential step: nothing may
                // be lost, so exactly the recorded entry (or the recovery of the
                // renamed directory) must exist.
                assert_eq!(
                    archives.len(),
                    1,
                    "{label}: archive mode keeps exactly one archive, saw {archives:?}"
                );
                assert!(
                    archives[0].path().join("marker.txt").exists(),
                    "{label}: the archived data must survive"
                );
                let recorded = archive_path.expect("archive path");
                assert!(
                    archives[0].path() == recorded || recorded.exists(),
                    "{label}: the archive points at the data that was moved"
                );
            } else {
                assert!(
                    archives.is_empty(),
                    "{label}: no archive is invented for a purge, saw {archives:?}"
                );
            }
        }
    }

    // Whichever the mode, reconciliation never leaves the instance directory of
    // a row it removed.
    if restarted.instance(&id).is_none() {
        assert!(
            !instance_dir.exists(),
            "{label}: the instance shell is removed with the row"
        );
    }
}

/// Waits until reconciliation has settled `id` (row gone, or not `Deleting`).
fn wait_until_settled(h: &Harness, id: &InstanceId) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match h.instance(id) {
            None => return,
            Some(row) if row.status() != InstanceStatus::Deleting => return,
            Some(_) => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "reconciliation never settled the interrupted delete"
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

#[test]
fn reconciliation_adopts_a_live_stopping_row_as_running() {
    // Nothing is signalled during reconciliation, so a `Stopping` row whose
    // process is alive is still running. Writing it `Stopped` would leave a live
    // engine under a "stopped" label that collides on the port at the next start.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("live-stopping", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    let identity = h.procs.spawn("/fake/engine/bin/postgres");
    h.store()
        .update_process_identity(
            &id,
            Some(identity.pid()),
            Some(identity.started_at()),
            Some(identity.exe()),
        )
        .expect("identity");
    h.store()
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Stopping)
        .expect("stopping");

    let restarted = super::harness::restart(&h.paths, Arc::clone(&h.procs));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Some(row) = restarted.instance(&id)
            && row.status() == InstanceStatus::Running
        {
            assert_eq!(
                row.pid(),
                Some(identity.pid()),
                "an adopted Stopping row keeps its identity"
            );
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "a live Stopping row was not adopted as Running"
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

#[test]
fn a_reconciled_interrupted_create_leaves_nothing_behind() {
    // A create the app died in the middle of must not leave a `Failed` zombie
    // with a half-initialised data directory: the same cleanup a failed create
    // runs is applied, and consumers are told the row is gone.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("interrupted", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let name = instance.name().clone();
    // Model a crash mid-create: the row stayed `Initializing` and half a data
    // directory exists, plus a credential pointer.
    let data_dir = h.paths.data_dir(&name);
    std::fs::write(data_dir.join("PG_VERSION"), b"18\n").expect("half data dir");
    h.store()
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Initializing)
        .expect("initializing");

    let (restarted, mut startup_receiver) = super::harness::restart_with(
        &h.paths,
        Arc::clone(&h.procs),
        Arc::clone(&h.terminator),
        Arc::clone(&h.secrets),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while restarted.instance(&id).is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "an interrupted create must be cleaned up, not left as a zombie"
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    assert!(
        !data_dir.exists(),
        "the half-initialised data directory is removed"
    );
    assert!(
        !h.paths.instance_dir(&name).exists(),
        "the instance shell is removed"
    );
    assert!(
        h.store().get_credential(&id).expect("credential").is_none(),
        "the credential pointer is gone"
    );

    // Subscribe-before-startup is the contract: `restart_with` returns a receiver
    // created before reconciliation began, so the removal event is observable even
    // when reconciliation wins the race. `InstanceRemoved` is also durable in the
    // sense that the row is gone; assert both.
    let removal_seen = super::harness::wait_for_removed(
        &mut startup_receiver,
        &id,
        std::time::Duration::from_secs(30),
    );
    assert!(
        removal_seen,
        "reconciliation emits InstanceRemoved for {id}; the row is gone but no removal event arrived before timeout"
    );
}

#[test]
fn a_create_that_finishes_while_reconciliation_lists_is_never_rolled_back() {
    // The flake this test pins: reconciliation lists the rows (an `await` point),
    // then acts on that snapshot. A create that was `Initializing` at list time can
    // finish and release its instance lock before reconciliation reaches the
    // cleanup, so the stale snapshot row looks unowned and gets the failed-create
    // rollback — deleting a successfully created instance.
    //
    // `on_reconcile` forces that interleaving deterministically: the `before` hook
    // writes the `Initializing` row the snapshot will see, and the `after` hook
    // settles it to `Stopped` — exactly what a create that completes mid-list
    // leaves behind. Without the fix (act on the snapshot row, guarded only by
    // `is_busy`) the row is deleted and this test fails. With the fix (take the
    // instance lock and re-read the row) the settled row is left alone.
    let h = Harness::paused(EngineKey::Postgresql);
    let root = h.paths.root().to_path_buf();
    let name = crate::model::InstanceName::new("finished-during-list").expect("name");
    let port = test_port();
    let id = crate::model::InstanceId::new();

    let before_id = id.clone();
    let before_name = name.clone();
    let after_id = id.clone();
    super::super::reconcile::on_reconcile(
        &root,
        move |inner| {
            let spec = crate::model::InstanceSpec::new(
                before_name,
                EngineKey::Postgresql,
                "18.7",
                Some(crate::model::Port::new(port).expect("port")),
                crate::model::AuthMode::Password,
            )
            .expect("spec");
            let row = crate::model::Instance::new(
                &spec,
                before_id,
                InstanceStatus::Initializing,
                std::time::SystemTime::now(),
            );
            inner
                .db(|store| store.insert_instance(&row))
                .expect("insert initializing row");
        },
        move |inner| {
            // The create finished between the list and the cleanup: the row settles
            // to `Stopped`, and its operation lock is already released (never taken
            // here, mirroring a create that completed and dropped its guard).
            let moved = inner
                .db(|store| {
                    store.transition_status(
                        &after_id,
                        InstanceStatus::Initializing,
                        InstanceStatus::Stopped,
                    )
                })
                .expect("settle row");
            assert!(moved, "the row must still be Initializing after the list");
        },
    );

    let restarted = super::harness::restart_paused(&h.paths, Arc::clone(&h.procs));
    let done = super::super::reconcile::on_reconcile_done(&root);
    restarted.service.start_background();
    // Wait for reconciliation to run to completion, so the assertion is made
    // *after* every cleanup decision, not before it (observing the row first would
    // let the bug pass on timing).
    done.recv_timeout(std::time::Duration::from_secs(30))
        .expect("reconciliation must finish");
    let row = restarted.instance(&id).unwrap_or_else(|| {
        panic!(
            "a create that finished during reconciliation must survive; rows={:?}",
            restarted.instances()
        )
    });
    assert_eq!(
        row.status(),
        InstanceStatus::Stopped,
        "the finished create keeps its settled status"
    );
}

#[test]
fn reconciliation_skips_a_deleting_row_owned_by_a_live_delete() {
    // The row alone cannot tell "a delete is running elsewhere" from "the app
    // died mid-delete": both are `Deleting`. The in-flight marker resolves it, and
    // this pins both directions deterministically instead of relying on a race
    // window.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("owned", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let name = instance.name().clone();
    // Data present means "the move never happened", which alone would make
    // reconciliation abort the delete (crash point (a)).
    std::fs::write(h.paths.data_dir(&name).join("marker.txt"), b"x").expect("data");
    h.store()
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Deleting)
        .expect("deleting");

    // A live process that is *not* ours owns the delete, exactly like the other
    // service sharing the home. The marker records its full identity, because a
    // bare PID is not proof of ownership.
    let owner = h.procs.spawn("/some/other/dblocal");
    let marker = h
        .paths
        .instance_dir(&name)
        .join(super::super::delete::IN_FLIGHT_MARKER);
    let started = owner
        .started_at()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}.0", d.as_secs()))
        .unwrap_or_else(|_| "0.0".to_owned());
    std::fs::write(
        &marker,
        format!(
            "pid={}\nstarted={started}\nexe={}\nnonce=some-nonce\n",
            owner.pid(),
            owner.exe().display()
        ),
    )
    .expect("marker");

    let restarted = super::harness::restart(&h.paths, Arc::clone(&h.procs));
    std::thread::sleep(std::time::Duration::from_millis(200));
    let row = restarted
        .instance(&id)
        .expect("an in-flight delete must not be aborted by another process");
    assert_eq!(
        row.status(),
        InstanceStatus::Deleting,
        "reconciliation must leave a delete that is still running alone"
    );

    // Once the owner is gone the marker is stale, and the next launch recovers.
    h.procs.kill(owner.pid());
    let again = super::harness::restart(&h.paths, Arc::clone(&h.procs));
    wait_until_settled(&again, &id);
    let recovered = again.instance(&id).expect("recovered row");
    assert_eq!(
        recovered.status(),
        InstanceStatus::Stopped,
        "a stale marker lets reconciliation abort the delete back to Stopped"
    );
    assert!(
        h.paths.data_dir(&name).join("marker.txt").exists(),
        "aborting the delete keeps the data"
    );
}

#[test]
fn an_unverifiable_marker_identity_is_treated_as_interrupted_and_never_purges() {
    // `own_identity` falls back to `SystemTime::now()` when the probe cannot
    // observe our own process. A reader in another process can never validate
    // that start time, so `delete_in_flight` answers `false` and reconciliation
    // treats the `Deleting` row as interrupted. With the data directory still
    // present that means "abort the delete back to `Stopped`", never a purge.
    //
    // This pins the documented behaviour in docs/architecture/service.md: an
    // unverifiable marker degrades to recovery, and recovery never purges.
    //
    // Mutation check: make `delete_in_flight` trust a bare PID instead of the
    // recorded start time and this test fails with
    //   panicked at .../recovery.rs:231:9:
    //   reconciliation never settled the interrupted delete
    // because the row stays `Deleting` forever instead of aborting to `Stopped`.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("unverifiable", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let name = instance.name().clone();

    // Data still there, so the safe answer is "abort, do not purge".
    let data_dir = h.paths.data_dir(&name);
    std::fs::write(data_dir.join("precious.txt"), b"keep").expect("precious data");
    h.store()
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Deleting)
        .expect("deleting");

    // A marker that names a live process, but with a start time the probe will
    // never match (a foreign instant) — the same shape `own_identity`'s `now()`
    // fallback leaves behind.
    let owner = h.procs.spawn("/some/other/dblocal");
    let marker = h
        .paths
        .instance_dir(&name)
        .join(super::super::delete::IN_FLIGHT_MARKER);
    std::fs::write(
        &marker,
        format!(
            "pid={}\nstarted=1.0\nexe={}\nnonce=foreign\n",
            owner.pid(),
            owner.exe().display()
        ),
    )
    .expect("marker");

    let restarted = super::harness::restart(&h.paths, Arc::clone(&h.procs));
    wait_until_settled(&restarted, &id);

    let row = restarted
        .instance(&id)
        .expect("the row survives an interrupted delete");
    assert_eq!(
        row.status(),
        InstanceStatus::Stopped,
        "an unverifiable marker means interrupted, so the delete aborts"
    );
    assert!(
        data_dir.join("precious.txt").exists(),
        "recovery must never purge data it cannot prove was archived"
    );
    assert!(
        restarted
            .service
            .list_archives()
            .expect("archives")
            .is_empty(),
        "no archive is invented for a delete that never moved the data"
    );
}

/// R1: a row that holds a process and is owned by an in-flight operation must be
/// left alone by reconciliation's third phase.
///
/// The third phase (`pending`: `Starting`/`Running`/`Stopping`) acts on the
/// startup snapshot. When an in-process stop (or start) owns the instance, the
/// snapshot row is a claim the operation is still working on:
///
/// - `Stopping` adopted as `Running` lets a second stop be accepted while the
///   first is in flight, or leaves a `Running` row over a dead process if the
///   stop already finished;
/// - a `Starting` row adopted as `Running` claims readiness the start has not
///   reached, and one marked absent is written `Stopped` with its identity
///   cleared under a live start.
///
/// The test is deterministic: the instance lock is held before reconciliation
/// runs, and `on_reconcile` inserts the owned row just before the snapshot is
/// taken, so the interleaving does not depend on timing.
#[test]
fn reconciliation_leaves_a_row_owned_by_an_in_flight_operation_alone() {
    for status in [InstanceStatus::Stopping, InstanceStatus::Starting] {
        run_in_flight_reconcile_case(status);
    }
}

fn run_in_flight_reconcile_case(status: InstanceStatus) {
    let h = Harness::paused(EngineKey::Postgresql);
    let root = h.paths.root().to_path_buf();
    let id = crate::model::InstanceId::new();
    let name = crate::model::InstanceName::new("in-flight").expect("name");
    let port = test_port();
    let procs = Arc::clone(&h.procs);

    // The operation owns the instance before reconciliation starts. The guard is
    // held for the whole pass and dropped only after it finishes.
    let guard = h.lock_instance(&id);

    let before_id = id.clone();
    let before_name = name.clone();
    let before_procs = Arc::clone(&procs);
    super::super::reconcile::on_reconcile(
        &root,
        move |inner| {
            let identity = before_procs.spawn("/fake/engine/bin/postgres");
            let spec = crate::model::InstanceSpec::new(
                before_name,
                EngineKey::Postgresql,
                "18.7",
                Some(crate::model::Port::new(port).expect("port")),
                crate::model::AuthMode::Password,
            )
            .expect("spec");
            let row =
                crate::model::Instance::new(&spec, before_id, status, std::time::SystemTime::now())
                    .with_process(
                        identity.pid(),
                        identity.started_at(),
                        identity.exe().to_path_buf(),
                    );
            inner
                .db(|store| store.insert_instance(&row))
                .expect("insert the operation-owned row");
        },
        |_| {},
    );

    let mut events = h.receiver();
    let done = super::super::reconcile::on_reconcile_done(&root);
    h.service.start_background();
    done.recv_timeout(std::time::Duration::from_secs(30))
        .expect("reconciliation must finish");
    drop(guard);

    let row = h.instance(&id).expect("the operation-owned row survives");
    assert_eq!(
        row.status(),
        status,
        "reconciliation must not overwrite the {status:?} claim of an in-flight operation"
    );
    assert!(
        row.pid().is_some(),
        "the {status:?} row keeps the identity the operation recorded"
    );

    let mut collected = Vec::new();
    while let Ok(event) = events.try_recv() {
        collected.push(event);
    }
    let announced = collected.iter().any(|event| {
        matches!(
            event,
            crate::service::ServiceEvent::InstanceChanged { view } if view.id == id
        )
    });
    assert!(
        !announced,
        "reconciliation must not announce a change for a row an operation owns; events={:?}",
        super::harness::describe(&collected)
    );
}

/// Item 1: a row must never be observable as terminal (`Stopped`) while it still
/// carries the identity of the process reconciliation just declared gone.
///
/// `mark(ReconciledAbsent)` used to write the status first and clear the identity
/// second, two separate store writes. A reader on another connection (the GUI, the
/// CLI, a second service, or this test's fresh `Store::open`) that lands between
/// them sees `Stopped` with the old pid. The `on_after_status` hook pins that exact
/// instant, so the assertion is deterministic rather than a race.
#[test]
fn a_terminal_status_is_never_observed_with_a_stale_identity() {
    let h = Harness::paused(EngineKey::Postgresql);
    let root = h.paths.root().to_path_buf();
    let paths = h.paths.clone();
    let id = crate::model::InstanceId::new();
    let name = crate::model::InstanceName::new("stale-identity").expect("name");
    let port = test_port();
    let procs = Arc::clone(&h.procs);

    // A `Running` row whose recorded process is dead: reconciliation decides
    // `ReconciledAbsent`, which is the `mark` path under test.
    let before_id = id.clone();
    let before_name = name.clone();
    let before_procs = Arc::clone(&procs);
    super::super::reconcile::on_reconcile(
        &root,
        move |inner| {
            // A dead identity: spawn then kill, so the probe reports it absent.
            let identity = before_procs.spawn("/fake/engine/bin/postgres");
            before_procs.kill(identity.pid());
            let spec = crate::model::InstanceSpec::new(
                before_name,
                EngineKey::Postgresql,
                "18.7",
                Some(crate::model::Port::new(port).expect("port")),
                crate::model::AuthMode::Password,
            )
            .expect("spec");
            let row = crate::model::Instance::new(
                &spec,
                before_id,
                InstanceStatus::Running,
                std::time::SystemTime::now(),
            )
            .with_process(
                identity.pid(),
                identity.started_at(),
                identity.exe().to_path_buf(),
            );
            inner
                .db(|store| store.insert_instance(&row))
                .expect("insert running row");
        },
        |_| {},
    );

    // Observe a FRESH connection at the instant the status has been written and the
    // identity has not: this is the window a real reader can land in.
    let observed = Arc::new(std::sync::Mutex::new(None::<(InstanceStatus, bool)>));
    let observed_hook = Arc::clone(&observed);
    let paths_hook = paths.clone();
    super::super::reconcile::on_after_status(&root, move |_inner, id| {
        let store = crate::store::Store::open(&paths_hook).expect("fresh store");
        let row = store.get_instance(id).expect("read").expect("row present");
        *observed_hook.lock().expect("observed") = Some((row.status(), row.pid().is_some()));
    });

    let done = super::super::reconcile::on_reconcile_done(&root);
    h.service.start_background();
    done.recv_timeout(std::time::Duration::from_secs(30))
        .expect("reconciliation must finish");

    let seen = observed
        .lock()
        .expect("observed")
        .clone()
        .expect("the hook must have observed the row between the two writes");
    assert_ne!(
        seen,
        (InstanceStatus::Stopped, true),
        "no reader may see Stopped with a recorded identity; every write of a \
         terminal status must clear the identity in the same transaction"
    );

    let row = h.instance(&id).expect("the row survives");
    assert_eq!(row.status(), InstanceStatus::Stopped);
    assert!(row.pid().is_none(), "the identity is cleared at the end");
}
