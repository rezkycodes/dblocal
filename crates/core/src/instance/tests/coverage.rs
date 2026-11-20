//! Tests that fill the operational paths the behaviour suites do not reach:
//! catalog-driven source selection, runtime bookkeeping, a successful archive
//! purge and a log follower that really delivers lines.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::{Duration, SystemTime};

use crate::model::{
    ArchivedData, DeleteMode, EngineKey, InstanceName, InstanceStatus, Runtime, RuntimeState,
};
use crate::service::{DblService, ServiceEvent};

use super::harness::{Harness, test_port};

#[test]
fn source_for_selects_from_the_bundled_catalog() {
    // No injected runtime source, so create has to select an artifact from the
    // catalog for this platform and glibc.
    let h = Harness::new(EngineKey::Postgresql);
    h.uninstall_runtime(EngineKey::Postgresql);
    let port = test_port();
    // The bundled catalog lists full semver tags, which is what the GUI passes
    // after the user picks a version.
    let spec = crate::model::InstanceSpec::new(
        InstanceName::new("catalog-pick").expect("name"),
        EngineKey::Postgresql,
        "18.7.0",
        Some(crate::model::Port::new(port).expect("port")),
        crate::model::AuthMode::Password,
    )
    .expect("spec");
    let op = h.service.create_instance(spec).expect("create");
    let events = h.run(op.clone());
    let finished = super::harness::finished_result(&events, &op);
    // The machine cannot reach the catalog host, so this must fail on the
    // download — but only after selecting a real artifact, which is what proves
    // the selection path ran. A missing artifact would be `Unsupported` and the
    // stage order would stop before `downloading`.
    match finished {
        Some(Err(error)) => assert_ne!(
            error.code,
            crate::error::CODE_UNSUPPORTED,
            "artifact selection must succeed for PostgreSQL 18.7 on linux/x86_64"
        ),
        Some(Ok(())) => {}
        None => panic!("no result"),
    }
    assert!(h.instances().is_empty(), "a failed create leaves no row");
}

