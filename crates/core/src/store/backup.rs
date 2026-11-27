//! Pre-migration backup of the on-disk state database.
//!
//! Before the first pending migration step is applied, the database as found is
//! copied next to itself as `<file name>.v<old>.bak` with `VACUUM INTO`. SQLite
//! produces the copy (never a file copy), so rows that still live only in the
//! `-wal` file are included. The call runs while the migrating connection holds
//! its `BEGIN IMMEDIATE` write lock and after it re-reads `user_version` inside
//! that transaction, so a concurrent opener cannot migrate between the check and
//! the copy and a backup can never contain the new schema.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};

use crate::DblError;
use crate::consts::{STATE_DB_FILE, STORE_BUSY_TIMEOUT_MS};

use super::error::map_sql;

/// Copies the database at `old_version` to `<file name>.v<old>.bak`.
///
/// Does nothing for an in-memory database. On any failure the temporary file is
/// removed and a [`DblError::Storage`] naming the backup path is returned, so
/// the caller can leave the database at its old version.
pub(crate) fn before_migration(conn: &Connection, old_version: i32) -> crate::Result<()> {
    let Some(source) = on_disk_path(conn) else {
        return Ok(());
    };
    let backup = backup_path(&source, old_version);
    let temp = temp_path(&backup);
    if let Err(cause) = write_backup(&source, &temp, old_version) {
        remove_quietly(&temp);
        return Err(failed(&backup, cause));
    }
    if let Err(cause) = publish(&temp, &backup) {
        remove_quietly(&temp);
        return Err(failed(&backup, cause));
    }
    tracing::info!(
        path = %backup.display(),
        from_version = old_version,
        "backed up the state database before migration"
    );
    Ok(())
}

fn on_disk_path(conn: &Connection) -> Option<PathBuf> {
    conn.path()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

fn backup_path(source: &Path, version: i32) -> PathBuf {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| STATE_DB_FILE.to_owned());
    source.with_file_name(format!("{name}.v{version}.bak"))
}

fn temp_path(backup: &Path) -> PathBuf {
    let mut name = backup.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

fn write_backup(source: &Path, temp: &Path, expected: i32) -> crate::Result<()> {
    remove_quietly(temp);
    create_private_temp(temp)?;
    vacuum_into(source, temp)?;
    #[cfg(unix)]
    set_private_mode(temp)?;
    verify(temp, expected)
}

fn create_private_temp(path: &Path) -> crate::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .map_err(|source| DblError::Io { source })?;
    drop(file);
    Ok(())
}

#[cfg(unix)]
fn set_private_mode(path: &Path) -> crate::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|source| DblError::Io { source })
}

/// Copies the committed database into `destination` through a second read-only
/// connection, so it sees the pre-upgrade snapshot including the WAL.
fn vacuum_into(source: &Path, destination: &Path) -> crate::Result<()> {
    let destination_text = destination.to_str().ok_or_else(|| DblError::Storage {
        reason: format!("backup path {} is not valid UTF-8", destination.display()),
    })?;
    let conn =
        Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(map_sql)?;
    conn.busy_timeout(std::time::Duration::from_millis(u64::from(
        STORE_BUSY_TIMEOUT_MS,
    )))
    .map_err(map_sql)?;
    conn.execute("VACUUM INTO ?1", params![destination_text])
        .map_err(map_sql)?;
    Ok(())
}

fn verify(path: &Path, expected: i32) -> crate::Result<()> {
    let conn =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(map_sql)?;
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_sql)?;
    if version != expected {
        return Err(DblError::Storage {
            reason: format!("backup reports schema version {version}, expected {expected}"),
        });
    }
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(map_sql)?;
    if !check.eq_ignore_ascii_case("ok") {
        return Err(DblError::Storage {
            reason: format!("backup failed quick_check: {check}"),
        });
    }
    Ok(())
}

fn publish(temp: &Path, backup: &Path) -> crate::Result<()> {
    // `rename` replaces an existing destination atomically on Unix and Windows,
    // so an older good backup is never deleted before the new one is in place.
    std::fs::rename(temp, backup).map_err(|source| DblError::Io { source })
}

fn remove_quietly(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "could not remove a temporary backup file"
            );
        }
    }
}

fn failed(backup: &Path, cause: DblError) -> DblError {
    DblError::Storage {
        reason: format!(
            "could not back up the state database to {} before migrating: {cause}",
            backup.display()
        ),
    }
}
