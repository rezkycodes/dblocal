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
        harness.service.inner(),
        &instance
    ));
    // File present but never backed up -> false.
    seed(&path, EngineKey::Valkey);
    assert!(!super::super::config_ops::restore_available(
        harness.service.inner(),
        &instance
    ));
}

// --- F1: config_apply validates the value through the LocalService call site ---
//
// Each case applies an out-of-range integer, a non-integer, an enum outside its
// set and a malformed size, and asserts (a) the typed CODE_INVALID_VALUE, (b) the
// file bytes are unchanged, (c) no backup was taken. Plus a batch whose second
// entry is invalid leaves the first unwritten. These go through LocalService, so
// removing `validate_value(spec, value)?` in config_ops::apply turns them red.

/// Creates an instance of any engine by seeding a fake runtime first, so `create`
/// skips the download path. Returns the harness, id and config path.
fn created_with_runtime(engine: EngineKey) -> (Harness, InstanceId, PathBuf) {
    let harness = Harness::new(engine);
    // Seed the runtime `create` looks for (`<home>/runtimes/<engine>/18.7/<arch>/bin`).
    let dir = harness
        .paths
        .runtime_dir(engine, "18.7", crate::platform::Arch::X86_64)
        .expect("runtime dir");
    fs::create_dir_all(dir.join("bin")).expect("runtime bin");
    let port = test_port();
    let spec = harness.spec("cfg", engine, port);
    harness.create(spec);
    let instance = harness.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let path =
        match engine {
            EngineKey::Postgresql | EngineKey::Mariadb => harness
                .paths
                .data_dir(instance.name())
                .join(if engine == EngineKey::Postgresql {
                    "postgresql.conf"
                } else {
                    "my.cnf"
                }),
            EngineKey::Mysql | EngineKey::Valkey => {
                harness
                    .paths
                    .run_dir(instance.name())
                    .join(if engine == EngineKey::Mysql {
                        "my.cnf"
                    } else {
                        "valkey.conf"
                    })
            }
        };
    (harness, id, path)
}

fn seed_any(path: &std::path::Path, engine: EngineKey) {
    let body = match engine {
        EngineKey::Postgresql => "# base\nmax_connections = 100\n",
        EngineKey::Mysql | EngineKey::Mariadb => "[mysqld]\nport=3307\nmax_connections=151\n",
        EngineKey::Valkey => "port 6379\nmaxmemory 0\n",
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, body).expect("seed");
}

/// The backup directory for the test instance has no files yet.
fn backup_count(harness: &Harness) -> usize {
    let dir = harness.paths.root().join("dblocal-backups").join("cfg");
    fs::read_dir(&dir)
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or(0)
}

/// Family-specific bad values: out-of-range integer, non-integer, bad enum,
/// malformed size. Returns (key, value) pairs.
fn bad_values(engine: EngineKey) -> Vec<(&'static str, &'static str)> {
    match engine {
        EngineKey::Postgresql => vec![
            ("max_connections", "99999999"), // above the max
            ("max_connections", "abc"),      // not an integer
            ("shared_buffers", "12XB"),      // unknown unit
            ("work_mem", "1.5MB"),           // fractional
        ],
        EngineKey::Mysql | EngineKey::Mariadb => vec![
            ("max_connections", "999999999"), // above the max
            ("max_connections", "abc"),
            ("character-set-server", "klingon"), // outside the enum
            ("innodb_buffer_pool_size", "12XB"),
        ],
        EngineKey::Valkey => vec![
            ("timeout", "999999999999"), // above the max
            ("timeout", "abc"),
            ("maxmemory-policy", "bogus"), // outside the enum
            ("maxmemory", "12XB"),
        ],
    }
}

/// Runs the four validation classes for one engine through LocalService.
fn assert_engine_validates(engine: EngineKey) {
    let (harness, id, path) = created_with_runtime(engine);
    seed_any(&path, engine);
    let instance_name = harness.instances()[0].name().clone();
    let _ = instance_name;
    for (key, value) in bad_values(engine) {
        let before = fs::read(&path).expect("read before");
        let backups_before = backup_count(&harness);
        let result = harness
            .service
            .config_apply(&id, vec![(key.to_owned(), value.to_owned())]);
        let err = match result {
            Err(err) => err,
            Ok(_) => panic!("{engine:?} {key}={value:?} must be refused, was accepted"),
        };
        assert_eq!(
            err.code,
            codes::CODE_INVALID_VALUE,
            "{engine:?} {key}={value:?}: {err:?}"
        );
        assert_eq!(
            fs::read(&path).expect("read after"),
            before,
            "{engine:?} {key}={value:?}: the file must be unchanged"
        );
        assert_eq!(
            backup_count(&harness),
            backups_before,
            "{engine:?} {key}={value:?}: no backup may be taken on a refused value"
        );
    }
}

#[test]
fn apply_validates_every_value_through_the_service_postgresql() {
    assert_engine_validates(EngineKey::Postgresql);
}

#[test]
fn apply_validates_every_value_through_the_service_mysql() {
    assert_engine_validates(EngineKey::Mysql);
}

#[test]
fn apply_validates_every_value_through_the_service_mariadb() {
    assert_engine_validates(EngineKey::Mariadb);
}

#[test]
fn apply_validates_every_value_through_the_service_valkey() {
    assert_engine_validates(EngineKey::Valkey);
}

