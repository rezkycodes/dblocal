//! Database and user administration through `DblService` (`F-DBU-01..03`).
//!
//! Every call runs against a **running** instance over loopback, and the running
//! gate is enforced here: a row that is not `Running` is refused with
//! `AdminError::Connect` before any credential is read and before any dial
//! (W4-A3 fix 1 — a stopped instance's port may belong to another process).
//! Credentials are read from the secret store and handed to [`PgAdmin`] as an
//! `AdminTarget`; they are never placed in an event, a log line, an error or a
//! returned view — the only value that leaves this module is the one-time
//! password a create-user or password-reset call is defined to return
//! (`F-DBU-02` AC2, `F-DBU-03` AC1).
//!
//! Valkey has no named databases or login roles, so it returns the module's
//! typed, stable-coded "not supported" error instead of an empty list or a panic.
//! PostgreSQL, MySQL and MariaDB all have an executor.
//!
//! The engine calls sit behind [`AdminExecutor`] so every branch above them —
//! the running gate, the engine gate, the credential lookup, the
//! rollback-on-grant-failure, the pointer update after a reset — is
//! unit-testable without a server; the real [`PgAdmin`] is what the ignored
//! admin service e2e exercises.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::DblError;
use crate::admin::{
    AdminError, AdminExecutor, AdminResult, AdminTarget, CredentialSink, DatabaseInfo, Identifier,
    PgAdmin, PrivilegeLevel, UserInfo,
};
use crate::consts;
use crate::model::{AuthMode, EngineKey, Instance, InstanceId, InstanceStatus};
use crate::secret::SecretString;
use crate::secrets::{BackendKind, credential_ref_for};
use crate::service::{ServiceResult, UserFacingError};

use super::read_ops::load;
use super::service::Inner;

/// Builds the executor for one instance from its stored credential. Production
/// dials loopback (PostgreSQL) or the instance socket (MySQL/MariaDB); tests
/// inject a fake.
pub type AdminFactory =
    Arc<dyn Fn(&Instance, SecretString) -> ServiceResult<Box<dyn AdminExecutor>> + Send + Sync>;

/// The production factory for PostgreSQL: a [`PgAdmin`] over loopback.
pub fn pg_admin_factory() -> AdminFactory {
    Arc::new(|instance, password| {
        let target = AdminTarget::new(
            "127.0.0.1",
            instance.port().get(),
            instance.superuser(),
            password,
        )
        .map_err(UserFacingError::from)?
        .with_timeouts(
            Duration::from_secs(consts::ADMIN_CONNECT_TIMEOUT_SECS),
            Duration::from_secs(consts::ADMIN_STATEMENT_TIMEOUT_SECS),
        );
        Ok(Box::new(PgAdmin::new(target)))
    })
}

/// The production factory for every engine: PostgreSQL over loopback, and
/// MySQL/MariaDB over the instance unix socket (see [`crate::admin::mysql`]).
///
/// `paths` locates the instance's `my.cnf` so the socket path is the one the
/// adapter wrote (including the long-path fallback).
pub fn default_admin_factory(paths: crate::paths::DblPaths) -> AdminFactory {
    let pg = pg_admin_factory();
    Arc::new(move |instance, password| match instance.engine() {
        EngineKey::Postgresql => pg(instance, password),
        EngineKey::Mysql => mysql_admin(&paths, instance, password, "mysql"),
        EngineKey::Mariadb => mysql_admin(&paths, instance, password, "mariadb"),
        // Valkey has no named databases or login roles; the engine gate rejects
        // it before this factory is called, but returning Unsupported here keeps
        // the factory total rather than panicking.
        EngineKey::Valkey => Err(UserFacingError::from(AdminError::Unsupported)),
    })
}

/// Builds a [`MysqlAdmin`] for a MySQL/MariaDB instance over its unix socket, or
/// over loopback TCP on Windows (see [`crate::admin::mysql::Transport`]).
fn mysql_admin(
    paths: &crate::paths::DblPaths,
    instance: &Instance,
    password: SecretString,
    engine: &'static str,
) -> ServiceResult<Box<dyn AdminExecutor>> {
    let target = mysql_target(paths, instance, password).map_err(|err| {
        if err != AdminError::Unsupported {
            tracing::warn!(engine, instance = %instance.name(), error = %err, "cannot build the admin target");
        }
        UserFacingError::from(err)
    })?;
    Ok(Box::new(crate::admin::MysqlAdmin::new(target)))
}

