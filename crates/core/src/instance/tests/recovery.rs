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
    let restarted = super::harness::restart_with(
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

    let restarted = super::harness::restart_with(
        &h.paths,
        Arc::clone(&h.procs),
        Arc::clone(&h.terminator),
        Arc::clone(&h.secrets),
    );
    // Subscribe after the restart so the reconciliation events are observable.
    let mut receiver = restarted.receiver();
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

    // One of the events the restart emitted must be the removal.
    let removal_seen = std::iter::from_fn(|| receiver.try_recv().ok())
        .any(|event| super::harness::removed_for(&[event], &id));
    assert!(removal_seen, "reconciliation emits InstanceRemoved");
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
