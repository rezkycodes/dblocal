use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use rusqlite::params;
use tempfile::tempdir;

use crate::DblError;
use crate::error::CODE_STORAGE;
use crate::model::{
    ArchivedData, AuthMode, CredentialBackend, CredentialRef, EngineKey, Instance, InstanceId,
    InstanceName, InstanceSpec, InstanceStatus, Port, Runtime, RuntimeState,
};
use crate::paths::DblPaths;
use crate::platform::Arch;

use super::Store;

fn spec(name: &str, port: u16) -> InstanceSpec {
    InstanceSpec::new(
        InstanceName::new(name).expect("name"),
        EngineKey::Postgresql,
        "18.7",
        Some(Port::new(port).expect("port")),
        AuthMode::Password,
    )
    .expect("spec")
}

fn instance(name: &str, port: u16, status: InstanceStatus) -> Instance {
    let spec = spec(name, port);
    Instance::new(
        &spec,
        InstanceId::new(),
        status,
        UNIX_EPOCH + Duration::from_secs(10),
    )
}

fn runtime(id: &str, engine: EngineKey, version: &str, size: u64) -> Runtime {
    Runtime::new(
        id,
        engine,
        version,
        Arch::X86_64,
        PathBuf::from(format!("/runtimes/{id}")),
        size,
        UNIX_EPOCH + Duration::from_secs(20),
        RuntimeState::Installed,
    )
}

fn archive(id: &str, name: &str) -> ArchivedData {
    ArchivedData::new(
        id,
        InstanceName::new(name).expect("name"),
        EngineKey::Postgresql,
        "18.7",
        PathBuf::from(format!("/archive/{id}")),
        4096,
        UNIX_EPOCH + Duration::from_secs(30),
    )
}

#[test]
fn migration_from_empty_is_idempotent_on_reopen() {
    let dir = tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    {
        let store = Store::open(&paths).expect("open");
        assert_eq!(store.list_instances().expect("list").len(), 0);
    }
    let store = Store::open(&paths).expect("reopen");
    assert_eq!(store.list_instances().expect("list").len(), 0);
    let version: i32 = store
        .connection()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version");
    assert_eq!(version, super::schema::LATEST_USER_VERSION);
}

#[test]
fn open_creates_root_dir_with_private_mode_on_unix() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().join("state");
    let paths = DblPaths::with_root(&root);
    let store = Store::open(&paths).expect("open");
    assert!(root.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&root).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    // Verify the mandated pragmas on a disk-backed connection.
    let journal: String = store
        .connection()
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("journal_mode");
    assert_eq!(journal.to_ascii_lowercase(), "wal");
    let fk: i32 = store
        .connection()
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .expect("foreign_keys");
    assert_eq!(fk, 1);
    let sync: i32 = store
        .connection()
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .expect("synchronous");
    // NORMAL == 1
    assert_eq!(sync, 1);
}

#[test]
fn future_user_version_is_refused_with_stable_storage_error() {
    let dir = tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    {
        let store = Store::open(&paths).expect("open");
        store
            .connection()
            .pragma_update(None, "user_version", 999_i32)
            .expect("bump");
    }
    let err = Store::open(&paths).expect_err("must refuse");
    assert!(
        matches!(err, DblError::Storage { .. }),
        "expected Storage, got {err:?}"
    );
    assert_eq!(err.code(), CODE_STORAGE);
}

#[test]
fn unique_name_is_case_insensitive_and_unique_port_maps_to_typed_error() {
    let store = Store::open_in_memory().expect("store");
    let first = instance("app-dev", 5432, InstanceStatus::Stopped);
    store.insert_instance(&first).expect("insert");

    // A differently-cased name must collide at the database level (COLLATE NOCASE).
    // The domain model requires lowercase names, so the colliding row is inserted
    // with raw SQL to exercise the constraint and its typed mapping.
    let err = store
        .connection()
        .execute(
            "INSERT INTO instances (
                id, name, engine, version, port, status, auth, superuser,
                pid, process_started_at_secs, process_started_at_nanos, process_exe,
                autostart, created_at_secs, created_at_nanos
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL, NULL, NULL, 0, 0, 0)",
            params![
                InstanceId::new().as_str(),
                "APP-DEV",
                "postgresql",
                "18.7",
                5433_i64,
                "stopped",
                "password",
                "postgres",
            ],
        )
        .expect_err("case-insensitive duplicate name must be rejected");
    assert!(
        matches!(
            super::error::constraint_target(&err),
            Some(super::error::Constraint::InstanceName)
        ),
        "expected instance name constraint, got {err:?}"
    );

    let dup_port = instance("other", 5432, InstanceStatus::Stopped);
    let err = store.insert_instance(&dup_port).expect_err("dup port");
    assert!(matches!(err, DblError::AlreadyExists { .. }), "{err:?}");
}