/// The pure connection description for a MySQL/MariaDB admin session.
///
/// Split out of [`mysql_admin`] so the port and socket that reach
/// [`MysqlTarget`] are provable without an engine: on unix the target carries the
/// configured socket and the instance's own port; off unix it carries the port
/// the loopback arm dials (see [`crate::admin::mysql::Transport`]).
///
/// A non-MySQL engine is [`AdminError::Unsupported`] (`DBL-8111`), never a
/// connect failure: the `executor_for` engine gate normally stops such an engine
/// first, but this arm must keep the honest answer if it is ever reached.
pub(crate) fn mysql_target(
    paths: &crate::paths::DblPaths,
    instance: &Instance,
    password: SecretString,
) -> AdminResult<crate::admin::MysqlTarget> {
    let name = instance.name();
    let data_dir = paths.data_dir(name);
    let run_dir = paths.run_dir(name);
    let socket = match instance.engine() {
        EngineKey::Mysql => admin_socket(cfg!(unix), || {
            crate::engine::mysql_configured_socket_path(
                &run_dir,
                instance.port().get(),
                &paths.instance_dir(name),
            )
        }),
        EngineKey::Mariadb => admin_socket(cfg!(unix), || {
            crate::engine::mariadb_configured_socket_path(&data_dir, instance.port().get())
        }),
        // F3: a wrong engine is "not supported", not "cannot connect".
        _ => return Err(AdminError::Unsupported),
    }
    .map_err(|err| {
        tracing::warn!(instance = %name, error = %err, "cannot resolve the admin socket");
        AdminError::Connect
    })?;
    Ok(crate::admin::MysqlTarget::new(
        socket.unwrap_or_default(),
        instance.port().get(),
        instance.superuser(),
        password,
    )
    .with_timeouts(
        Duration::from_secs(consts::ADMIN_CONNECT_TIMEOUT_SECS),
        Duration::from_secs(consts::ADMIN_STATEMENT_TIMEOUT_SECS),
    ))
}

/// The socket path an admin target carries, or `None` on a platform without unix
/// sockets.
///
/// Resolving the socket is a unix-only concern: the loopback TCP arm never dials
/// it, and resolution validates the AF_UNIX path limit, which a Windows path
/// with no socket to dial must not be failed by. `resolve` is therefore not
/// called off unix.
pub(crate) fn admin_socket(
    unix: bool,
    resolve: impl FnOnce() -> crate::error::Result<std::path::PathBuf>,
) -> crate::error::Result<Option<std::path::PathBuf>> {
    if unix { resolve().map(Some) } else { Ok(None) }
}

/// Runs `f` against the executor for `instance`, after the engine gate, the
/// running gate and the credential read.
fn with_admin<T>(
    inner: &Inner,
    id: &InstanceId,
    f: impl FnOnce(&dyn AdminExecutor) -> AdminResult<T>,
) -> ServiceResult<T> {
    let instance = load(inner, id)?;
    let executor = executor_for(inner, &instance)?;
    Ok(f(executor.as_ref())?)
}

/// Builds the executor for `instance`, enforcing the engine, running and
/// password gates first, in that order.
///
/// The **engine gate comes first**: the answer must not depend on the state, so
/// a stopped Valkey instance says "not supported" (`DBL-8111`) exactly like a
/// running one — Valkey never has named databases or login roles, and starting
/// it cannot change that (W4-A3 fix 2).
///
/// The running gate then runs before any credential read: an admin dial carries
/// the superuser password, and a stopped instance's port may now belong to
/// something else (a stopped MySQL's freed 3306, say). Refusing here means an
/// admin call to a non-`Running` row never reads the password and never dials.
fn executor_for(inner: &Inner, instance: &Instance) -> ServiceResult<Box<dyn AdminExecutor>> {
    if instance.engine() == EngineKey::Valkey {
        // Valkey has no named databases or login roles: a typed "not supported",
        // not an empty list. PostgreSQL, MySQL and MariaDB are all supported.
        return Err(AdminError::Unsupported.into());
    }
    if instance.status() != InstanceStatus::Running {
        // The same DBL-8100 "Make sure the server is running" a stopped server
        // already produced by failing to connect, so no caller changes.
        return Err(AdminError::Connect.into());
    }
    if instance.auth() != AuthMode::Password {
        // The engine *is* supported; this instance was created without a
        // password, so there is nothing to authenticate an admin session with.
        // That is a different answer from the engine gate above (`F-DBU`).
        return Err(AdminError::NoPassword.into());
    }
    let password = superuser_password(inner, instance)?;
    (inner.admin_factory)(instance, password)
}

/// Reads the superuser password recorded for `instance`.
fn superuser_password(inner: &Inner, instance: &Instance) -> ServiceResult<SecretString> {
    let reference = inner
        .db(|store| store.get_credential(instance.id()))?
        .ok_or_else(|| DblError::NotFound {
            kind: "credential",
            name: instance.id().as_str().to_owned(),
        })?;
    Ok(inner.secrets.get(&reference)?)
}

