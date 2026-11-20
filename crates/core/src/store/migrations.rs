//! Schema migrations driven by `PRAGMA user_version`.
//!
//! Every step runs inside its own `BEGIN IMMEDIATE` transaction together with
//! its `user_version` bump, and the version is re-read *inside* that
//! transaction. Two processes opening the same fresh database at once (the GUI
//! and the `dblocal` CLI, or two CLI calls from an install script) therefore
//! serialize on the write lock instead of both replaying `CREATE TABLE`: the
//! loser sees the winner's committed version and skips the step.

use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::DblError;

use super::error::map_sql;
use super::schema::{LATEST_USER_VERSION, MIGRATIONS};

/// Applies every pending migration.
///
/// A database written by a newer binary is refused with a typed error rather
/// than downgraded or rewritten.
pub(crate) fn apply(conn: &Connection) -> crate::Result<()> {
    // Fast path: an up-to-date database takes no write lock at all, so ordinary
    // concurrent opens do not serialize against each other.
    let observed = user_version(conn)?;
    ensure_supported(observed)?;
    if observed == LATEST_USER_VERSION {
        return Ok(());
    }

    // Slow path: at least one step is pending. Take the write lock, then
    // re-read the version so a concurrent opener that already migrated lets us
    // skip the work instead of replaying `CREATE TABLE`.
    loop {
        let tx =
            Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(map_sql)?;
        let version = user_version(&tx)?;
        ensure_supported(version)?;
        if version == LATEST_USER_VERSION {
            return Ok(());
        }
        let step = usize::try_from(version)
            .ok()
            .and_then(|index| MIGRATIONS.get(index))
            .ok_or_else(|| DblError::Storage {
                reason: format!("no migration from schema version {version}"),
            })?;
        tx.execute_batch(step).map_err(map_sql)?;
        tx.pragma_update(None, "user_version", version + 1)
            .map_err(map_sql)?;
        tx.commit().map_err(map_sql)?;
    }
}

/// Rejects a database newer than this binary understands.
fn ensure_supported(version: i32) -> crate::Result<()> {
    if version > LATEST_USER_VERSION {
        return Err(DblError::Storage {
            reason: format!(
                "database schema version {version} is newer than the supported {LATEST_USER_VERSION}"
            ),
        });
    }
    Ok(())
}

/// Reads the schema version recorded in the database header.
pub(crate) fn user_version(conn: &Connection) -> crate::Result<i32> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_sql)
}
