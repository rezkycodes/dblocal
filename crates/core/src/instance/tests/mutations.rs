//! Small mutations, event sequences and early-exit handling.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use crate::model::{DeleteMode, EngineKey, InstanceStatus};
use crate::service::{DblService, ServiceEvent};

use super::harness::{FailAt, FakeProcs, Harness, test_port};
use crate::process::probe::ProcessProbe;

fn start_and_wait(h: &Harness, id: &crate::model::InstanceId) {
    let (_op, run) = h.run_call(|| h.service.start_instance(id));
    assert!(
        matches!(run.result(), Some(Ok(()))),
        "start: {:?}",
        run.result()
    );
    h.wait_for_status(id, InstanceStatus::Running);
}

#[test]
fn an_engine_that_exits_immediately_is_diagnosed_and_failed() {
    // Spawn returns an identity, but the process is gone by the time the health
    // poll looks: that is an early exit, which must be diagnosed and failed.
    let procs = Arc::new(FakeProcs::new());
    let h = Harness::with_options(EngineKey::Postgresql, {
        let procs = Arc::clone(&procs);
        move |options| {
            options.probe =
                Some(Arc::clone(&procs)
                    as Arc<dyn crate::process::probe::ProcessProbe + Send + Sync>);
            options.spawn = Some(Arc::new(move |_command, _log| {
                // Return a live identity, then kill it so the poll sees it gone.
                let identity = procs.spawn("/fake/engine/bin/postgres");
                procs.kill(identity.pid());
                Ok(identity)
            }));
        }
    });
    let port = test_port();
    let spec = h.spec("early-exit", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    let (_op, run) = h.run_call(|| h.service.start_instance(&id));
    let finished = run.result();
    assert!(
        matches!(finished, Some(Err(_))),
        "an immediate exit must fail: {finished:?}"
    );
    let failed = h.wait_for_status(&id, InstanceStatus::Failed);
    assert!(
        failed.pid().is_none(),
        "no identity survives a failed start"
    );
    let views = h.service.list_instances().expect("list");
    assert!(
        views[0].failure.is_some(),
        "the failure is carried on the view"
    );
}

#[test]
fn create_start_stop_delete_emit_the_expected_instance_changes() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("sequences", EngineKey::Postgresql, port);
    let mut receiver = h.service.subscribe();
    let op = h.service.create_instance(spec).expect("create");
    let create_run = h.run_on(&mut receiver, op);
    let create_events = &create_run.events;
    let created_changes = statuses(create_events);
    assert!(
        created_changes.contains(&InstanceStatus::Initializing)
            && created_changes.contains(&InstanceStatus::Stopped),
        "create moves through initializing to stopped: {created_changes:?}"
    );

    let id = h
        .instances()
        .into_iter()
        .next()
        .expect("instance")
        .id()
        .clone();
    let op = h.service.start_instance(&id).expect("start");
    let start_run = h.run_on(&mut receiver, op);
    let start_changes = statuses(&start_run.events);
    assert_eq!(
        start_changes.first().copied(),
        Some(InstanceStatus::Starting),
        "Starting is persisted before the spawn: {start_changes:?}"
    );
    assert_eq!(start_changes.last().copied(), Some(InstanceStatus::Running));

    let op = h.service.stop_instance(&id).expect("stop");
    let stop_run = h.run_on(&mut receiver, op);
    let stop_changes = statuses(&stop_run.events);
    assert_eq!(
        stop_changes.first().copied(),
        Some(InstanceStatus::Stopping),
        "Stopping is persisted before the signal: {stop_changes:?}"
    );
    assert_eq!(stop_changes.last().copied(), Some(InstanceStatus::Stopped));

    let op = h
        .service
        .delete_instance(&id, DeleteMode::PurgeData)
        .expect("delete");
    let delete_run = h.run_on(&mut receiver, op);
    let delete_changes = statuses(&delete_run.events);
    assert!(
        delete_changes.contains(&InstanceStatus::Deleting),
        "Deleting is persisted before the removal: {delete_changes:?}"
    );
}