#[test]
fn a_batch_with_one_invalid_entry_writes_nothing() {
    let (harness, id, path) = created_with_runtime(EngineKey::Valkey);
    seed_any(&path, EngineKey::Valkey);
    let before = fs::read(&path).expect("read before");
    let backups_before = backup_count(&harness);
    // First entry is valid, second is invalid: the whole batch must be refused.
    let err = harness
        .service
        .config_apply(
            &id,
            vec![
                ("maxmemory".to_owned(), "1gb".to_owned()),
                ("timeout".to_owned(), "abc".to_owned()),
            ],
        )
        .expect_err("a batch with one invalid entry refuses the whole call");
    assert_eq!(err.code, codes::CODE_INVALID_VALUE);
    assert_eq!(
        fs::read(&path).expect("read after"),
        before,
        "the valid first entry must not be written"
    );
    assert_eq!(backup_count(&harness), backups_before, "no backup");
}

// --- F1 (cont.): restore_available with the marker present ---

#[test]
fn restore_available_is_false_when_the_file_matches_the_recorded_hash() {
    let (harness, id, path) = created_with_runtime(EngineKey::Valkey);
    seed_any(&path, EngineKey::Valkey);
    // A backup must exist for the flag to be considered at all.
    harness
        .service
        .config_apply(&id, vec![("maxmemory".to_owned(), "1gb".to_owned())])
        .expect("apply takes a backup");
    // Record the CURRENT file hash as "last good", then do not touch the file.
    let instance = harness.instance(&id).expect("row");
    super::super::config_ops::record_last_good(harness.service.inner(), &instance);
    assert!(
        !super::super::config_ops::restore_available(harness.service.inner(), &instance),
        "an unchanged file that matches the recorded hash must not offer a restore"
    );
}

#[test]
fn restore_available_is_true_when_the_file_changed_after_the_recorded_hash() {
    let (harness, id, path) = created_with_runtime(EngineKey::Valkey);
    seed_any(&path, EngineKey::Valkey);
    harness
        .service
        .config_apply(&id, vec![("maxmemory".to_owned(), "1gb".to_owned())])
        .expect("apply takes a backup");
    let instance = harness.instance(&id).expect("row");
    // Record the hash, then change the file (an out-of-band edit a start would see).
    super::super::config_ops::record_last_good(harness.service.inner(), &instance);
    harness
        .service
        .config_apply(&id, vec![("maxmemory".to_owned(), "2gb".to_owned())])
        .expect("apply changes the file and adds a backup");
    assert!(
        super::super::config_ops::restore_available(harness.service.inner(), &instance),
        "a file changed since the recorded hash must offer a restore"
    );
}

// --- F2: opening the raw config file through the LocalService seam ---

use std::sync::{Arc, Mutex};

/// Records the open command instead of launching a real editor.
struct RecordingOpen {
    seen: Mutex<Vec<crate::config::OpenCommand>>,
    fail: bool,
}

impl RecordingOpen {
    fn new(fail: bool) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            fail,
        })
    }
}

impl crate::config::OpenLauncher for RecordingOpen {
    fn launch(&self, command: &crate::config::OpenCommand) -> std::io::Result<()> {
        if self.fail {
            return Err(std::io::Error::other("no handler"));
        }
        self.seen.lock().expect("lock").push(command.clone());
        Ok(())
    }
}

fn created_with_launcher(launcher: Arc<RecordingOpen>) -> (Harness, InstanceId, PathBuf) {
    let harness = Harness::with_options(EngineKey::Valkey, move |options| {
        options.open_launcher = Some(launcher as Arc<dyn crate::config::OpenLauncher>);
    });
    let port = test_port();
    let spec = harness.spec("cfg", EngineKey::Valkey, port);
    harness.create(spec);
    let instance = harness.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let path = harness.paths.run_dir(instance.name()).join("valkey.conf");
    (harness, id, path)
}

#[test]
fn open_raw_launches_the_handler_with_the_file_as_one_argument() {
    let launcher = RecordingOpen::new(false);
    let (harness, id, path) = created_with_launcher(Arc::clone(&launcher));
    seed(&path, EngineKey::Valkey);
    let command = harness.service.config_open_raw(&id).expect("open");
    let seen = launcher.seen.lock().expect("lock");
    assert_eq!(seen.len(), 1, "exactly one launch");
    assert_eq!(command, seen[0], "the returned command is the one launched");
    // The path is exactly one argument and never split.
    assert!(
        command
            .args
            .iter()
            .any(|arg| arg == &path.display().to_string()),
        "{:?}",
        command.args
    );
}

#[test]
fn open_raw_refuses_a_missing_file_without_launching() {
    let launcher = RecordingOpen::new(false);
    let (harness, id, path) = created_with_launcher(Arc::clone(&launcher));
    let _ = fs::remove_file(&path);
    let err = harness
        .service
        .config_open_raw(&id)
        .expect_err("missing file is refused");
    assert_eq!(err.code, codes::CODE_MISSING_FILE);
    assert!(
        launcher.seen.lock().expect("lock").is_empty(),
        "nothing opened"
    );
}

#[test]
fn open_raw_reports_a_launcher_failure() {
    let launcher = RecordingOpen::new(true);
    let (harness, id, path) = created_with_launcher(launcher);
    seed(&path, EngineKey::Valkey);
    let err = harness
        .service
        .config_open_raw(&id)
        .expect_err("launcher failed");
    assert_eq!(err.code, codes::CODE_LAUNCHER);
}
