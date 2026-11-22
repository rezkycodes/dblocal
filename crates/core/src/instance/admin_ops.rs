//! Database and user administration through `DblService` (`F-DBU-01..03`).
//!
//! Every call runs against a **running** instance over loopback. Credentials are
//! read from the secret store and handed to [`PgAdmin`] as an `AdminTarget`; they
//! are never placed in an event, a log line, an error or a returned view — the
//! only value that leaves this module is the one-time password a create-user or
//! password-reset call is defined to return (`F-DBU-02` AC2, `F-DBU-03` AC1).
//!
//! MySQL/MariaDB and Valkey have no executor yet (the MySQL one is `W4-A2`), so
//! they return the module's typed, stable-coded "not supported for this engine
//! yet" error instead of an empty list or a panic.

use std::sync::Mutex;
use std::time::Duration;

use crate::DblError;
use crate::admin::{
    AdminError, AdminResult, AdminTarget, CredentialSink, DatabaseInfo, Identifier, PgAdmin,
    PrivilegeLevel, UserInfo,
};
use crate::consts;
use crate::model::{EngineKey, Instance, InstanceId};
use crate::secret::SecretString;
use crate::secrets::{BackendKind, credential_ref_for};
use crate::service::{ServiceResult, UserFacingError};

use super::read_ops::load;
use super::service::Inner;

/// Runs `f` against a PostgreSQL admin client for `instance`.
///
/// Rejects every other engine with [`AdminError::Unsupported`] (DBL-8111) before
/// any connection is attempted, and rejects an instance whose credential cannot
/// be read with the store's typed error.
fn with_pg_admin<T>(
    inner: &Inner,
    id: &InstanceId,
    f: impl FnOnce(&PgAdmin) -> AdminResult<T>,
) -> ServiceResult<T> {
    let instance = load(inner, id)?;
    let admin = pg_admin(inner, &instance)?;
    Ok(f(&admin)?)
}

/// Builds the admin client for one instance, reading the superuser credential.
fn pg_admin(inner: &Inner, instance: &Instance) -> ServiceResult<PgAdmin> {
    if instance.engine() != EngineKey::Postgresql {
        return Err(AdminError::Unsupported.into());
    }
    let password = superuser_password(inner, instance)?;
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
    Ok(PgAdmin::new(target))
}

/// Reads the superuser password recorded for `instance`.
fn superuser_password(inner: &Inner, instance: &Instance) -> ServiceResult<SecretString> {
    if instance.auth() != crate::model::AuthMode::Password {
        // With no password there is nothing to authenticate an admin session
        // with (and nothing to reset later).
        return Err(AdminError::Unsupported.into());
    }
    let reference = inner
        .db(|store| store.get_credential(instance.id()))?
        .ok_or_else(|| DblError::NotFound {
            kind: "credential",
            name: instance.id().as_str().to_owned(),
        })?;
    Ok(inner.secrets.get(&reference)?)
}

pub(crate) fn list_databases(inner: &Inner, id: &InstanceId) -> ServiceResult<Vec<DatabaseInfo>> {
    with_pg_admin(inner, id, |admin| admin.list_databases())
}

pub(crate) fn list_users(inner: &Inner, id: &InstanceId) -> ServiceResult<Vec<UserInfo>> {
    with_pg_admin(inner, id, |admin| admin.list_users())
}

pub(crate) fn create_database(inner: &Inner, id: &InstanceId, name: &str) -> ServiceResult<()> {
    // F-DBU-01 AC4: reject an out-of-allowlist name *before* any statement is
    // sent, with the identifier error rather than a server error.
    let db = Identifier::new(name).map_err(UserFacingError::from)?;
    with_pg_admin(inner, id, |admin| admin.create_database(&db))?;
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
    with_pg_admin(inner, id, |admin| admin.drop_database(&db, force))?;
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
    let password = with_pg_admin(inner, id, |admin| {
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
    let admin = pg_admin(inner, &instance)?;
    let sink = StoreSink::new(inner, &instance);
    let password = admin
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
struct StoreSink<'a> {
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

    fn backend(&self) -> Option<BackendKind> {
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