fn statuses(events: &[ServiceEvent]) -> Vec<InstanceStatus> {
    events
        .iter()
        .filter_map(|event| match event {
            ServiceEvent::InstanceChanged { view } => Some(view.status),
            _ => None,
        })
        .collect()
}

#[test]
fn change_port_moves_a_stopped_instance_and_refuses_a_taken_one() {
    let h = Harness::new(EngineKey::Postgresql);
    let first = h.spec("first", EngineKey::Postgresql, test_port());
    h.create(first);
    let second = h.spec("second", EngineKey::Postgresql, test_port());
    h.create(second);

    let instances = h.instances();
    let one = instances
        .iter()
        .find(|i| i.name().as_str() == "first")
        .expect("first");
    let two = instances
        .iter()
        .find(|i| i.name().as_str() == "second")
        .expect("second");

    // Moving onto another instance's port is refused.
    let err = h
        .service
        .change_port(one.id(), two.port())
        .expect_err("port taken by another instance");
    assert_eq!(err.code(), crate::error::CODE_PORT_IN_USE);

    // Moving onto a free port works and releases the old allocation.
    let free = crate::model::Port::new(test_port()).expect("port");
    h.service.change_port(one.id(), free).expect("move");
    let moved = h.instance(one.id()).expect("moved");
    assert_eq!(moved.port(), free);
    let suggestion = h
        .service
        .suggest_port(EngineKey::Postgresql)
        .expect("suggest");
    assert_ne!(suggestion, free, "the new allocation is held");
}

#[test]
fn change_port_is_refused_while_running() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("running", EngineKey::Postgresql, port);
    h.create(spec);
    let id = h
        .instances()
        .into_iter()
        .next()
        .expect("instance")
        .id()
        .clone();
    start_and_wait(&h, &id);

    let free = crate::model::Port::new(test_port()).expect("port");
    let err = h.service.change_port(&id, free).expect_err("must refuse");
    assert_eq!(err.code(), crate::error::CODE_INVALID_TRANSITION);
}

