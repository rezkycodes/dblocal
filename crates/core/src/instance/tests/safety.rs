//! Cross-process safety and path-traversal tests.

use std::path::PathBuf;
use std::sync::Arc;

use crate::model::{DeleteMode, EngineKey, InstanceName, InstanceStatus};
use crate::service::DblService;

use super::harness::{Harness, test_port, wait_for_row_gone};

#[test]
fn two_services_on_one_home_spawn_exactly_once() {
    // Two independent services over the same home, released from a barrier at the
    // same instant so the calls genuinely overlap. Calling them one after the
    // other would let the second observe `Starting` and fail the state machine
    // before the compare-and-set is ever reached, which is why this used to pass
    // even with the CAS removed. The CAS must let exactly one win, only the
    // winner may spawn, and the loser gets the typed busy error.
    let winners = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for round in 0..50 {
        let h = Harness::new(EngineKey::Postgresql);
        let port = test_port();
        let spec = h.spec("race", EngineKey::Postgresql, port);
        h.create(spec);
        let instance = h.instances().into_iter().next().expect("instance");
        let id = instance.id().clone();

        // A second service over the same state tree, sharing the process table.
        let second =
            super::harness::second_service(&h.paths, Arc::clone(&h.procs), Arc::clone(&h.secrets));

        let barrier = Arc::new(std::sync::Barrier::new(2));
        let a_svc = h.service.clone();
        let b_svc = second.service.clone();
        let a_barrier = Arc::clone(&barrier);
        let b_barrier = Arc::clone(&barrier);
        let a_id = id.clone();
        let b_id = id.clone();
        let a = std::thread::spawn(move || {
            a_barrier.wait();
            a_svc.start_instance(&a_id)
        });
        let b = std::thread::spawn(move || {
            b_barrier.wait();
            b_svc.start_instance(&b_id)
        });
        let a = a.join().expect("first thread did not panic");
        let b = b.join().expect("second thread did not panic");

        let winner_first = match (&a, &b) {
            (Ok(_), Err(_)) => true,
            (Err(_), Ok(_)) => false,
            _ => panic!("round {round}: exactly one start must be accepted, got {a:?} / {b:?}"),
        };
        winners.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // The loser must get the typed busy error, not a silent success.
        let loser = if winner_first { &b } else { &a };
        let loser_err = loser.as_ref().expect_err("the loser must be refused");
        assert_eq!(
            loser_err.code(),
            crate::error::CODE_INVALID_TRANSITION,
            "round {round}: the loser gets a typed busy error, got {loser_err:?}"
        );

        // Wait for the winner's operation to settle before counting spawns: the
        // spawn happens inside the operation, not before `start_instance`
        // returns. The end state is what matters, so poll it rather than the
        // winner's event stream — the winner may be the second service (its own
        // channel) and the operation can finish before this thread subscribes.
        let winner = if winner_first { &h } else { &second };
        winner.wait_for_status(&id, InstanceStatus::Running);

        assert_eq!(
            h.procs.spawned_count(),
            1,
            "round {round}: exactly one spawn must have happened"
        );
    }
    assert_eq!(
        winners.load(std::sync::atomic::Ordering::SeqCst),
        50,
        "every round produced exactly one winner"
    );
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

        let second =
            super::harness::second_service(&h.paths, Arc::clone(&h.procs), Arc::clone(&h.secrets));

        // Release both deletes from a barrier, so they genuinely overlap and the
        // compare-and-set is what decides the winner (a sequential pair would let
        // the second see `Deleting` and be refused by the terminal-status check
        // before any CAS).
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let a_svc = h.service.clone();
        let b_svc = second.service.clone();
        let a_barrier = Arc::clone(&barrier);
        let b_barrier = Arc::clone(&barrier);
        let a_id = id.clone();
        let b_id = id.clone();
        let a = std::thread::spawn(move || {
            a_barrier.wait();
            a_svc.delete_instance(&a_id, DeleteMode::ArchiveData)
        });
        let b = std::thread::spawn(move || {
            b_barrier.wait();
            b_svc.delete_instance(&b_id, DeleteMode::ArchiveData)
        });
        let a = a.join().expect("first thread did not panic");
        let b = b.join().expect("second thread did not panic");

        let (winner_first, winner_op) = match (&a, &b) {
            (Ok(op), Err(_)) => (true, op.clone()),
            (Err(_), Ok(op)) => (false, op.clone()),
            _ => panic!("round {round}: exactly one delete must be accepted, got {a:?} / {b:?}"),
        };
        let _ = (winner_first, winner_op);
        // Poll the end state rather than the winner's event stream: the winner may
        // be the second service (its own channel) and may finish before this
        // thread subscribes.
        wait_for_row_gone(&h.paths, &id, round);

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
    // Two rows are prepared *after* the first service is stopped and *before* the
    // restarted one exists, so only reconciliation decides their fate. Writing
    // them while the old supervisor still looped would let it witness the bogus
    // "dead" row as a death first, which is a different (valid) path.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("adopted", EngineKey::Postgresql, port);
    h.create(spec);
    let port2 = test_port();
    let spec2 = h.spec("dead", EngineKey::Postgresql, port2);
    h.create(spec2);
    let instances = h.instances();
    let adopted = instances
        .iter()
        .find(|i| i.name().as_str() == "adopted")
        .expect("adopted instance");
    let dead = instances
        .iter()
        .find(|i| i.name().as_str() == "dead")
        .expect("dead instance");
    let id = adopted.id().clone();
    let dead_id = dead.id().clone();
    let procs = Arc::clone(&h.procs);

    // The live engine and a dead PID are planted on the home after the old
    // service is gone.
    let prepare_id = id.clone();
    let prepare_dead_id = dead_id.clone();
    let restarted = super::harness::restart_owned_with(h, move |paths| {
        let store = crate::store::Store::open(paths).expect("store");
        // A live process behind `adopted`.
        let identity = procs.spawn("/fake/engine/bin/postgres");
        store
            .update_process_identity(
                &prepare_id,
                Some(identity.pid()),
                Some(identity.started_at()),
                Some(identity.exe()),
            )
            .expect("identity");
        store
            .update_status(&prepare_id, InstanceStatus::Running)
            .expect("status");
        // A dead row, pointing at a PID nobody owns.
        store
            .update_process_identity(
                &prepare_dead_id,
                Some(999_999),
                Some(std::time::SystemTime::UNIX_EPOCH),
                Some(std::path::Path::new("/usr/bin/gone")),
            )
            .expect("dead identity");
        store
            .update_status(&prepare_dead_id, InstanceStatus::Running)
            .expect("status");
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
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
    let (_op, run) = h.run_call(|| h.service.start_instance(&id));
    assert!(matches!(run.result(), Some(Ok(()))));
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
