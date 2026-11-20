//! Schema migrations driven by `PRAGMA user_version`.
//!
//! Each step runs alone in one transaction together with its `user_version`
//! bump, so an interrupted upgrade leaves the database at its previous version
//! instead of half-migrated.

use rusqlite::Connection;

use crate::DblError;

use super::error::map_sql;
use super::schema::{LATEST_USER_VERSION, MIGRATIONS};

/// Applies every pending migration.
///
/// A database written by a newer binary is refused with a typed error rather
/// than downgraded or rewritten.
pub(crate) fn apply(conn: &Connection) -> crate::Result<()> {
    let current = user_version(conn)?;
    if current > LATEST_USER_VERSION {
        return Err(DblError::Storage {
            reason: format!(
                "database schema version {current} is newer than the supported {LATEST_USER_VERSION}"
            ),
        });
    }
    for version in current..LATEST_USER_VERSION {
        let step = usize::try_from(version)
            .ok()
            .and_then(|index| MIGRATIONS.get(index))
            .ok_or_else(|| DblError::Storage {
                reason: format!("no migration from schema version {version}"),
            })?;
        let tx = conn.unchecked_transaction().map_err(map_sql)?;
        tx.execute_batch(step).map_err(map_sql)?;
        tx.pragma_update(None, "user_version", version + 1)
            .map_err(map_sql)?;
        tx.commit().map_err(map_sql)?;
    }
    Ok(())
}

/// Reads the schema version recorded in the database header.
pub(crate) fn user_version(conn: &Connection) -> crate::Result<i32> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_sql)
}