#[test]
fn change_port_rewrites_postgres_config_and_start_repairs_stale_config() {
    // M10: exercise the real managed-block branch written by configure_after_init,
    // not a hand-made legacy config.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("pgcfg", EngineKey::Postgresql, port);
    h.create(spec);
    let inst = h
        .instances()
        .into_iter()
        .find(|i| i.name().as_str() == "pgcfg")
        .expect("instance");
    let id = inst.id().clone();
    let name = crate::model::InstanceName::new("pgcfg").expect("name");
    let data_dir = h.paths.data_dir(&name);
    let run_dir = h.paths.run_dir(&name);
    std::fs::create_dir_all(&data_dir).expect("data dir");
    std::fs::create_dir_all(&run_dir).expect("run dir");
    let conf = data_dir.join("postgresql.conf");

    // Seed a realistic initdb-shaped base config, including a user override
    // OUTSIDE the managed block (`port = 5433` and a commented listen line).
    // The managed-block branch must replace only the block and leave every
    // surrounding user line byte-identical; the legacy in-place scanner would
    // rewrite the user's `port = 5433` too and leave two `port` lines, which is
    // exactly what this test now distinguishes (M10 survived before: with no
    // user `port` line the two paths produced the same bytes).
    let base =
        "# initdb base\nmax_connections = 42\nport = 5433\n#listen_addresses = 'localhost'\n";
    std::fs::write(&conf, base).expect("base conf");
    let ctx = crate::engine::InitContext {
        instance_name: "pgcfg".into(),
        runtime_dir: h.paths.runtimes_dir(),
        data_dir: data_dir.clone(),
        port: crate::model::Port::new(port).expect("port"),
        superuser: "postgres".into(),
        auth: crate::model::AuthMode::None,
        password: None,
    };
    let socket = crate::engine::choose_postgres_socket_dir(&run_dir.join("sock"), port);
    crate::engine::configure_postgres_after_init(&ctx, &socket).expect("configure_after_init");
    let configured = std::fs::read_to_string(&conf).expect("configured");
    assert!(
        configured.starts_with(base),
        "the seeded base must survive as-is"
    );
    assert!(configured.contains("# DBLocal managed settings"));
    assert!(configured.contains(&format!("port = {port}")));

    let free = crate::model::Port::new(test_port()).expect("port");
    crate::engine::sync_postgres_port(&data_dir, &run_dir, free.get()).expect("sync_port");
    let text = std::fs::read_to_string(&conf).expect("read conf");
    let before_block = text
        .split(
            "# DBLocal managed settings
",
        )
        .next()
        .expect("prefix");
    assert_eq!(
        before_block,
        format!("{base}\n"),
        "content before the managed block must be byte-identical afterwards"
    );
    assert_eq!(
        text.matches(
            "# DBLocal managed settings
"
        )
        .count(),
        1,
        "exactly one managed block start"
    );
    assert_eq!(
        text.matches("# DBLocal managed settings end").count(),
        1,
        "exactly one managed block end"
    );
    // Exactly one `port = <new>` line: the block's. The user override above
    // stays at 5433, so a legacy rewrite that touched it would make this two.
    assert_eq!(
        text.matches(&format!("port = {}", free.get())).count(),
        1,
        "the new port appears in exactly one place: {text}"
    );
    assert!(
        text.contains("port = 5433"),
        "the user override outside the block must not be rewritten: {text}"
    );
    assert!(text.contains("listen_addresses = '127.0.0.1'"));
    assert!(!text.contains(&format!("port = {port}\n")));

    // Idempotent: a second call is a no-op.
    crate::engine::sync_postgres_port(&data_dir, &run_dir, free.get()).expect("sync again");
    let again = std::fs::read_to_string(&conf).expect("read again");
    assert_eq!(text, again, "second sync_port must be a no-op");

    // Start repairs a stale config (crash between config rewrite and store update).
    let stale = again.replace(&format!("port = {}", free.get()), "port = 1");
    std::fs::write(&conf, stale).expect("write stale");
    h.service
        .change_port(&id, free)
        .expect("change port updates store");
    start_and_wait(&h, &id);
    let repaired = std::fs::read_to_string(&conf).expect("read repaired");
    assert!(
        repaired.contains(&format!("port = {}", free.get())),
        "start must repair stale config: {repaired}"
    );
}

#[test]
fn change_port_rewrites_valkey_config() {
    let h = Harness::new(EngineKey::Valkey);
    let port = test_port();
    let spec = h.spec("vkcfg", EngineKey::Valkey, port);
    h.create(spec);
    let inst = h
        .instances()
        .into_iter()
        .find(|i| i.name().as_str() == "vkcfg")
        .expect("instance");
    let id = inst.id().clone();
    let name = crate::model::InstanceName::new("vkcfg").expect("name");
    let run_dir = h.paths.run_dir(&name);
    std::fs::create_dir_all(&run_dir).expect("run dir");
    let conf = run_dir.join("valkey.conf");
    std::fs::write(&conf, format!("bind 127.0.0.1\nport {port}\n# keep\n")).expect("write conf");

    let free = crate::model::Port::new(test_port()).expect("port");
    h.service.change_port(&id, free).expect("change port");
    let text = std::fs::read_to_string(&conf).expect("read conf");
    assert!(text.contains(&format!("port {}", free.get())));
    assert!(!text.contains(&format!("port {port}\n")));
    assert!(text.contains("# keep"));
}

