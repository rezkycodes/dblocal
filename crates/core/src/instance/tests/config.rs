//! Service-level tests for the config editor (`DblService` path).
//!
//! These run against a real `LocalService` on a temp home with the scripted
//! adapter, so the per-instance guard, the event, the backup write and the
//! `restore_available` flag are exercised end to end without an engine.
//!
//! The harness pre-installs the PostgreSQL and Valkey runtimes; the MySQL and
//! MariaDB `my.cnf` layout is covered by the pure editor tests and the real
//! engine e2e, so no fake runtime is needed here.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::PathBuf;

use crate::config::error as codes;
use crate::model::{EngineKey, InstanceId, InstanceStatus};
use crate::service::DblService;

use super::harness::{FailAt, Harness, ScriptedAdapter};
use super::harness_assert::test_port;

fn created(engine: EngineKey) -> (Harness, InstanceId, PathBuf) {
    let harness = Harness::new(engine);
    let port = test_port();
    let spec = harness.spec("cfg", engine, port);
    harness.create(spec);
    let instance = harness.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let path = match engine {
        EngineKey::Postgresql => harness
            .paths
            .data_dir(instance.name())
            .join("postgresql.conf"),
        EngineKey::Valkey => harness.paths.run_dir(instance.name()).join("valkey.conf"),
        other => panic!("service tests only create PostgreSQL/Valkey, got {other:?}"),
    };
    (harness, id, path)
}

fn seed(path: &std::path::Path, engine: EngineKey) {
    let body = match engine {
        EngineKey::Postgresql => "# base\nmax_connections = 100\n",
        EngineKey::Valkey => "port 6379\nmaxmemory 0\n",
        _ => unreachable!("only pg/valkey"),
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, body).expect("seed");
}

#[test]
fn view_lists_the_editable_subset_with_defaults() {
    let (harness, id, _path) = created(EngineKey::Valkey);
    let view = harness.service.config_view(&id).expect("view");
    let keys: Vec<&str> = view.settings.iter().map(|s| s.key.as_str()).collect();
    assert!(keys.contains(&"maxmemory"));
    assert!(keys.contains(&"maxmemory-policy"));
    let maxmemory = view
        .settings
        .iter()
        .find(|s| s.key == "maxmemory")
        .expect("maxmemory");
    assert_eq!(maxmemory.default, "0");
    assert!(maxmemory.current.is_none(), "not set in the file yet");
    assert!(maxmemory.restart_required);
    let port = view
        .settings
        .iter()
        .find(|s| s.key == "port")
        .expect("port");
    assert!(port.readonly);
}

#[test]
fn apply_validates_first_and_refuses_the_whole_batch() {
    let (harness, id, path) = created(EngineKey::Valkey);
    seed(&path, EngineKey::Valkey);
    let before = fs::read(&path).expect("read");
    let err = harness
        .service
        .config_apply(
            &id,
            vec![
                ("maxmemory".to_owned(), "1gb".to_owned()),
                ("not_a_key".to_owned(), "1".to_owned()),
            ],
        )
        .expect_err("unknown key refuses the batch");
    assert_eq!(err.code, codes::CODE_UNKNOWN_KEY);
    assert_eq!(fs::read(&path).expect("read after"), before);
}

#[test]
fn apply_refuses_a_managed_key() {
    let (harness, id, path) = created(EngineKey::Valkey);
    seed(&path, EngineKey::Valkey);
    let err = harness
        .service
        .config_apply(&id, vec![("port".to_owned(), "6400".to_owned())])
        .expect_err("managed");
    assert_eq!(err.code, codes::CODE_MANAGED_KEY);
}

#[test]
fn apply_takes_a_backup_writes_atomically_and_emits_an_event() {
    let (harness, id, path) = created(EngineKey::Valkey);
    seed(&path, EngineKey::Valkey);
    let original = fs::read(&path).expect("read");
    let mut receiver = harness.receiver();

    let applied = harness
        .service
        .config_apply(&id, vec![("maxmemory".to_owned(), "1gb".to_owned())])
        .expect("apply");
    assert!(applied.restart_required);
    assert_eq!(applied.changed, 1);
    assert_eq!(fs::read(&applied.backup.path).expect("backup"), original);
    let now = fs::read_to_string(&path).expect("read after");
    assert!(now.contains("maxmemory 1gb"), "{now}");
    assert!(now.starts_with("port 6379\n"));

    let mut saw = false;
    while let Ok(event) = receiver.try_recv() {
        if matches!(event, crate::service::ServiceEvent::ConfigChanged { id: event_id } if event_id == id)
        {
            saw = true;
        }
    }
    assert!(saw, "ConfigChanged must be emitted");
}

#[test]
fn a_value_that_would_be_read_as_a_comment_is_refused_and_writes_nothing() {
    let (harness, id, path) = created(EngineKey::Valkey);
    seed(&path, EngineKey::Valkey);
    let before = fs::read(&path).expect("read");
    let err = harness
        .service
        .config_apply(
            &id,
            vec![("maxmemory".to_owned(), "1gb # injected".to_owned())],
        )
        .expect_err("comment injection");
    assert_eq!(err.code, codes::CODE_INVALID_VALUE);
    assert_eq!(fs::read(&path).expect("read after"), before);
}