#[test]
fn instance_crud_round_trips_and_updates() {
    let store = Store::open_in_memory().expect("store");
    let mut inst = instance("app-dev", 5432, InstanceStatus::Stopped).with_autostart(true);
    store.insert_instance(&inst).expect("insert");

    let loaded = store
        .get_instance(inst.id())
        .expect("get")
        .expect("present");
    assert_eq!(loaded, inst);

    let by_name = store
        .get_instance_by_name("APP-DEV")
        .expect("by name")
        .expect("present");
    assert_eq!(by_name.id(), inst.id());

    let names: Vec<_> = store
        .list_instances()
        .expect("list")
        .into_iter()
        .map(|i| i.name().as_str().to_owned())
        .collect();
    assert_eq!(names, vec!["app-dev"]);

    store
        .update_status(inst.id(), InstanceStatus::Running)
        .expect("status");
    inst = store
        .get_instance(inst.id())
        .expect("get")
        .expect("present");
    assert_eq!(inst.status(), InstanceStatus::Running);

    let started = UNIX_EPOCH + Duration::from_secs(100);
    let exe = PathBuf::from("/usr/bin/postgres");
    store
        .update_process_identity(inst.id(), Some(42), Some(started), Some(exe.as_path()))
        .expect("pid");
    inst = store
        .get_instance(inst.id())
        .expect("get")
        .expect("present");
    assert_eq!(inst.pid(), Some(42));
    assert_eq!(inst.process_started_at(), Some(started));
    assert_eq!(inst.process_exe(), Some(exe.as_path()));

    store
        .update_process_identity(inst.id(), None, None, None)
        .expect("clear");
    inst = store
        .get_instance(inst.id())
        .expect("get")
        .expect("present");
    assert!(inst.pid().is_none());

    let new_port = Port::new(5433).expect("port");
    store.update_port(inst.id(), new_port).expect("port");
    inst = store
        .get_instance(inst.id())
        .expect("get")
        .expect("present");
    assert_eq!(inst.port(), new_port);

    store.set_autostart(inst.id(), false).expect("auto");
    inst = store
        .get_instance(inst.id())
        .expect("get")
        .expect("present");
    assert!(!inst.autostart());

    store.delete_instance(inst.id()).expect("delete");
    assert!(store.get_instance(inst.id()).expect("get").is_none());
}

#[test]
fn partial_process_identity_is_rejected() {
    let store = Store::open_in_memory().expect("store");
    let inst = instance("app-dev", 5432, InstanceStatus::Stopped);
    store.insert_instance(&inst).expect("insert");
    let err = store
        .update_process_identity(inst.id(), Some(1), None, None)
        .expect_err("must reject");
    assert!(matches!(err, DblError::InvalidInput { .. }));
    assert!(matches!(
        store.update_process_identity(
            inst.id(),
            None,
            Some(UNIX_EPOCH),
            Some(PathBuf::from("/bin/pg").as_path()),
        ),
        Err(DblError::InvalidInput { .. })
    ));
}

#[test]
fn updates_on_missing_entities_are_not_found() {
    let store = Store::open_in_memory().expect("store");
    let missing = InstanceId::new();
    assert!(matches!(
        store.update_status(&missing, InstanceStatus::Running),
        Err(DblError::NotFound { .. })
    ));
    assert!(matches!(
        store.update_port(&missing, Port::new(5433).expect("port")),
        Err(DblError::NotFound { .. })
    ));
    assert!(matches!(
        store.set_autostart(&missing, true),
        Err(DblError::NotFound { .. })
    ));
    assert!(matches!(
        store.delete_instance(&missing),
        Err(DblError::NotFound { .. })
    ));
}

#[test]
fn delete_cascades_credential_pointer_but_keeps_runtimes_and_archives() {
    let store = Store::open_in_memory().expect("store");
    let inst = instance("app-dev", 5432, InstanceStatus::Stopped);
    store.insert_instance(&inst).expect("insert");
    let cred = CredentialRef::new(
        inst.id().clone(),
        CredentialBackend::Keychain,
        format!("dblocal:{}", inst.id()),
        "postgres",
    );
    store.set_credential(&cred).expect("cred");
    assert!(store.get_credential(inst.id()).expect("get").is_some());

    store
        .add_runtime(&runtime("rt-1", EngineKey::Postgresql, "18.7", 100))
        .expect("runtime");
    store
        .add_archive(&archive("ar-1", "app-dev"))
        .expect("archive");

    store.delete_instance(inst.id()).expect("delete");
    assert!(store.get_credential(inst.id()).expect("get").is_none());
    assert_eq!(store.list_runtimes().expect("rt").len(), 1);
    assert_eq!(store.list_archives().expect("ar").len(), 1);
}