#[test]
fn change_port_legacy_postgres_layout_rewrites_correctly() {
    // M10: keep one legacy-layout test, correctly built with format!.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("pglegacy", EngineKey::Postgresql, port);
    h.create(spec);
    let inst = h
        .instances()
        .into_iter()
        .find(|i| i.name().as_str() == "pglegacy")
        .expect("instance");
    let id = inst.id().clone();
    let name = crate::model::InstanceName::new("pglegacy").expect("name");
    let conf = h.paths.data_dir(&name).join("postgresql.conf");
    let legacy = format!(
        "# user\nmax_connections = 42\nport = {port}\nlisten_addresses = '127.0.0.1'\nunix_socket_directories = '/tmp/s'\n"
    );
    std::fs::write(&conf, legacy).expect("write conf");

    let free = crate::model::Port::new(test_port()).expect("port");
    h.service.change_port(&id, free).expect("change port");
    let text = std::fs::read_to_string(&conf).expect("read conf");
    assert!(text.contains(&format!("port = {}", free.get())));
    assert!(!text.contains(&format!("port = {port}\n")));
    assert!(text.contains("max_connections = 42"));
}

#[test]
fn data_size_and_autostart_read_and_write_state() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("sizes", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    // The adapter wrote PG_VERSION into the data directory.
    let size = h.service.data_size(&id).expect("size");
    assert!(size > 0, "the data directory has content");

    assert!(!instance.autostart());
    h.service.set_autostart(&id, true).expect("autostart on");
    assert!(h.instance(&id).expect("row").autostart());
    h.service.set_autostart(&id, false).expect("autostart off");
    assert!(!h.instance(&id).expect("row").autostart());
}

#[test]
fn tail_logs_returns_what_the_engine_wrote() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("logs", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let log = h.paths.logs_dir(instance.name()).join("postgres.log");
    std::fs::create_dir_all(log.parent().unwrap()).expect("logs dir");
    std::fs::write(&log, "line-1\nline-2\nline-3\n").expect("write log");

    let lines = h.service.tail_logs(&id, 2).expect("tail");
    assert_eq!(lines, vec!["line-2", "line-3"]);
}

#[test]
fn cancelling_an_unknown_operation_is_a_typed_not_found() {
    let h = Harness::new(EngineKey::Postgresql);
    let err = h
        .service
        .cancel_operation(&crate::service::OperationId::new("op-9999"))
        .expect_err("unknown operation");
    assert_eq!(err.code(), crate::error::CODE_NOT_FOUND);
}

#[test]
fn a_missing_instance_is_reported_not_found() {
    let h = Harness::new(EngineKey::Postgresql);
    let id = crate::model::InstanceId::new();
    assert_eq!(
        h.service.start_instance(&id).expect_err("missing").code(),
        crate::error::CODE_NOT_FOUND
    );
    assert_eq!(
        h.service.data_size(&id).expect_err("missing").code(),
        crate::error::CODE_NOT_FOUND
    );
}

#[test]
fn a_stop_whose_kill_is_refused_keeps_the_row_running_and_the_identity() {
    // The graceful request fails, and the terminator refuses (a mismatched
    // identity). The engine is still alive, so the honest row is `Running` with
    // its identity intact: clearing it would orphan a live engine DBLocal could
    // never stop again, which is exactly the defect this test used to bless.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("stuck", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    // A live identity is needed for the terminator to be consulted at all.
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
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Running)
        .expect("running");
    *h.adapter.fail_at.lock().unwrap() = FailAt::Shutdown;
    h.terminator
        .refuse
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let (op, run) = h.run_call(|| h.service.stop_instance(&id));
    let events = &run.events;
    assert!(
        matches!(run.result(), Some(Err(_))),
        "a refused kill surfaces as a failure op {op}: {:#?}",
        super::harness::describe(events)
    );

    let settled = h.instance(&id).expect("row");
    assert_eq!(
        settled.status(),
        InstanceStatus::Running,
        "an engine that survived must stay Running, not be called Stopped"
    );
    assert_eq!(
        settled.pid(),
        Some(identity.pid()),
        "the identity must be kept so a retry can still stop the engine"
    );
    // The engine is still in the fake table, so a retry is meaningful.
    assert!(
        matches!(
            h.procs.observe(identity.pid()),
            crate::process::probe::ProcessObservation::Found { .. }
        ),
        "the refused process must still be running"
    );
}

