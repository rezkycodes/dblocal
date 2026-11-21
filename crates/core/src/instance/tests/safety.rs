//! Cross-process safety and path-traversal tests.

use std::path::PathBuf;
use std::sync::Arc;

use crate::model::{DeleteMode, EngineKey, InstanceId, InstanceName, InstanceStatus};
use crate::service::DblService;

use super::harness::{Harness, test_port};

#[test]
fn two_services_on_one_home_spawn_exactly_once() {
    // Two independent services over the same home, both asked to start the same
    // stopped instance at the same instant. The compare-and-set transition must
    // let exactly one win, and only the winner may spawn.
    for round in 0..50 {
        let h = Harness::new(EngineKey::Postgresql);
        let port = test_port();
        let spec = h.spec("race", EngineKey::Postgresql, port);
        h.create(spec);
        let instance = h.instances().into_iter().next().expect("instance");
        let id = instance.id().clone();

        // A second service over the same state tree, sharing the process table.
        let second = super::harness::second_service(&h.paths, Arc::clone(&h.procs));

        let a = h.service.start_instance(&id);
        let b = second.service.start_instance(&id);

        let accepted = match (&a, &b) {
            (Ok(op), Err(_)) => (true, op.clone()),
            (Err(_), Ok(op)) => (false, op.clone()),
            _ => panic!("round {round}: exactly one start must be accepted, got {a:?} / {b:?}"),
        };
        let winner_is_first = accepted.0;
        let winner_op = accepted.1;

        // The loser must get the typed busy error, not a silent success.
        let loser = if winner_is_first { &b } else { &a };
        let loser_err = loser.as_ref().expect_err("the loser must be refused");
        assert_eq!(
            loser_err.code(),
            crate::error::CODE_INVALID_TRANSITION,
            "round {round}: the loser gets a typed busy error, got {loser_err:?}"
        );

        // Wait for the winner's operation to settle before counting spawns: the
        // spawn happens inside the operation, not before `start_instance`
        // returns.
        let winner = if winner_is_first { &h } else { &second };
        let _ = winner.run(winner_op);
        winner.wait_for_status(&id, InstanceStatus::Running);

        assert_eq!(
            h.procs.spawned_count(),
            1,
            "round {round}: exactly one spawn must have happened"
        );
    }
}

#[test]
fn two_concurrent_deletes_remove_the_instance_once() {
    // The same compare-and-set that guards start must guard delete: one caller
    // removes the row (and archives), the other is refused as busy.
    for round in 0..50 {
        let h = Harness::new(EngineKey::Postgresql);
        let port = test_port();
        let spec = h.spec("del-race", EngineKey::Postgresql, port);
        h.create(spec);
        let instance = h.instances().into_iter().next().expect("instance");
        let id = instance.id().clone();

        let second = super::harness::second_service(&h.paths, Arc::clone(&h.procs));
        let a = h.service.delete_instance(&id, DeleteMode::ArchiveData);
        let b = second.service.delete_instance(&id, DeleteMode::ArchiveData);

        let accepted = match (&a, &b) {
            (Ok(op), Err(_)) => (true, op.clone()),
            (Err(_), Ok(op)) => (false, op.clone()),
            _ => panic!("round {round}: exactly one delete must be accepted, got {a:?} / {b:?}"),
        };
        let winner = if accepted.0 { &h } else { &second };
        let _ = winner.run(accepted.1);

        // The row is gone exactly once and at most one archive exists.
        assert!(
            h.instance(&id).is_none() && second.instance(&id).is_none(),
            "round {round}: the row must be removed exactly once"
        );
        let archives = h.service.list_archives().expect("archives");
        assert!(
            archives.len() <= 1,
            "round {round}: at most one archive, saw {}",
            archives.len()
        );
    }
}

