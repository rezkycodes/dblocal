//! Instance CRUD, all typed and transactional.

use std::time::SystemTime;

use rusqlite::{OptionalExtension, params};

use crate::DblError;
use crate::model::{CredentialRef, Instance, InstanceId, InstanceStatus, Port};

use super::Store;
use crate::store::error::{Constraint, constraint_target, ensure_affected, map_sql};
use crate::store::rows::{self, INSTANCE_COLUMNS};
use crate::store::time::{encode, encode_opt};

impl Store {
    /// Inserts a new instance.
    ///
    /// A duplicate name (compared case-insensitively) or a duplicate port is
    /// reported as [`DblError::AlreadyExists`], never as a raw SQL failure.
    pub fn insert_instance(&self, instance: &Instance) -> crate::Result<()> {
        let (started_secs, started_nanos) = encode_opt(instance.process_started_at());
        let (created_secs, created_nanos) = encode(instance.created_at());
        let exe = instance
            .process_exe()
            .map(|path| path.to_string_lossy().into_owned());
        self.conn
            .execute(
                "INSERT INTO instances (
                    id, name, engine, version, port, status, auth, superuser,
                    pid, process_started_at_secs, process_started_at_nanos, process_exe,
                    autostart, created_at_secs, created_at_nanos
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    instance.id().as_str(),
                    instance.name().as_str(),
                    instance.engine().as_str(),
                    instance.version(),
                    i64::from(instance.port().get()),
                    instance.status().as_str(),
                    instance.auth().as_str(),
                    instance.superuser(),
                    instance.pid().map(i64::from),
                    started_secs,
                    started_nanos.map(i64::from),
                    exe,
                    i64::from(u8::from(instance.autostart())),
                    created_secs,
                    i64::from(created_nanos),
                ],
            )
            .map_err(instance_conflict)?;
        Ok(())
    }

    /// Inserts an instance and its credential pointer in one transaction.
    ///
    /// Create always writes both (`F-CRD-01`), so a failure to record the
    /// pointer must not leave an instance claiming a secret it does not have.
    pub fn insert_instance_with_credential(
        &self,
        instance: &Instance,
        credential: &CredentialRef,
    ) -> crate::Result<()> {
        if credential.instance_id() != instance.id() {
            return Err(DblError::InvalidInput {
                field: "credential",
                reason: "credential points at a different instance".to_owned(),
            });
        }
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(map_sql)?;
        let outcome = self
            .insert_instance(instance)
            .and_then(|()| self.set_credential(credential));
        match outcome {
            Ok(()) => self.conn.execute_batch("COMMIT").map_err(map_sql),
            Err(err) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    /// Returns an instance by id.
    pub fn get_instance(&self, id: &InstanceId) -> crate::Result<Option<Instance>> {
        self.conn
            .query_row(
                &format!("SELECT {INSTANCE_COLUMNS} FROM instances WHERE id = ?1"),
                params![id.as_str()],
                rows::instance,
            )
            .optional()
            .map_err(map_sql)
    }

    /// Returns an instance by name, comparing case-insensitively.
    pub fn get_instance_by_name(&self, name: &str) -> crate::Result<Option<Instance>> {
        self.conn
            .query_row(
                &format!("SELECT {INSTANCE_COLUMNS} FROM instances WHERE name = ?1 COLLATE NOCASE"),
                params![name],
                rows::instance,
            )
            .optional()
            .map_err(map_sql)
    }

    /// Lists every instance, ordered by name.
    pub fn list_instances(&self) -> crate::Result<Vec<Instance>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {INSTANCE_COLUMNS} FROM instances ORDER BY name COLLATE NOCASE, id"
            ))
            .map_err(map_sql)?;
        let rows = stmt.query_map([], rows::instance).map_err(map_sql)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sql)
    }

    /// Persists a status exactly as given.
    ///
    /// Transition legality belongs to [`InstanceStatus::apply`] and the
    /// service; the store never second-guesses the caller.
    pub fn update_status(&self, id: &InstanceId, status: InstanceStatus) -> crate::Result<()> {
        let affected = self
            .conn
            .execute(
                "UPDATE instances SET status = ?1 WHERE id = ?2",
                params![status.as_str(), id.as_str()],
            )
            .map_err(map_sql)?;
        ensure_affected(affected, "instance", id.as_str())
    }

    /// Persists the recorded process identity, or clears it when every part is
    /// `None`.
    ///
    /// Rejects a partially specified identity: `pid`, start time and executable
    /// are either all known or all forgotten, which is what PID-reuse
    /// verification (AGENTS.md invariant 7) needs.
    pub fn update_process_identity(
        &self,
        id: &InstanceId,
        pid: Option<u32>,
        started_at: Option<SystemTime>,
        exe: Option<&std::path::Path>,
    ) -> crate::Result<()> {
        let (started_secs, started_nanos) = encode_opt(started_at);
        let exe = exe.map(|path| path.to_string_lossy().into_owned());
        let parts = [pid.is_some(), started_secs.is_some(), exe.is_some()];
        if parts.iter().any(|set| *set) && !parts.iter().all(|set| *set) {
            return Err(DblError::InvalidInput {
                field: "process_identity",
                reason: "pid, process start time and executable must be set together".to_owned(),
            });
        }
        let affected = self
            .conn
            .execute(
                "UPDATE instances
                 SET pid = ?1, process_started_at_secs = ?2,
                     process_started_at_nanos = ?3, process_exe = ?4
                 WHERE id = ?5",
                params![
                    pid.map(i64::from),
                    started_secs,
                    started_nanos.map(i64::from),
                    exe,
                    id.as_str(),
                ],
            )
            .map_err(map_sql)?;
        ensure_affected(affected, "instance", id.as_str())
    }

    /// Persists a new port, reporting a collision as `AlreadyExists`.
    pub fn update_port(&self, id: &InstanceId, port: Port) -> crate::Result<()> {
        let affected = self
            .conn
            .execute(
                "UPDATE instances SET port = ?1 WHERE id = ?2",
                params![i64::from(port.get()), id.as_str()],
            )
            .map_err(instance_conflict)?;
        ensure_affected(affected, "instance", id.as_str())
    }

    /// Persists the autostart flag.
    pub fn set_autostart(&self, id: &InstanceId, enabled: bool) -> crate::Result<()> {
        let affected = self
            .conn
            .execute(
                "UPDATE instances SET autostart = ?1 WHERE id = ?2",
                params![i64::from(u8::from(enabled)), id.as_str()],
            )
            .map_err(map_sql)?;
        ensure_affected(affected, "instance", id.as_str())
    }

    /// Deletes an instance and, by cascade, its credential pointer.
    ///
    /// Runtimes and archives are left alone: they outlive instances (D-18 and
    /// `F-RTM-04`).
    pub fn delete_instance(&self, id: &InstanceId) -> crate::Result<()> {
        let affected = self
            .conn
            .execute("DELETE FROM instances WHERE id = ?1", params![id.as_str()])
            .map_err(map_sql)?;
        ensure_affected(affected, "instance", id.as_str())
    }

    /// Compares and sets the status in one atomic statement.
    ///
    /// Returns `true` when the row existed and its status matched `from`, so
    /// the update was applied. Returns `false` on any mismatch or if the row is
    /// gone. This is the cross-process lock primitive used by the service.
    pub fn transition_status(
        &self,
        id: &InstanceId,
        from: InstanceStatus,
        to: InstanceStatus,
    ) -> crate::Result<bool> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(map_sql)?;
        let result = self.conn.execute(
            "UPDATE instances SET status = ?1 WHERE id = ?2 AND status = ?3",
            params![to.as_str(), id.as_str(), from.as_str()],
        );
        match result {
            Ok(affected) => {
                // A failing COMMIT would otherwise leave the transaction open on
                // this connection, poisoning every later statement; roll back so
                // the handle stays usable and the error is reported.
                match self.conn.execute_batch("COMMIT") {
                    Ok(()) => Ok(affected == 1),
                    Err(err) => {
                        let _ = self.conn.execute_batch("ROLLBACK");
                        Err(map_sql(err))
                    }
                }
            }
            Err(err) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(map_sql(err))
            }
        }
    }
}

fn instance_conflict(err: rusqlite::Error) -> DblError {
    match constraint_target(&err) {
        Some(Constraint::InstanceName) => DblError::AlreadyExists {
            kind: "instance name",
            name: String::new(),
        },
        Some(Constraint::InstancePort) => DblError::AlreadyExists {
            kind: "instance port",
            name: String::new(),
        },
        _ => map_sql(err),
    }
}