#[test]
fn a_stop_still_runs_when_the_credential_cannot_be_read() {
    // A locked keychain removes the graceful path (the adapter cannot
    // authenticate) but must never prevent escalation through the process layer.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("locked", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let (_op, start_run) = h.run_call(|| h.service.start_instance(&id));
    assert!(matches!(start_run.result(), Some(Ok(()))));
    h.wait_for_status(&id, InstanceStatus::Running);

    h.secrets
        .unreadable
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (op, run) = h.run_call(|| h.service.stop_instance(&id));
    let events = &run.events;
    assert!(
        matches!(run.result(), Some(Ok(()))),
        "an unreadable credential must not block a stop op {op}: {:#?}",
        super::harness::describe(events)
    );
    let stopped = h.wait_for_status(&id, InstanceStatus::Stopped);
    assert!(stopped.pid().is_none());
    assert!(
        h.terminator.calls.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "the process layer was still asked to stop it"
    );
}

#[test]
fn change_port_config_rewrite_failure_leaves_store_and_file_unchanged_postgres() {
    // Item 5: if the config rewrite fails, the store port and the config file must
    // remain unchanged. We force failure by making postgresql.conf a directory.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("pgfail", EngineKey::Postgresql, port);
    h.create(spec);
    let inst = h
        .instances()
        .into_iter()
        .find(|i| i.name().as_str() == "pgfail")
        .expect("instance");
    let id = inst.id().clone();
    let name = crate::model::InstanceName::new("pgfail").expect("name");
    let conf = h.paths.data_dir(&name).join("postgresql.conf");
    std::fs::create_dir_all(&conf).expect("make conf a directory to force rewrite failure");

    let before = h.instance(&id).expect("row").port();
    let free = crate::model::Port::new(test_port()).expect("port");
    let err = h
        .service
        .change_port(&id, free)
        .expect_err("rewrite must fail");
    assert!(
        matches!(
            err,
            crate::DblError::Engine { .. } | crate::DblError::Io { .. }
        ),
        "expected typed engine/io error, got {err:?}"
    );
    let after = h.instance(&id).expect("row").port();
    assert_eq!(
        after, before,
        "store port must be unchanged on rewrite failure"
    );
    assert!(
        conf.is_dir(),
        "config path must remain the directory we created"
    );
}

#[test]
fn change_port_config_rewrite_failure_leaves_store_and_file_unchanged_valkey() {
    let h = Harness::new(EngineKey::Valkey);
    let port = test_port();
    let spec = h.spec("vkfail", EngineKey::Valkey, port);
    h.create(spec);
    let inst = h
        .instances()
        .into_iter()
        .find(|i| i.name().as_str() == "vkfail")
        .expect("instance");
    let id = inst.id().clone();
    let name = crate::model::InstanceName::new("vkfail").expect("name");
    let conf = h.paths.run_dir(&name).join("valkey.conf");
    std::fs::create_dir_all(&conf).expect("make conf a directory to force rewrite failure");

    let before = h.instance(&id).expect("row").port();
    let free = crate::model::Port::new(test_port()).expect("port");
    let err = h
        .service
        .change_port(&id, free)
        .expect_err("rewrite must fail");
    assert!(
        matches!(
            err,
            crate::DblError::Engine { .. } | crate::DblError::Io { .. }
        ),
        "expected typed engine/io error, got {err:?}"
    );
    let after = h.instance(&id).expect("row").port();
    assert_eq!(
        after, before,
        "store port must be unchanged on rewrite failure"
    );
    assert!(
        conf.is_dir(),
        "config path must remain the directory we created"
    );
}