#[test]
fn runtime_crud_and_total_size() {
    let store = Store::open_in_memory().expect("store");
    let r1 = runtime("rt-1", EngineKey::Postgresql, "18.7", 100);
    let r2 = runtime("rt-2", EngineKey::Valkey, "8.0", 250);
    store.add_runtime(&r1).expect("r1");
    store.add_runtime(&r2).expect("r2");
    let listed = store.list_runtimes().expect("list");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id(), "rt-1");
    assert_eq!(store.total_runtime_size().expect("sum"), 350);

    // Duplicate (engine, version, arch) must be rejected.
    let dup = runtime("rt-3", EngineKey::Postgresql, "18.7", 999);
    assert!(matches!(
        store.add_runtime(&dup),
        Err(DblError::AlreadyExists { .. })
    ));

    store.remove_runtime("rt-1").expect("remove");
    assert_eq!(store.list_runtimes().expect("list").len(), 1);
    assert!(matches!(
        store.remove_runtime("rt-1"),
        Err(DblError::NotFound {
            kind: "runtime",
            ..
        })
    ));
}

#[test]
fn archive_crud() {
    let store = Store::open_in_memory().expect("store");
    store.add_archive(&archive("ar-1", "app-dev")).expect("a1");
    store.add_archive(&archive("ar-2", "other")).expect("a2");
    let listed = store.list_archives().expect("list");
    assert_eq!(listed.len(), 2);
    // Newest first (ar-2 and ar-1 have same timestamp; stable enough to just check presence).
    assert!(listed.iter().any(|a| a.id() == "ar-1"));
    assert!(listed.iter().any(|a| a.id() == "ar-2"));
    store.remove_archive("ar-1").expect("remove");
    assert_eq!(store.list_archives().expect("list").len(), 1);
    assert!(matches!(
        store.remove_archive("ar-1"),
        Err(DblError::NotFound {
            kind: "archive",
            ..
        })
    ));
}

#[test]
fn settings_get_set() {
    let store = Store::open_in_memory().expect("store");
    assert!(store.get_setting("theme").expect("get").is_none());
    store.set_setting("theme", "dark").expect("set");
    assert_eq!(
        store.get_setting("theme").expect("get").as_deref(),
        Some("dark")
    );
    store.set_setting("theme", "light").expect("overwrite");
    assert_eq!(
        store.get_setting("theme").expect("get").as_deref(),
        Some("light")
    );
}

#[test]
fn wal_allows_concurrent_reader_while_writer_active() {
    let dir = tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    let writer = Store::open(&paths).expect("writer");
    let reader = Store::open(&paths).expect("reader");

    writer
        .connection()
        .execute_batch("BEGIN IMMEDIATE")
        .expect("begin");
    writer.set_setting("k", "v").expect("write while tx open");
    // Reader must not block on the writer's uncommitted transaction.
    let _ = reader.list_instances().expect("reader can read");
    writer.connection().execute_batch("COMMIT").expect("commit");
    assert_eq!(reader.get_setting("k").expect("get").as_deref(), Some("v"));
}

#[test]
fn drop_without_commit_leaves_no_partial_row() {
    let dir = tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    {
        let store = Store::open(&paths).expect("open");
        store.connection().execute_batch("BEGIN").expect("begin");
        store
            .connection()
            .execute(
                "INSERT INTO instances (
                    id, name, engine, version, port, status, auth, superuser,
                    pid, process_started_at_secs, process_started_at_nanos, process_exe,
                    autostart, created_at_secs, created_at_nanos
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL, NULL, NULL, 0, 0, 0)",
                params![
                    InstanceId::new().as_str(),
                    "app-dev",
                    "postgresql",
                    "18.7",
                    5432_i64,
                    "stopped",
                    "password",
                    "postgres",
                ],
            )
            .expect("insert");
        // drop without commit
    }
    let store = Store::open(&paths).expect("reopen");
    assert!(store.list_instances().expect("list").is_empty());
}

#[test]
fn transactional_insert_instance_with_credential_rolls_back_on_failure() {
    let store = Store::open_in_memory().expect("store");
    let inst = instance("app-dev", 5432, InstanceStatus::Stopped);
    let other = instance("other", 5433, InstanceStatus::Stopped);
    let bad = CredentialRef::new(
        other.id().clone(),
        CredentialBackend::File,
        "key",
        "postgres",
    );
    let err = store
        .insert_instance_with_credential(&inst, &bad)
        .expect_err("must fail");
    assert!(matches!(err, DblError::InvalidInput { .. }));
    assert!(store.list_instances().expect("list").is_empty());
}