#[test]
fn purge_archive_rejects_traversal_absolute_and_symlink_paths() {
    let h = Harness::new(EngineKey::Postgresql);
    // Hand-build archive rows the service itself would never write.
    let outside = h.dir.path().join("outside-target");
    std::fs::create_dir_all(&outside).expect("outside");
    std::fs::create_dir_all(h.paths.archive_dir()).expect("archive dir");
    let store = h.store();

    for (id, path) in [
        ("evil-dotdot", PathBuf::from("../../../../etc")),
        ("evil-absolute", PathBuf::from("/etc")),
        (
            "evil-symlink",
            h.paths.archive_dir().join("link-to-outside"),
        ),
    ] {
        if id == "evil-symlink" {
            std::os::unix::fs::symlink(&outside, &path).expect("symlink");
        }
        let record = crate::model::ArchivedData::new(
            id,
            InstanceName::new("abc").expect("name"),
            EngineKey::Postgresql,
            "18.7",
            path,
            1,
            std::time::SystemTime::UNIX_EPOCH,
        );
        store.add_archive(&record).expect("seed archive");
    }

    for id in ["evil-dotdot", "evil-absolute", "evil-symlink"] {
        let err = h.service.purge_archive(id).expect_err("must refuse");
        assert_eq!(
            err.code(),
            crate::error::CODE_UNSAFE_ARCHIVE,
            "{id} must be refused as unsafe, got {err:?}"
        );
    }
    assert!(outside.exists(), "nothing outside the home may be removed");
}

#[test]
fn remove_runtime_refuses_while_an_instance_uses_it() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("app-dev", EngineKey::Postgresql, port);
    h.create(spec);
    // The harness never records a real runtime, so plant one.
    let dir = h
        .paths
        .runtimes_dir()
        .join("postgresql")
        .join("18.7")
        .join("x86_64");
    std::fs::create_dir_all(dir.join("bin")).expect("runtime dir");
    let runtime = crate::model::Runtime::new(
        "postgresql-18.7-x86_64",
        EngineKey::Postgresql,
        "18.7",
        crate::platform::Arch::X86_64,
        dir,
        1,
        std::time::SystemTime::UNIX_EPOCH,
        crate::model::RuntimeState::Installed,
    );
    h.store().add_runtime(&runtime).expect("record runtime");

    let err = h
        .service
        .remove_runtime("postgresql-18.7-x86_64")
        .expect_err("in use");
    assert_eq!(err.code(), crate::error::CODE_INVALID_INPUT);
    assert!(runtime.path().exists(), "the runtime must be kept");
}