pub(crate) fn list_databases(inner: &Inner, id: &InstanceId) -> ServiceResult<Vec<DatabaseInfo>> {
    with_admin(inner, id, |admin| admin.list_databases())
}

pub(crate) fn list_users(inner: &Inner, id: &InstanceId) -> ServiceResult<Vec<UserInfo>> {
    with_admin(inner, id, |admin| admin.list_users())
}

pub(crate) fn create_database(inner: &Inner, id: &InstanceId, name: &str) -> ServiceResult<()> {
    // F-DBU-01 AC4: reject an out-of-allowlist name *before* any statement is
    // sent, with the identifier error rather than a server error.
    let db = Identifier::new(name).map_err(UserFacingError::from)?;
    with_admin(inner, id, |admin| admin.create_database(&db))?;
    announce(inner, id);
    Ok(())
}

pub(crate) fn drop_database(
    inner: &Inner,
    id: &InstanceId,
    name: &str,
    force: bool,
) -> ServiceResult<()> {
    let db = Identifier::new(name).map_err(UserFacingError::from)?;
    with_admin(inner, id, |admin| admin.drop_database(&db, force))?;
    announce(inner, id);
    Ok(())
}

/// Creates a role and grants `level` on `db`, returning the password once.
///
/// If the grant fails after the role exists, the half-created role is dropped so
/// a failure never leaves a login role with no privileges behind. The original
/// grant error is what the caller sees.
pub(crate) fn create_user(
    inner: &Inner,
    id: &InstanceId,
    name: &str,
    db: &str,
    level: PrivilegeLevel,
) -> ServiceResult<SecretString> {
    let user = Identifier::role(name).map_err(UserFacingError::from)?;
    let database = Identifier::new(db).map_err(UserFacingError::from)?;
    let password = with_admin(inner, id, |admin| {
        if !admin.database_exists(&database)? {
            return Err(AdminError::DatabaseNotFound);
        }
        let password = admin.create_user(&user)?;
        if let Err(err) = admin.grant(&user, &database, level) {
            let _ = admin.drop_user(&user);
            return Err(err);
        }
        Ok(password)
    })?;
    announce(inner, id);
    Ok(password)
}

/// Resets the superuser password with the F-DBU-03 AC2 rollback guarantee.
pub(crate) fn reset_superuser_password(
    inner: &Inner,
    id: &InstanceId,
) -> ServiceResult<SecretString> {
    let instance = load(inner, id)?;
    let executor = executor_for(inner, &instance)?;
    let sink = StoreSink::new(inner, &instance);
    let password = executor
        .reset_superuser_password(&sink)
        .map_err(UserFacingError::from)?;
    // The engine and the secret store now agree. Record where the new secret
    // actually landed, in case the keychain was unavailable and the write went
    // to the file fallback. This happens only after the reset succeeded, so a
    // failed reset leaves the pointer (and therefore the keychain) untouched.
    if let Some(backend) = sink.backend() {
        let reference = credential_ref_for(id, backend, instance.superuser());
        inner.db(|store| store.set_credential(&reference))?;
    }
    announce(inner, id);
    Ok(password)
}

/// Tells the UI the admin view of `id` is stale.
fn announce(inner: &Inner, id: &InstanceId) {
    inner.emit(crate::service::ServiceEvent::AdminChanged { id: id.clone() });
}

/// Writes a reset password through the service's secret store.
///
/// The write prefers the keychain exactly like an instance create, and records
/// which backend actually stored it so [`reset_superuser_password`] can persist
/// the new pointer. The value is never logged.
pub(crate) struct StoreSink<'a> {
    inner: &'a Inner,
    reference: crate::model::CredentialRef,
    backend: Mutex<Option<BackendKind>>,
}

impl<'a> StoreSink<'a> {
    fn new(inner: &'a Inner, instance: &Instance) -> Self {
        Self {
            inner,
            reference: credential_ref_for(
                instance.id(),
                crate::model::CredentialBackend::File,
                instance.superuser(),
            ),
            backend: Mutex::new(None),
        }
    }

    /// The backend that actually stored the password, if it was stored.
    pub(crate) fn backend(&self) -> Option<BackendKind> {
        self.backend.lock().ok().and_then(|backend| *backend)
    }
}

impl CredentialSink for StoreSink<'_> {
    fn store(&self, password: &SecretString) -> crate::error::Result<()> {
        let backend = self.inner.secrets.set(&self.reference, password)?;
        if let Ok(mut slot) = self.backend.lock() {
            *slot = Some(backend);
        }
        Ok(())
    }
}