#[test]
fn no_secret_column_anywhere_in_schema() {
    let store = Store::open_in_memory().expect("store");
    let mut stmt = store
        .connection()
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .expect("tables");
    let tables = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("map")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect");
    for table in tables {
        let mut cstmt = store
            .connection()
            .prepare(&format!("PRAGMA table_info('{table}')"))
            .expect("pragma");
        let cols = cstmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("map")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        for col in cols {
            let lower = col.to_ascii_lowercase();
            assert!(
                !(lower.contains("pass")
                    || lower.contains("secret")
                    || lower.contains("token")
                    || lower.contains("uri")),
                "forbidden column {col} in table {table}"
            );
        }
    }
}

#[test]
fn corrupt_database_is_reported_and_left_untouched() {
    let dir = tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    let db = paths.state_db();
    std::fs::create_dir_all(dir.path()).expect("dir");
    let garbage = b"this is not a sqlite database at all";
    std::fs::write(&db, garbage).expect("write");
    let err = Store::open(&paths).expect_err("must fail");
    assert!(matches!(err, DblError::Storage { .. }), "{err:?}");
    assert_eq!(err.code(), CODE_STORAGE);
    let after = std::fs::read(&db).expect("read");
    assert_eq!(
        after, garbage,
        "corrupt file must not be modified or deleted"
    );
}

#[test]
fn uses_dblocal_home_temp_dir_never_real_home() {
    let dir = tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    let store = Store::open(&paths).expect("open");
    store.set_setting("probe", "ok").expect("set");
    assert!(dir.path().join(crate::consts::STATE_DB_FILE).exists());
    assert_ne!(
        paths.root(),
        crate::paths::DblPaths::from_env().expect("env").root()
    );
}

#[test]
fn credential_pointer_round_trips_and_replaces() {
    let store = Store::open_in_memory().expect("store");
    let inst = instance("app-dev", 5432, InstanceStatus::Stopped);
    store.insert_instance(&inst).expect("insert");
    assert!(store.get_credential(inst.id()).expect("get").is_none());

    let keychain = CredentialRef::new(
        inst.id().clone(),
        CredentialBackend::Keychain,
        "dblocal:key",
        "postgres",
    );
    store.set_credential(&keychain).expect("set");
    let loaded = store
        .get_credential(inst.id())
        .expect("get")
        .expect("present");
    assert_eq!(loaded.backend(), CredentialBackend::Keychain);
    assert_eq!(loaded.key_ref(), "dblocal:key");
    assert_eq!(loaded.username(), "postgres");

    let file = CredentialRef::new(
        inst.id().clone(),
        CredentialBackend::File,
        "secrets/app-dev",
        "postgres",
    );
    store.set_credential(&file).expect("replace");
    let loaded = store
        .get_credential(inst.id())
        .expect("get")
        .expect("present");
    assert_eq!(loaded.backend(), CredentialBackend::File);
    assert_eq!(loaded.key_ref(), "secrets/app-dev");
}

#[test]
fn insert_instance_with_credential_commits_both_rows() {
    let store = Store::open_in_memory().expect("store");
    let inst = instance("app-dev", 5432, InstanceStatus::Stopped);
    let cred = CredentialRef::new(
        inst.id().clone(),
        CredentialBackend::Keychain,
        "dblocal:key",
        "postgres",
    );
    store
        .insert_instance_with_credential(&inst, &cred)
        .expect("create");
    assert!(store.get_instance(inst.id()).expect("get").is_some());
    assert!(store.get_credential(inst.id()).expect("get").is_some());
}