#[test]
fn start_repairs_config_rewritten_but_store_not_updated_postgres() {
    // Item 5 crash simulation: config rewritten to new port, store not updated.
    // start() must sync config from the store (source of truth) before spawning.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("pgcrash", EngineKey::Postgresql, port);
    h.create(spec);
    let inst = h
        .instances()
        .into_iter()
        .find(|i| i.name().as_str() == "pgcrash")
        .expect("instance");
    let id = inst.id().clone();
    let name = crate::model::InstanceName::new("pgcrash").expect("name");
    let data_dir = h.paths.data_dir(&name);
    let run_dir = h.paths.run_dir(&name);
    std::fs::create_dir_all(&data_dir).expect("data");
    std::fs::create_dir_all(&run_dir).expect("run");
    let conf = data_dir.join("postgresql.conf");
    std::fs::write(
        &conf,
        "# base\n# DBLocal managed settings\nlisten_addresses = '127.0.0.1'\nport = 1\nunix_socket_directories = ''\n# DBLocal managed settings end\n",
    )
    .expect("write stale wrong-port conf");

    // Store still has original port; start must repair config to store port.
    start_and_wait(&h, &id);
    let repaired = std::fs::read_to_string(&conf).expect("read");
    assert!(
        repaired.contains(&format!("port = {port}")),
        "start must repair config from store port: {repaired}"
    );
    assert!(
        !repaired.contains("port = 1\n"),
        "stale port must be gone: {repaired}"
    );
}

#[test]
fn start_repairs_config_rewritten_but_store_not_updated_valkey() {
    // Item 5 crash simulation for Valkey.
    let h = Harness::new(EngineKey::Valkey);
    let port = test_port();
    let spec = h.spec("vkcrash", EngineKey::Valkey, port);
    h.create(spec);
    let inst = h
        .instances()
        .into_iter()
        .find(|i| i.name().as_str() == "vkcrash")
        .expect("instance");
    let id = inst.id().clone();
    let name = crate::model::InstanceName::new("vkcrash").expect("name");
    let run_dir = h.paths.run_dir(&name);
    std::fs::create_dir_all(&run_dir).expect("run");
    let conf = run_dir.join("valkey.conf");
    std::fs::write(&conf, "bind 127.0.0.1\nport 1\n# keep\n").expect("write stale conf");

    start_and_wait(&h, &id);
    let repaired = std::fs::read_to_string(&conf).expect("read");
    assert!(
        repaired.contains(&format!("port {port}\n")),
        "start must repair config from store port: {repaired}"
    );
    assert!(
        !repaired.contains("port 1\n"),
        "stale port must be gone: {repaired}"
    );
}

#[test]
fn change_port_check_then_act_window_is_documented() {
    // Item 9: there is a pre-existing check-then-act window in change_port. It checks
    // holds_process()/port availability then writes; a concurrent start from another
    // process can interleave. We do not redesign it. This test documents the current
    // single-process behaviour: once a row is Running, change_port is refused by
    // holds_process(), which is the guard the service relies on. Cross-process TOCTOU
    // remains possible because port probing and config rewrite are not atomic with the
    // store CAS; a store compare-and-set on port+status could cover it in a future
    // redesign.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("toctou", EngineKey::Postgresql, port);
    h.create(spec);
    let inst = h
        .instances()
        .into_iter()
        .find(|i| i.name().as_str() == "toctou")
        .expect("instance");
    let id = inst.id().clone();
    start_and_wait(&h, &id);
    let free = crate::model::Port::new(test_port()).expect("port");
    let err = h
        .service
        .change_port(&id, free)
        .expect_err("running instance refused");
    assert_eq!(err.code(), crate::error::CODE_INVALID_TRANSITION);
}

