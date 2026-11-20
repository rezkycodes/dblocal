//! Runtime and archive CRUD, all typed and transactional.

use rusqlite::params;

use crate::DblError;
use crate::model::{ArchivedData, Runtime};

use super::Store;
use crate::store::error::{Constraint, constraint_target, ensure_affected, map_sql};
use crate::store::rows::{self, ARCHIVE_COLUMNS, RUNTIME_COLUMNS};
use crate::store::time::encode;

impl Store {
    /// Records an installed runtime.
    ///
    /// `(engine, version, arch)` is unique; a second insert is `AlreadyExists`.
    pub fn add_runtime(&self, runtime: &Runtime) -> crate::Result<()> {
        let (secs, nanos) = encode(runtime.installed_at());
        self.conn
            .execute(
                "INSERT INTO runtimes (id, engine, version, arch, path, size_bytes, state, installed_at_secs, installed_at_nanos)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    runtime.id(),
                    runtime.engine().as_str(),
                    runtime.version(),
                    runtime.arch().as_str(),
                    runtime.path().to_string_lossy(),
                    i64::try_from(runtime.size_bytes()).map_err(|_| size_too_large())?,
                    runtime.state().as_str(),
                    secs,
                    i64::from(nanos),
                ],
            )
            .map_err(runtime_conflict)?;
        Ok(())
    }

    /// Lists installed runtimes, ordered by engine, version and architecture.
    pub fn list_runtimes(&self) -> crate::Result<Vec<Runtime>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {RUNTIME_COLUMNS} FROM runtimes ORDER BY engine, version, arch, id"
            ))
            .map_err(map_sql)?;
        let rows = stmt.query_map([], rows::runtime).map_err(map_sql)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sql)
    }

    /// Removes a runtime by id.
    ///
    /// Referential safety ("still used by an instance") is the caller's rule:
    /// the schema keeps `versions` as a tag rather than a foreign key so an
    /// instance still points at a version that the catalog dropped.
    pub fn remove_runtime(&self, runtime_id: &str) -> crate::Result<()> {
        let affected = self
            .conn
            .execute("DELETE FROM runtimes WHERE id = ?1", params![runtime_id])
            .map_err(map_sql)?;
        ensure_affected(affected, "runtime", runtime_id)
    }

    /// Sum of `size_bytes` across every runtime, for Settings -> Storage.
    pub fn total_runtime_size(&self) -> crate::Result<u64> {
        let total: Option<i64> = self
            .conn
            .query_row("SELECT SUM(size_bytes) FROM runtimes", [], |row| row.get(0))
            .map_err(map_sql)?;
        Ok(total.unwrap_or(0).max(0).unsigned_abs())
    }

    /// Records an archived data directory (D-18).
    pub fn add_archive(&self, archive: &ArchivedData) -> crate::Result<()> {
        let (secs, nanos) = encode(archive.archived_at());
        self.conn
            .execute(
                "INSERT INTO archives (id, instance_name, engine, version, path, size_bytes, archived_at_secs, archived_at_nanos)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    archive.id(),
                    archive.instance_name().as_str(),
                    archive.engine().as_str(),
                    archive.version(),
                    archive.path().to_string_lossy(),
                    i64::try_from(archive.size_bytes()).map_err(|_| size_too_large())?,
                    secs,
                    i64::from(nanos),
                ],
            )
            .map_err(runtime_conflict)?;
        Ok(())
    }

    /// Lists archives, newest first.
    pub fn list_archives(&self) -> crate::Result<Vec<ArchivedData>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {ARCHIVE_COLUMNS} FROM archives
                 ORDER BY archived_at_secs DESC, archived_at_nanos DESC, id"
            ))
            .map_err(map_sql)?;
        let rows = stmt.query_map([], rows::archive).map_err(map_sql)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sql)
    }

    /// Removes an archive record by id.
    pub fn remove_archive(&self, archive_id: &str) -> crate::Result<()> {
        let affected = self
            .conn
            .execute("DELETE FROM archives WHERE id = ?1", params![archive_id])
            .map_err(map_sql)?;
        ensure_affected(affected, "archive", archive_id)
    }
}

fn size_too_large() -> DblError {
    DblError::InvalidInput {
        field: "size_bytes",
        reason: "does not fit in a signed 64-bit column".to_owned(),
    }
}

fn runtime_conflict(err: rusqlite::Error) -> DblError {
    match constraint_target(&err) {
        Some(Constraint::RuntimeKey) => DblError::AlreadyExists {
            kind: "runtime",
            name: String::new(),
        },
        Some(Constraint::PrimaryKey) => DblError::AlreadyExists {
            kind: "record",
            name: String::new(),
        },
        _ => map_sql(err),
    }
}
