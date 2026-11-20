//! Public store API: opening a database and the primitives shared by every
//! table.
//!
//! The type is synchronous on purpose: the service layer calls it from
//! `spawn_blocking`, so nothing here is async.

mod instance;
mod runtime;

use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use rusqlite::{Connection, OptionalExtension, params};

use crate::DblError;
use crate::model::{CredentialBackend, CredentialRef, InstanceId};
use crate::paths::DblPaths;

use super::error::map_sql;
use super::migrations::apply;
use super::schema::STORE_BUSY_TIMEOUT_MS;

/// Synchronous persistence for DBLocal state.
///
/// One instance owns one SQLite connection. Share it behind the service's own
/// lock; do not open a second writer while the first is in a transaction.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens, creates and migrates the state database under `paths`.
    ///
    /// The `~/.dblocal` root is created with mode `0700` on Unix. Every
    /// connection gets `journal_mode=WAL`, `foreign_keys=ON`,
    /// `synchronous=NORMAL` and a fixed `busy_timeout`.
    pub fn open(paths: &DblPaths) -> crate::Result<Self> {
        ensure_private_dir(paths.root())?;
        let conn = Connection::open(paths.state_db()).map_err(map_sql)?;
        Self::from_connection(conn)
    }

    /// Opens an empty in-memory database, for tests and spikes.
    pub fn open_in_memory() -> crate::Result<Self> {
        let conn = Connection::open_in_memory().map_err(map_sql)?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> crate::Result<Self> {
        configure(&conn)?;
        apply(&conn)?;
        Ok(Self { conn })
    }

    /// Reads a setting value by key.
    pub fn get_setting(&self, key: &str) -> crate::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sql)
    }

    /// Inserts or replaces a setting.
    pub fn set_setting(&self, key: &str, value: &str) -> crate::Result<()> {
        self.conn
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    /// Reads the credential pointer for an instance.
    ///
    /// The secret itself is never in SQLite; this only says where it lives.
    pub fn get_credential(&self, id: &InstanceId) -> crate::Result<Option<CredentialRef>> {
        self.conn
            .query_row(
                "SELECT backend, key_ref, username FROM credentials WHERE instance_id = ?1",
                params![id.as_str()],
                |row| {
                    let backend: String = row.get(0)?;
                    let key_ref: String = row.get(1)?;
                    let username: String = row.get(2)?;
                    Ok((backend, key_ref, username))
                },
            )
            .optional()
            .map_err(map_sql)?
            .map(|(backend, key_ref, username)| {
                let backend = backend.parse::<CredentialBackend>()?;
                Ok(CredentialRef::new(id.clone(), backend, key_ref, username))
            })
            .transpose()
    }

    /// Inserts or replaces the credential pointer for an instance.
    pub fn set_credential(&self, credential: &CredentialRef) -> crate::Result<()> {
        self.conn
            .execute(
                "INSERT INTO credentials (instance_id, backend, key_ref, username)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(instance_id) DO UPDATE SET
                     backend = excluded.backend,
                     key_ref = excluded.key_ref,
                     username = excluded.username",
                params![
                    credential.instance_id().as_str(),
                    credential.backend().as_str(),
                    credential.key_ref(),
                    credential.username(),
                ],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    /// Escape hatch used only by tests that need to build a bad state.
    #[cfg(test)]
    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }
}

fn configure(conn: &Connection) -> crate::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(map_sql)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(map_sql)?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(map_sql)?;
    conn.busy_timeout(std::time::Duration::from_millis(u64::from(
        STORE_BUSY_TIMEOUT_MS,
    )))
    .map_err(map_sql)?;
    Ok(())
}

fn ensure_private_dir(path: &Path) -> crate::Result<()> {
    std::fs::create_dir_all(path).map_err(|source| DblError::Io { source })?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|source| DblError::Io { source })?;
    Ok(())
}