/// Item 5(a): `finish_operation` must put `OperationFinished` in the channel
/// **before** it records the durable registry result.
///
/// A consumer may stop polling the registry the moment it reports "finished" and
/// then drain its channel; it must find the terminal event.
///
/// The seam lives inside `Ops::finish`, at the exact instant the durable result is
/// recorded, and owns the receiver. In the fixed order (broadcast, then record) the
/// event is already in the channel when the seam runs; with the swapped order
/// (record, then broadcast) the seam drains an empty channel. It sits in `Ops::finish`
/// rather than at the end of `finish_operation` so the plain two-statement swap, not
/// only a swap that also moves the seam, is caught.
#[test]
fn a_finished_operation_is_already_in_the_channel_when_the_registry_says_so() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("finish-order", EngineKey::Postgresql, port);

    // Subscribe before the create, so the hook's receiver is the one that would see
    // the terminal event. The hook fires inside `Ops::finish`, at the instant the
    // durable result is recorded, and owns the receiver.
    let mut receiver = h.receiver();
    let result: Arc<std::sync::Mutex<Option<bool>>> = Arc::new(std::sync::Mutex::new(None));
    let result_hook = Arc::clone(&result);
    h.service.inner().ops.on_finish_recorded(move |op| {
        let op = op.clone();
        let mut found = false;
        loop {
            match receiver.try_recv() {
                Ok(ServiceEvent::OperationFinished { id, .. }) if id == op => {
                    found = true;
                    break;
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
            }
        }
        *result_hook.lock().expect("result") = Some(found);
    });

    let (_op, run) = h.run_call(|| h.service.create_instance(spec));
    assert!(
        matches!(run.result(), Some(Ok(()))),
        "create: {:?}",
        run.result()
    );

    // `h.run_call` returns as soon as it drains `OperationFinished`, which the
    // service emits *before* it calls `Ops::finish`; the hook therefore may not have
    // run yet. Wait for it, bounded, so a slow hook is not a spurious failure but a
    // hook that never runs is.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let found = loop {
        if let Some(found) = *result.lock().expect("result") {
            break found;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the finish hook must have run for this create"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert!(
        found,
        "the registry reported the operation finished before the channel held \
         OperationFinished: finish_operation must broadcast before recording"
    );
}

/// Finding 2: an operation must not still hold its instance lock when it announces
/// `OperationFinished`. `create_instance` used to release it only after
/// `finish_operation`, so a consumer that reacts to the create finishing by starting
/// the new instance (the GUI's auto-start, or a CLI chaining create then start) was
/// refused as busy (`DBL-1004`). Start/stop/delete already drop first.
///
/// The `Ops::finish` seam fires as the terminal result is recorded, i.e. right at the
/// announcement; taking the instance lock there must succeed. Red on the old order
/// (the lock is still held as an operation guard, which refuses at once), green now.
#[test]
fn create_announces_completion_with_its_instance_lock_released() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("release-at-finish", EngineKey::Postgresql, port);
    let inner = Arc::clone(h.service.inner());

    let outcome: Arc<std::sync::Mutex<Option<bool>>> = Arc::new(std::sync::Mutex::new(None));
    let outcome_hook = Arc::clone(&outcome);
    let ops = inner.ops.clone();
    let ops_for_hook = ops.clone();
    ops.on_finish_recorded(move |_op| {
        // Try to take the instance lock at the instant completion is announced. The
        // test home has exactly one instance (this create's row).
        let row = inner
            .db(|store| store.list_instances())
            .ok()
            .and_then(|rows| rows.into_iter().next());
        let free = match row {
            Some(instance) => ops_for_hook.lock_instance(instance.id()).is_ok(),
            None => true,
        };
        *outcome_hook.lock().expect("outcome") = Some(free);
    });

    let mut receiver = h.receiver();
    let op = h.service.create_instance(spec).expect("create");
    let run = h.run_on(&mut receiver, op);
    assert!(
        matches!(run.result(), Some(Ok(()))),
        "create: {:?}",
        run.result()
    );

    // The seam fires inside `Ops::finish`, which runs after `h.run_on` may return;
    // wait for it bounded.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let free = loop {
        if let Some(free) = *outcome.lock().expect("outcome") {
            break free;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the finish seam must have run for this create"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert!(
        free,
        "create still held its instance lock when it announced OperationFinished; a \
         consumer chaining start on the event is refused as busy"
    );
}
