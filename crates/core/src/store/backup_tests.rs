//! Tests for the pre-migration backup.
//!
//! The version 1 fixture is built from the real [`MIGRATION_V1`]
//! (`MIGRATIONS[0]`), never from a hand-written schema, so it cannot drift from
//! what a v0.1.x binary actually wrote.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};
use tempfile::tempdir;

use crate::DblError;
use crate::model::InstanceId;
use crate::paths::DblPaths;

use super::Store;
use super::migrations::user_version;
use super::schema::{LATEST_USER_VERSION, MIGRATIONS};

/// Builds a genuine version 1 database with the given `(name, port)` rows and
/// returns its still-open writer, so a caller can keep rows in the `-wal` file.
fn build_v1(paths: &DblPaths, rows: &[(&str, u16)]) -> Connection {
    std::fs::create_dir_all(paths.root()).expect("create root");
    let conn = Connection::open(paths.state_db()).expect("open fixture");
    conn.pragma_update(None, "journal_mode", "WAL")
        .expect("wal");
    conn.pragma_update(None, "wal_autocheckpoint", 0_i32)
        .expect("no autocheckpoint");
    conn.execute_batch(MIGRATIONS[0]).expect("apply real v1");
    conn.pragma_update(None, "user_version", 1_i32)
        .expect("set v1");
    for (name, port) in rows {
        conn.execute(
            "INSERT INTO instances (
                id, name, engine, version, port, status, auth, superuser,
                pid, process_started_at_secs, process_started_at_nanos, process_exe,
                autostart, created_at_secs, created_at_nanos
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL, NULL, NULL, 0, 10, 0)",
            params![
                InstanceId::new().as_str(),
                name,
                "postgresql",
                "18.7",
                i64::from(*port),
                "stopped",
                "password",
                "postgres",
            ],
        )
        .expect("insert v1 row");
    }
    conn
}

fn backup_of(paths: &DblPaths, version: i32) -> PathBuf {
    paths
        .state_db()
        .with_file_name(format!("state.db.v{version}.bak"))
}

fn files_ending(root: &Path, suffix: &str) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .expect("read dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().ends_with(suffix))
                .unwrap_or(false)
        })
        .collect()
}

fn rows_in(path: &Path) -> Vec<(String, i64)> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).expect("open");
    let mut stmt = conn
        .prepare("SELECT name, port FROM instances ORDER BY name")
        .expect("prepare");
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows")
}

fn has_extra_port(path: &Path) -> bool {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).expect("open");
    let mut stmt = conn
        .prepare("PRAGMA table_info('instances')")
        .expect("pragma");
    stmt.query_map([], |row| row.get::<_, String>(1))
        .expect("columns")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect")
        .iter()
        .any(|name| name == "extra_port")
}

#[test]
fn upgrade_backs_up_v1_rows_without_the_new_column() {
    let dir = tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    drop(build_v1(&paths, &[("legacy-a", 5432), ("legacy-b", 5433)]));

    let store = Store::open(&paths).expect("migrate v1 -> v2");
    assert_eq!(
        store
            .connection()
            .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .expect("version"),
        LATEST_USER_VERSION
    );

    let backup = backup_of(&paths, 1);
    assert!(
        backup.is_file(),
        "backup must exist at {}",
        backup.display()
    );
    assert_eq!(
        user_version(&Connection::open(&backup).expect("open backup")).expect("backup version"),
        1
    );
    assert_eq!(
        rows_in(&backup),
        vec![("legacy-a".to_owned(), 5432), ("legacy-b".to_owned(), 5433),]
    );
    assert!(
        !has_extra_port(&backup),
        "the backup must predate the v2 column"
    );
    assert!(has_extra_port(&paths.state_db()));
}