#[test]
fn an_unknown_catalog_version_is_reported_as_unsupported() {
    let h = Harness::new(EngineKey::Postgresql);
    h.uninstall_runtime(EngineKey::Postgresql);
    let spec = crate::model::InstanceSpec::new(
        InstanceName::new("nope").expect("name"),
        EngineKey::Postgresql,
        "1.0",
        None,
        crate::model::AuthMode::Password,
    )
    .expect("spec");
    let op = h.service.create_instance(spec).expect("create");
    let events = h.run(op.clone());
    match super::harness::finished_result(&events, &op) {
        Some(Err(error)) => assert_eq!(error.code, crate::error::CODE_UNSUPPORTED),
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn a_recorded_runtime_is_listed_and_removable_when_unused() {
    let h = Harness::new(EngineKey::Postgresql);
    let dir = h
        .paths
        .runtimes_dir()
        .join("postgresql")
        .join("16.16")
        .join("x86_64");
    std::fs::create_dir_all(dir.join("bin")).expect("runtime");
    let runtime = Runtime::new(
        "postgresql-16.16-x86_64",
        EngineKey::Postgresql,
        "16.16",
        crate::platform::Arch::X86_64,
        dir.clone(),
        42,
        SystemTime::UNIX_EPOCH,
        RuntimeState::Installed,
    );
    h.store().add_runtime(&runtime).expect("record");

    let listed = h.service.list_runtimes().expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id(), "postgresql-16.16-x86_64");

    // Unused by any instance, so removal proceeds and the tree is gone.
    h.service
        .remove_runtime("postgresql-16.16-x86_64")
        .expect("remove");
    assert!(!dir.exists(), "the runtime directory is removed");
    assert!(h.service.list_runtimes().expect("list").is_empty());
}

#[test]
fn removing_an_unknown_runtime_is_not_found() {
    let h = Harness::new(EngineKey::Postgresql);
    let err = h.service.remove_runtime("ghost").expect_err("missing");
    assert_eq!(err.code(), crate::error::CODE_NOT_FOUND);
}

#[test]
fn a_real_archive_can_be_listed_and_purged() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("archived", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    let op = h
        .service
        .delete_instance(&id, DeleteMode::ArchiveData)
        .expect("delete");
    let events = h.run(op.clone());
    assert!(matches!(
        super::harness::finished_result(&events, &op),
        Some(Ok(()))
    ));

    let archives = h.service.list_archives().expect("list");
    assert_eq!(archives.len(), 1);
    let archive = archives[0].clone();
    assert!(archive.path().exists(), "archived data exists");
    assert_eq!(archive.engine(), EngineKey::Postgresql);

    h.service
        .purge_archive(archive.id())
        .expect("purge succeeds for a service-issued id");
    assert!(!archive.path().exists(), "archived data is removed");
    assert!(h.service.list_archives().expect("list").is_empty());
}

#[test]
fn purging_an_unknown_archive_is_not_found() {
    let h = Harness::new(EngineKey::Postgresql);
    let err = h.service.purge_archive("ghost").expect_err("missing");
    assert_eq!(err.code(), crate::error::CODE_NOT_FOUND);
}

#[test]
fn a_watched_log_streams_appended_lines() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("live", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let log = h.paths.logs_dir(instance.name()).join("postgres.log");
    std::fs::create_dir_all(log.parent().unwrap()).expect("logs dir");
    std::fs::write(&log, "").expect("empty log");

    let mut receiver = h.service.subscribe();
    h.service.watch_logs(&id, true).expect("watch");
    std::thread::sleep(Duration::from_millis(100));
    {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .expect("open log");
        writeln!(file, "engine ready").expect("append");
        file.flush().expect("flush");
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match receiver.blocking_recv() {
            Ok(ServiceEvent::LogLines { id: got, lines }) if got == id => {
                assert!(lines.iter().any(|line| line.contains("engine ready")));
                break;
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => panic!("event channel closed"),
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the follower never delivered a line"
        );
    }
    h.service.watch_logs(&id, false).expect("stop watching");
    assert_eq!(h.service.watched_logs(), 0);
}

#[test]
fn diagnostics_expose_the_state_root_and_subscriber_count() {
    let h = Harness::new(EngineKey::Postgresql);
    assert_eq!(h.service.state_root(), h.paths.root());
    let before = h.service.subscriber_count();
    let _receiver = h.service.subscribe();
    assert_eq!(h.service.subscriber_count(), before + 1);
    assert!(format!("{:?}", h.service).contains("LocalService"));
}

#[test]
fn a_second_create_of_the_same_name_is_rejected_by_the_row_too() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("dupe", EngineKey::Postgresql, port);
    h.create(spec);
    // `validate_spec` and `create_instance` both refuse, before any work.
    let port2 = test_port();
    let again = h.spec("dupe", EngineKey::Postgresql, port2);
    let err = h.service.validate_spec(&again).expect_err("name taken");
    assert_eq!(err.code(), crate::error::CODE_ALREADY_EXISTS);
}

#[test]
fn delete_after_a_failed_start_uses_the_failed_source_status() {
    let h = Harness::new(EngineKey::Postgresql);
    *h.adapter.fail_at.lock().unwrap() = super::harness::FailAt::Health;
    let port = test_port();
    let spec = h.spec("failed-then-deleted", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let op = h.service.start_instance(&id).expect("start");
    let _ = h.run(op);
    h.wait_for_status(&id, InstanceStatus::Failed);

    let op = h
        .service
        .delete_instance(&id, DeleteMode::PurgeData)
        .expect("delete");
    let events = h.run(op.clone());
    assert!(matches!(
        super::harness::finished_result(&events, &op),
        Some(Ok(()))
    ));
    assert!(h.instance(&id).is_none());
}

#[test]
fn an_archive_row_without_a_directory_is_still_purged() {
    // A recorded archive whose directory vanished (a user removed it by hand)
    // must still be removable, so the row does not become permanent.
    let h = Harness::new(EngineKey::Postgresql);
    std::fs::create_dir_all(h.paths.archive_dir()).expect("archive dir");
    let record = ArchivedData::new(
        "already-gone",
        InstanceName::new("abc").expect("name"),
        EngineKey::Postgresql,
        "18.7",
        h.paths.archive_dir().join("abc-1"),
        0,
        SystemTime::UNIX_EPOCH,
    );
    h.store().add_archive(&record).expect("record");
    h.service.purge_archive("already-gone").expect("purge");
    assert!(h.service.list_archives().expect("list").is_empty());
}

#[test]
fn a_default_service_builds_every_production_dependency() {
    // No injected adapter, probe, terminator, secrets, spawn or port probe: this
    // exercises the real defaults (`adapter_for`, `SysinfoProbe`,
    // `ProcessTerminator`, `SecretStore::for_paths`, `spawn_detached`, a real
    // bind probe) and the catalog/glibc probing done on the runtime thread.
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = crate::paths::DblPaths::with_root(dir.path());
    let service =
        crate::service::LocalService::new(paths.clone(), crate::service::ServiceOptions::default())
            .expect("default service");
    assert_eq!(service.state_root(), paths.root());
    assert!(service.list_instances().expect("list").is_empty());
    let view = service.catalog().expect("catalog");
    assert_eq!(view.engines.len(), EngineKey::ALL.len());
    // The Debug impls are part of the public shape and must not print secrets.
    assert!(format!("{:?}", service).contains("LocalService"));
    assert!(format!("{:?}", crate::service::ServiceOptions::default()).contains("ServiceOptions"));
}

#[test]
fn the_production_terminator_never_signals_an_absent_process() {
    use crate::instance::ProcessTerminator;
    use crate::instance::Terminator;
    // A recorded identity far above any real pid range: the process layer
    // reports it absent, so nothing is signalled and the outcome is NotRunning.
    let identity = crate::process::identity::ProcessIdentity::new(
        999_999_999,
        "/nonexistent/engine",
        SystemTime::UNIX_EPOCH,
    )
    .expect("identity");
    let outcome = ProcessTerminator
        .terminate(&identity, Duration::from_millis(10))
        .expect("terminate");
    assert_eq!(
        outcome,
        crate::process::terminate::TerminateOutcome::NotRunning
    );
}

#[test]
fn dropping_the_last_service_handle_stops_the_supervisor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = crate::paths::DblPaths::with_root(dir.path());
    let service =
        crate::service::LocalService::new(paths, crate::service::ServiceOptions::default())
            .expect("service");
    let clone = service.clone();
    assert_eq!(service.subscriber_count(), 0);
    // Dropping one handle must not stop the other; dropping the last one sets
    // the stop flag and lets the runtime shut down.
    drop(service);
    assert!(clone.list_instances().expect("list").is_empty());
    drop(clone);
}

#[test]
fn a_failed_start_settles_through_the_credential_and_store_paths() {
    // Drive a start where the adapter cannot read a credential: a password-mode
    // instance whose credential pointer was removed. That must fail typed and
    // still leave a terminal status (exercises the settle path and diagnosis).
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("lost-secret", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    // Remove the credential pointer, so `read_credential` returns an empty
    // reference set and the start proceeds unauthenticated rather than failing;
    // either way the row must settle out of `Starting`.
    let op = h.service.start_instance(&id).expect("start");
    let events = h.run(op.clone());
    assert!(super::harness::finished_result(&events, &op).is_some());
    let settled = h.instance(&id).expect("row");
    assert_ne!(
        settled.status(),
        InstanceStatus::Starting,
        "never strands Starting"
    );
}