#[test]
fn corrupt_rows_decode_to_storage_errors() {
    let store = Store::open_in_memory().expect("store");
    store
        .connection()
        .execute(
            "INSERT INTO instances (
                id, name, engine, version, port, status, auth, superuser,
                pid, process_started_at_secs, process_started_at_nanos, process_exe,
                autostart, created_at_secs, created_at_nanos
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL, NULL, NULL, 0, 0, 0)",
            params![
                InstanceId::new().as_str(),
                "app-dev",
                "postgresql",
                "18.7",
                5432_i64,
                "paused",
                "password",
                "postgres",
            ],
        )
        .expect("insert bad status");
    let err = store.list_instances().expect_err("bad row must fail");
    assert!(matches!(err, DblError::Storage { .. }), "{err:?}");

    let store = Store::open_in_memory().expect("store");
    store
        .connection()
        .execute(
            "INSERT INTO runtimes (id, engine, version, arch, path, size_bytes, state, installed_at_secs, installed_at_nanos)
             VALUES ('rt-1', 'oracle', '1.0', 'x86_64', '/p', 1, 'installed', 0, 0)",
            [],
        )
        .expect("insert bad engine");
    assert!(matches!(
        store.list_runtimes(),
        Err(DblError::Storage { .. })
    ));

    let store = Store::open_in_memory().expect("store");
    store
        .connection()
        .execute(
            "INSERT INTO archives (id, instance_name, engine, version, path, size_bytes, archived_at_secs, archived_at_nanos)
             VALUES ('ar-1', 'Bad Name', 'postgresql', '18.7', '/p', 1, 0, 0)",
            [],
        )
        .expect("insert bad name");
    assert!(matches!(
        store.list_archives(),
        Err(DblError::Storage { .. })
    ));
}

#[test]
fn superseded_superuser_row_is_rejected() {
    let store = Store::open_in_memory().expect("store");
    store
        .connection()
        .execute(
            "INSERT INTO instances (
                id, name, engine, version, port, status, auth, superuser,
                pid, process_started_at_secs, process_started_at_nanos, process_exe,
                autostart, created_at_secs, created_at_nanos
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL, NULL, NULL, 0, 0, 0)",
            params![
                InstanceId::new().as_str(),
                "app-dev",
                "postgresql",
                "18.7",
                5432_i64,
                "stopped",
                "none",
                "someone-else",
            ],
        )
        .expect("insert");
    assert!(matches!(
        store.list_instances(),
        Err(DblError::Storage { .. })
    ));
}

#[test]
fn concurrent_first_open_never_fails() {
    use std::sync::{Arc, Barrier};

    let threads = 4usize;
    let rounds = 100usize;
    let mut failures = 0usize;
    for _ in 0..rounds {
        let round_dir = tempdir().expect("tempdir");
        let round_paths = Arc::new(DblPaths::with_root(round_dir.path()));
        let barrier = Arc::new(Barrier::new(threads));
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let p = Arc::clone(&round_paths);
                let b = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    b.wait();
                    Store::open(&p)
                })
            })
            .collect();
        for handle in handles {
            if handle.join().expect("join").is_err() {
                failures += 1;
            }
        }
    }
    eprintln!(
        "concurrent first open: {failures} failures out of {}",
        threads * rounds
    );
    assert_eq!(failures, 0, "concurrent first open had failures");
}

#[test]
fn concurrent_open_upgrades_version_zero_exactly_once() {
    use std::sync::{Arc, Barrier};

    let threads = 4usize;
    let rounds = 50usize;
    let latest = super::schema::LATEST_USER_VERSION;
    for _ in 0..rounds {
        let dir = tempdir().expect("tempdir");
        let paths = Arc::new(DblPaths::with_root(dir.path()));
        // Seed a database file at version 0 with no schema objects, as a
        // pre-migration state an older process might leave behind.
        {
            std::fs::create_dir_all(dir.path()).expect("dir");
            let conn = rusqlite::Connection::open(paths.state_db()).expect("open");
            conn.pragma_update(None, "user_version", 0_i32).expect("v0");
        }

        let barrier = Arc::new(Barrier::new(threads));
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let p = Arc::clone(&paths);
                let b = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    b.wait();
                    Store::open(&p)
                })
            })
            .collect();
        let mut failures = 0usize;
        for handle in handles {
            if handle.join().expect("join").is_err() {
                failures += 1;
            }
        }
        assert_eq!(failures, 0, "concurrent upgrade from version 0 failed");

        let store = Store::open(&paths).expect("reopen");
        let version: i32 = store
            .connection()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, latest);
        // Schema is valid and usable after the concurrent upgrade.
        assert!(store.list_instances().expect("list").is_empty());
    }
}

#[test]
fn latest_database_is_untouched_and_data_survives() {
    let dir = tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    let latest = super::schema::LATEST_USER_VERSION;
    {
        let store = Store::open(&paths).expect("open");
        store.set_setting("probe", "kept").expect("set");
        let inst = instance("app-dev", 5432, InstanceStatus::Stopped);
        store.insert_instance(&inst).expect("insert");
    }
    // Reopen when already at the latest version: version and data must survive.
    for _ in 0..5 {
        let store = Store::open(&paths).expect("reopen");
        let version: i32 = store
            .connection()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, latest);
        assert_eq!(
            store.get_setting("probe").expect("get").as_deref(),
            Some("kept")
        );
        assert_eq!(store.list_instances().expect("list").len(), 1);
    }
}
