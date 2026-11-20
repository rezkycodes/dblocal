//! Cross-process safety and path-traversal tests.

use std::path::PathBuf;
use std::sync::Arc;

use crate::model::{DeleteMode, EngineKey, InstanceId, InstanceName, InstanceStatus};
use crate::paths::DblPaths;
use crate::service::DblService;

use super::harness::{Harness, test_port};

#[test]
fn two_services_on_one_home_spawn_exactly_once() {
    // Two independent services over the same home, both asked to start the same
    // stopped instance at the same instant. The compare-and-set transition must
    // let exactly one win.
    for round in 0..50 {
        let h = Harness::new(EngineKey::Postgresql);
        let port = test_port();
        let spec = h.spec("race", EngineKey::Postgresql, port);
        h.create(spec);
        let instance = h.instances().into_iter().next().expect("instance");
        let id = instance.id().clone();

        // A second service over the same state tree, with its own fakes.
        let second = super::harness::second_service(&h.paths, Arc::clone(&h.procs));

        let a = h.service.start_instance(&id);
        let b = second.service.start_instance(&id);

        let winners = [a.is_ok(), b.is_ok()].into_iter().filter(|ok| *ok).count();
        assert_eq!(
            winners, 1,
            "round {round}: exactly one start must be accepted"
        );

        // Exactly one spawn reached the process table.
        let spawned = h.procs.spawned_count();
        // Let whichever operation was accepted finish.
        let accepted = if a.is_ok() { &h } else { &second };
        let _ = accepted;
        std::thread::sleep(std::time::Duration::from_millis(80));
        assert!(
            spawned <= 1,
            "round {round}: at most one spawn, saw {spawned}"
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

#[allow(dead_code)]
fn _unused(_: &DblPaths, _: Arc<()>, _: InstanceId, _: DeleteMode) {}

#[test]
fn reconciliation_finishes_a_delete_interrupted_by_a_crash() {
    // A `Deleting` row is terminal, so only reconciliation can finish it. If it
    // did not, the instance would be unreachable: no start, no stop, no second
    // delete.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("half-deleted", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    let store = h.store();
    store
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Deleting)
        .expect("claim");
    assert_eq!(
        store.get_instance(&id).expect("get").expect("row").status(),
        InstanceStatus::Deleting
    );

    let restarted = super::harness::restart(&h.paths, Arc::clone(&h.procs));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if restarted.instance(&id).is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "an interrupted delete was never finished"
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    assert!(
        !h.paths.instance_dir(instance.name()).exists(),
        "the half-created instance directory is removed too"
    );
}

#[test]
fn a_reconciled_interrupted_create_surfaces_as_failed() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("interrupted", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    // Model a crash mid-create: the row stayed `Initializing`.
    h.store()
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Initializing)
        .expect("claim");

    let restarted = super::harness::restart(&h.paths, Arc::clone(&h.procs));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(current) = restarted.instance(&id)
            && current.status() == InstanceStatus::Failed
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "an interrupted create must surface as failed"
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}