#[test]
fn committed_wal_rows_reach_the_backup() {
    let dir = tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    // Keep the writer open so the committed rows stay in `-wal` (no checkpoint
    // on close) until the migrating connection has taken its backup.
    let writer = build_v1(&paths, &[("wal-row", 5501)]);
    let wal = paths.state_db().with_extension("db-wal");
    assert!(
        std::fs::metadata(&wal).expect("wal exists").len() > 0,
        "the row must live in the -wal file"
    );

    let store = Store::open(&paths).expect("migrate");
    drop(store);
    drop(writer);

    let backup = backup_of(&paths, 1);
    assert_eq!(rows_in(&backup), vec![("wal-row".to_owned(), 5501)]);
}

#[test]
fn fresh_and_up_to_date_databases_get_no_backup() {
    let dir = tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    drop(Store::open(&paths).expect("fresh open"));
    assert!(
        files_ending(dir.path(), ".bak").is_empty(),
        "a fresh database must not be backed up"
    );

    drop(Store::open(&paths).expect("reopen up to date"));
    assert!(
        files_ending(dir.path(), ".bak").is_empty(),
        "an up-to-date database must not be backed up"
    );
}

#[test]
fn newer_database_is_refused_without_a_backup() {
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
    assert!(matches!(err, DblError::Storage { .. }), "{err:?}");
    assert!(
        files_ending(dir.path(), ".bak").is_empty(),
        "a refused newer database must not be backed up"
    );
}

#[test]
fn backup_failure_keeps_the_old_version_and_reports_the_path() {
    let dir = tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    drop(build_v1(&paths, &[("keep-me", 5432)]));
    // A directory where the temporary file must go makes the write fail closed.
    let blocker = backup_of(&paths, 1).with_file_name("state.db.v1.bak.tmp");
    std::fs::create_dir(&blocker).expect("blocker dir");

    let err = Store::open(&paths).expect_err("backup failure must abort the migration");
    let DblError::Storage { reason } = &err else {
        panic!("expected Storage, got {err:?}");
    };
    assert!(
        reason.contains("state.db.v1.bak"),
        "message must name the backup path: {reason}"
    );

    let conn = Connection::open(paths.state_db()).expect("reopen raw");
    assert_eq!(user_version(&conn).expect("version"), 1);
    assert!(
        !backup_of(&paths, 1).exists(),
        "a partial backup must never sit under the final name"
    );
}

#[test]
fn an_older_backup_is_replaced() {
    let dir = tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    drop(build_v1(&paths, &[("fresh", 5440)]));
    let stale = backup_of(&paths, 1);
    std::fs::write(&stale, b"stale bytes that are not a database").expect("stale backup");

    drop(Store::open(&paths).expect("migrate"));

    assert_eq!(rows_in(&stale), vec![("fresh".to_owned(), 5440)]);
    assert_eq!(
        user_version(&Connection::open(&stale).expect("open")).expect("version"),
        1
    );
}

#[test]
fn concurrent_opens_leave_one_valid_backup_and_no_temp_files() {
    use std::sync::{Arc, Barrier};

    let dir = tempdir().expect("tempdir");
    let paths = Arc::new(DblPaths::with_root(dir.path()));
    drop(build_v1(&paths, &[("shared", 5450)]));

    let threads = 4usize;
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
    for handle in handles {
        handle.join().expect("join").expect("concurrent open");
    }

    let backups = files_ending(dir.path(), ".bak");
    assert_eq!(backups.len(), 1, "exactly one backup: {backups:?}");
    assert_eq!(
        rows_in(&backups[0]),
        vec![("shared".to_owned(), 5450)],
        "the single backup must be a valid v1 copy"
    );
    assert_eq!(
        user_version(&Connection::open(&backups[0]).expect("open")).expect("version"),
        1
    );
    assert!(
        files_ending(dir.path(), ".tmp").is_empty(),
        "no temporary file may be left behind"
    );
}

#[cfg(unix)]
#[test]
fn backup_is_private_on_unix() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    drop(build_v1(&paths, &[("secret-ish", 5460)]));

    drop(Store::open(&paths).expect("migrate"));

    let mode = std::fs::metadata(backup_of(&paths, 1))
        .expect("meta")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}