#[test]
fn reconciliation_adopts_a_live_process_and_clears_a_dead_one() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("adopted", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    // Simulate a session that started the engine and then quit: the row says
    // `running` with a live process behind it.
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
        .update_status(&id, InstanceStatus::Running)
        .expect("status");

    // A dead row, pointing at a PID nobody owns.
    let port2 = test_port();
    let spec2 = h.spec("dead", EngineKey::Postgresql, port2);
    h.create(spec2);
    let dead = h
        .instances()
        .into_iter()
        .find(|i| i.name().as_str() == "dead")
        .expect("dead instance");
    let dead_id = dead.id().clone();
    h.store()
        .update_process_identity(
            &dead_id,
            Some(999_999),
            Some(std::time::SystemTime::UNIX_EPOCH),
            Some(std::path::Path::new("/usr/bin/gone")),
        )
        .expect("dead identity");
    h.store()
        .update_status(&dead_id, InstanceStatus::Running)
        .expect("status");

    // Restart the app: drop the service, build a fresh one over the same home.
    // Reconciliation runs in the background, so wait for both rows to settle.
    let restarted = super::harness::restart(&h.paths, Arc::clone(&h.procs));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let views = restarted.service.list_instances().expect("list");
        let adopted = views.iter().find(|v| v.id == id).expect("adopted present");
        let cleared = views
            .iter()
            .find(|v| v.id == dead_id)
            .expect("dead present");
        if adopted.status == InstanceStatus::Running && cleared.status == InstanceStatus::Stopped {
            assert!(
                restarted
                    .instance(&dead_id)
                    .expect("dead row")
                    .pid()
                    .is_none(),
                "a dead identity is cleared on reconciliation"
            );
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "reconciliation did not settle: adopted={:?} cleared={:?}",
            adopted.status,
            cleared.status
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

#[test]
fn the_supervisor_notices_a_killed_process() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("watched", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let op = h.service.start_instance(&id).expect("start");
    h.run(op);
    h.wait_for_status(&id, InstanceStatus::Running);

    let running = h.instance(&id).expect("running");
    h.procs.kill(running.pid().expect("pid"));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(current) = h.instance(&id)
            && current.status() == InstanceStatus::Failed
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "supervisor never noticed"
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

#[test]
fn log_watcher_does_no_work_while_disabled() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("logs", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    assert_eq!(
        h.service.tail_logs(&id, 10).expect("tail"),
        Vec::<String>::new()
    );
    assert_eq!(h.service.watched_logs(), 0, "no follower until asked");
    h.service.watch_logs(&id, true).expect("watch on");
    assert_eq!(h.service.watched_logs(), 1);
    h.service.watch_logs(&id, false).expect("watch off");
    assert_eq!(h.service.watched_logs(), 0);
}

#[test]
fn sentinel_password_never_appears_in_events_errors_or_the_database() {
    const SENTINEL: &str = "SENTINEL-pw-9c1f";
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("secret-scan", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    // Store a known sentinel so the scan has something to find if it leaks.
    let reference = h
        .store()
        .get_credential(&id)
        .expect("credential")
        .expect("recorded");
    use crate::secrets::SecretBackend;
    h.secrets
        .file
        .set(&reference, &crate::secret::SecretString::from(SENTINEL))
        .expect("set");

    let mut receiver = h.service.subscribe();
    let op = h.service.start_instance(&id).expect("start");
    let mut rendered = String::new();
    loop {
        match receiver.blocking_recv() {
            Ok(event) => {
                rendered.push_str(&format!("{event:?}"));
                if matches!(&event, crate::service::ServiceEvent::OperationFinished { id, .. } if id == &op)
                {
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }
    let uri = h
        .service
        .connection_uri(&id, crate::service::UriFormat::DotEnv);
    assert!(
        !rendered.contains(SENTINEL),
        "events leaked the password: {rendered}"
    );
    if let Ok(secret) = uri {
        assert!(!format!("{secret:?}").contains(SENTINEL));
    }
    let db = std::fs::read(h.paths.state_db()).expect("db");
    assert!(
        !String::from_utf8_lossy(&db).contains(SENTINEL),
        "SQLite must never contain the password"
    );
}

#[test]
fn a_corrupt_row_cannot_delete_outside_the_home() {
    let h = Harness::new(EngineKey::Postgresql);
    let outside = h.dir.path().join("keep-me");
    std::fs::create_dir_all(&outside).expect("outside");
    // Point a row's recorded archive at a path outside the home.
    let record = crate::model::ArchivedData::new(
        "corrupt",
        InstanceName::new("abc").expect("name"),
        EngineKey::Postgresql,
        "18.7",
        outside.clone(),
        1,
        std::time::SystemTime::UNIX_EPOCH,
    );
    h.store().add_archive(&record).expect("seed");
    let err = h.service.purge_archive("corrupt").expect_err("must refuse");
    assert_eq!(err.code(), crate::error::CODE_UNSAFE_ARCHIVE);
    assert!(outside.exists(), "outside data must survive");
}

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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
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
    // service sharing the home.
    let owner = h.procs.spawn("/some/other/dblocal");
    let marker = h
        .paths
        .instance_dir(&name)
        .join(super::super::delete::IN_FLIGHT_MARKER);
    std::fs::write(&marker, format!("{} some-nonce", owner.pid())).expect("marker");

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