#[test]
fn restore_puts_the_backup_back() {
    let (harness, id, path) = created(EngineKey::Valkey);
    seed(&path, EngineKey::Valkey);
    let original = fs::read(&path).expect("read");
    harness
        .service
        .config_apply(&id, vec![("maxmemory".to_owned(), "1gb".to_owned())])
        .expect("apply");
    assert_ne!(fs::read(&path).expect("after apply"), original);

    harness.service.config_restore(&id).expect("restore");
    assert_eq!(fs::read(&path).expect("after restore"), original);
}

#[test]
fn restore_without_a_backup_is_a_typed_error() {
    let (harness, id, _path) = created(EngineKey::Valkey);
    let err = harness.service.config_restore(&id).expect_err("no backup");
    assert_eq!(err.code, codes::CODE_NO_BACKUP);
}

#[test]
fn a_restore_is_itself_undoable_because_it_backs_up_the_file_it_replaces() {
    // `F-CFG-01` AC3: after apply(A) then restore, the file the restore replaced
    // is kept as a backup, so a second restore can bring back A.
    let (harness, id, path) = created(EngineKey::Valkey);
    seed(&path, EngineKey::Valkey);
    let original = fs::read(&path).expect("read");
    harness
        .service
        .config_apply(&id, vec![("maxmemory".to_owned(), "1gb".to_owned())])
        .expect("apply A");
    let after_apply = fs::read(&path).expect("after apply");
    assert_ne!(after_apply, original);

    harness.service.config_restore(&id).expect("restore 1");
    assert_eq!(fs::read(&path).expect("after restore 1"), original);

    // Restore again: the newest backup now holds the applied value, so a second
    // restore undoes the first (the restore is undoable).
    harness.service.config_restore(&id).expect("restore 2");
    assert_eq!(
        fs::read(&path).expect("after restore 2"),
        after_apply,
        "the second restore must bring back the applied value"
    );
}

#[test]
fn raw_path_points_at_the_file_the_editor_opens() {
    let (harness, id, path) = created(EngineKey::Postgresql);
    let raw = harness.service.config_raw_path(&id).expect("raw");
    assert_eq!(raw, path);
}

#[test]
fn the_view_never_contains_a_requirepass_value() {
    let (harness, id, path) = created(EngineKey::Valkey);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(&path, "port 6379\nrequirepass hunter2-SECRET\n").expect("write");
    let view = harness.service.config_view(&id).expect("view");
    let debug = format!("{view:?}");
    assert!(!debug.contains("hunter2-SECRET"), "{debug}");
    assert!(!view.settings.iter().any(|s| s.key == "requirepass"));
}

#[test]
fn a_busy_instance_refuses_an_apply_while_a_start_holds_the_guard() {
    let (harness, id, path) = created(EngineKey::Valkey);
    seed(&path, EngineKey::Valkey);
    let _lock = harness.lock_instance(&id);
    let err = harness
        .service
        .config_apply(&id, vec![("maxmemory".to_owned(), "1gb".to_owned())])
        .expect_err("busy");
    assert_eq!(err.code, codes::CODE_BUSY);
}

#[test]
fn a_failed_start_with_a_changed_config_offers_a_restore() {
    let harness = Harness::with_adapter(
        std::sync::Arc::new(ScriptedAdapter::failing(EngineKey::Valkey, FailAt::Health)),
        EngineKey::Valkey,
    );
    let port = test_port();
    harness.create(harness.spec("vk", EngineKey::Valkey, port));
    let instance = harness.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let path = harness.paths.run_dir(instance.name()).join("valkey.conf");
    seed(&path, EngineKey::Valkey);
    harness
        .service
        .config_apply(&id, vec![("maxmemory".to_owned(), "1gb".to_owned())])
        .expect("apply");

    let mut receiver = harness.receiver();
    let op = harness.service.start_instance(&id).expect("start call");
    let _ = harness.run_on(&mut receiver, op);
    harness.wait_for_status(&id, InstanceStatus::Failed);

    let view = harness
        .service
        .list_instances()
        .expect("list")
        .into_iter()
        .find(|v| v.id == id)
        .expect("row");
    let failure = view.failure.expect("failure details");
    assert!(
        failure.restore_available,
        "a changed config with a backup must offer a restore"
    );
}

#[test]
fn postgresql_view_shows_the_authentication_method_from_pg_hba() {
    let (harness, id, _path) = created(EngineKey::Postgresql);
    let name = harness.instances()[0].name().clone();
    let hba = harness.paths.data_dir(&name).join("pg_hba.conf");
    fs::write(
        &hba,
        "# TYPE  DATABASE  USER  ADDRESS  METHOD\n\
         host all all 127.0.0.1/32 scram-sha-256\n\
         host all all ::1/128 scram-sha-256\n\
         host all all 0.0.0.0/0 trust\n",
    )
    .expect("hba");
    let view = harness.service.config_view(&id).expect("view");
    let method = view
        .settings
        .iter()
        .find(|s| s.key == "authentication method")
        .expect("auth method row");
    assert!(method.readonly);
    assert_eq!(method.current.as_deref(), Some("scram-sha-256, trust"));
}

#[test]
fn restore_available_is_false_without_a_file_or_a_backup() {
    let (harness, id, path) = created(EngineKey::Valkey);
    // No file and no backup yet.
    let _ = fs::remove_file(&path);
    let instance = harness.instance(&id).expect("row");
    assert!(!super::super::config_ops::restore_available(
        &harness.service.inner(),
        &instance
    ));
    // File present but never backed up -> false.
    seed(&path, EngineKey::Valkey);
    assert!(!super::super::config_ops::restore_available(
        &harness.service.inner(),
        &instance
    ));
}
