//! Deterministic in-memory admin operations for [`super::MockService`].
//!
//! The mock mirrors the real service's rules without an engine: PostgreSQL,
//! MySQL and MariaDB are supported, Valkey is a typed "not supported" (no named
//! databases or login roles), and an instance seeded without a credential
//! behaves like a no-password instance and is unsupported for the same reason.
//!
//! Nothing here stores a role password or the superuser password; a create-user
//! or reset returns a fresh value exactly once, and the superuser reset updates
//! the map the connect URI reads so `uri` and the reset agree, as `F-DBU-03` AC1
//! requires.
//!
//! # Scripted scenarios
//!
//! [`super::AdminScenario`] lets the demo build and the GUI tests render every
//! state the tab has: an empty list, many rows, a failing mutation and a failing
//! reset. `Empty` and `ManyRows` are *sticky* (a demo screen must keep the state
//! it was asked for); `FailNextMutation` and `ResetFailure` are consumed by one
//! call.

use std::sync::{Arc, Mutex};

use crate::admin::{DatabaseInfo, Identifier, PrivilegeLevel, UserInfo};
use crate::model::{AuthMode, EngineKey, Instance, InstanceId};
use crate::secret::SecretString;
use crate::service::{ServiceEvent, ServiceResult, UserFacingError};

use super::{AdminScenario, Inner, lock, not_found};

/// The rule the real and mock services share: Valkey has no database/user model
/// (typed "not supported"), and a no-password instance has no credential to
/// authenticate an admin session with. PostgreSQL, MySQL and MariaDB are all
/// supported.
fn unsupported(instance: &Instance) -> Option<UserFacingError> {
    if instance.engine() == EngineKey::Valkey {
        return Some(UserFacingError::from(crate::admin::AdminError::Unsupported));
    }
    if instance.auth() != AuthMode::Password {
        return Some(UserFacingError::from(crate::admin::AdminError::NoPassword));
    }
    None
}

fn not_found_error(id: &InstanceId) -> UserFacingError {
    UserFacingError::from(not_found("instance", id))
}

fn admin_error(err: crate::admin::AdminError) -> UserFacingError {
    UserFacingError::from(err)
}

/// Loads one instance, mapping a missing row to the typed not-found error.
fn load(inner: &Arc<Mutex<Inner>>, id: &InstanceId) -> Result<Instance, UserFacingError> {
    lock(inner)
        .instances
        .get(id)
        .cloned()
        .ok_or_else(|| not_found_error(id))
}

/// The sticky scenarios (`Empty`, `ManyRows`), in queue order.
fn sticky(g: &Inner) -> Option<&AdminScenario> {
    g.admin_scenarios
        .iter()
        .find(|scenario| matches!(scenario, AdminScenario::Empty | AdminScenario::ManyRows(_)))
}

/// Removes and returns the first one-shot scenario matching `wanted`.
fn take_once(g: &mut Inner, wanted: impl Fn(&AdminScenario) -> bool) -> Option<AdminScenario> {
    let index = g
        .admin_scenarios
        .iter()
        .position(|scenario| wanted(scenario))?;
    g.admin_scenarios.remove(index)
}

/// The default databases every PostgreSQL instance starts with.
fn seeded_databases(owner: &str) -> Vec<DatabaseInfo> {
    vec![
        DatabaseInfo::new(crate::consts::DEFAULT_DATABASE, owner.to_owned(), 8_912_896),
        DatabaseInfo::new("app_development", owner.to_owned(), 8_650_752),
    ]
}

/// `db_00` .. `db_{n-1}`, so a long list is stable and obviously synthetic.
fn many_databases(owner: &str, count: usize) -> Vec<DatabaseInfo> {
    (0..count)
        .map(|index| {
            DatabaseInfo::new(
                format!("db_{index:02}"),
                owner.to_owned(),
                1_048_576 + index as i64 * 65_536,
            )
        })
        .collect()
}

/// The databases the mock reports for `id`, honouring a sticky scenario.
pub(crate) fn list_databases(
    inner: &Arc<Mutex<Inner>>,
    id: &InstanceId,
) -> ServiceResult<Vec<DatabaseInfo>> {
    let instance = load(inner, id)?;
    if let Some(err) = unsupported(&instance) {
        return Err(err);
    }
    let g = lock(inner);
    let owner = instance.superuser();
    if let Some(scenario) = sticky(&g) {
        return Ok(match scenario {
            AdminScenario::Empty => Vec::new(),
            AdminScenario::ManyRows(count) => many_databases(&owner, *count),
            _ => seeded_databases(&owner),
        });
    }
    Ok(g.databases
        .get(id)
        .cloned()
        .unwrap_or_else(|| seeded_databases(&owner)))
}

pub(crate) fn list_users(
    inner: &Arc<Mutex<Inner>>,
    id: &InstanceId,
) -> ServiceResult<Vec<UserInfo>> {
    let instance = load(inner, id)?;
    if let Some(err) = unsupported(&instance) {
        return Err(err);
    }
    let g = lock(inner);
    if matches!(sticky(&g), Some(AdminScenario::Empty)) {
        return Ok(Vec::new());
    }
    Ok(g.users.get(id).cloned().unwrap_or_else(|| {
        vec![UserInfo::new(
            crate::consts::SUPERUSER_POSTGRESQL,
            true,
            true,
        )]
    }))
}

/// The typed failure a `FailNextMutation` scenario injects, if one is queued.
fn next_mutation_failure(g: &mut Inner) -> Option<UserFacingError> {
    take_once(g, |scenario| {
        matches!(scenario, AdminScenario::FailNextMutation)
    })
    .map(|_| admin_error(crate::admin::AdminError::Unsupported))
}

pub(crate) fn create_database(
    inner: &Arc<Mutex<Inner>>,
    id: &InstanceId,
    name: &str,
) -> ServiceResult<()> {
    let instance = load(inner, id)?;
    if let Some(err) = unsupported(&instance) {
        return Err(err);
    }
    let validated = Identifier::new(name).map_err(admin_error)?;
    let mut g = lock(inner);
    if let Some(err) = next_mutation_failure(&mut g) {
        return Err(err);
    }
    let owner = instance.superuser().to_owned();
    let list = g
        .databases
        .entry(id.clone())
        .or_insert_with(|| seeded_databases(&owner));
    if list.iter().any(|db| db.name() == validated.as_str()) {
        return Err(admin_error(crate::admin::AdminError::DatabaseExists));
    }
    list.push(DatabaseInfo::new(validated.as_str(), owner, 8_388_608));
    g.emit(ServiceEvent::AdminChanged { id: id.clone() });
    Ok(())
}

pub(crate) fn drop_database(
    inner: &Arc<Mutex<Inner>>,
    id: &InstanceId,
    name: &str,
    _force: bool,
) -> ServiceResult<()> {
    let instance = load(inner, id)?;
    if let Some(err) = unsupported(&instance) {
        return Err(err);
    }
    let validated = Identifier::new(name).map_err(admin_error)?;
    let mut g = lock(inner);
    if let Some(err) = next_mutation_failure(&mut g) {
        return Err(err);
    }
    let owner = instance.superuser().to_owned();
    let list = g
        .databases
        .entry(id.clone())
        .or_insert_with(|| seeded_databases(&owner));
    let before = list.len();
    list.retain(|db| db.name() != validated.as_str());
    if list.len() == before {
        return Err(admin_error(crate::admin::AdminError::DatabaseNotFound));
    }
    g.emit(ServiceEvent::AdminChanged { id: id.clone() });
    Ok(())
}

pub(crate) fn create_user(
    inner: &Arc<Mutex<Inner>>,
    id: &InstanceId,
    name: &str,
    db: &str,
    _level: PrivilegeLevel,
) -> ServiceResult<SecretString> {
    let instance = load(inner, id)?;
    if let Some(err) = unsupported(&instance) {
        return Err(err);
    }
    let role = Identifier::role(name).map_err(admin_error)?;
    let database = Identifier::new(db).map_err(admin_error)?;
    let mut g = lock(inner);
    if let Some(err) = next_mutation_failure(&mut g) {
        return Err(err);
    }
    let owner = instance.superuser().to_owned();
    let databases = g
        .databases
        .entry(id.clone())
        .or_insert_with(|| seeded_databases(&owner));
    if !databases.iter().any(|d| d.name() == database.as_str()) {
        return Err(admin_error(crate::admin::AdminError::DatabaseNotFound));
    }
    let users = g.users.entry(id.clone()).or_insert_with(|| {
        vec![UserInfo::new(
            crate::consts::SUPERUSER_POSTGRESQL,
            true,
            true,
        )]
    });
    if users.iter().any(|u| u.name() == role.as_str()) {
        return Err(admin_error(crate::admin::AdminError::UserExists));
    }
    users.push(UserInfo::new(role.as_str(), true, false));
    let password = SecretString::new(format!("pwd-user-{:08x}", g.det.next_u64() as u32));
    g.emit(ServiceEvent::AdminChanged { id: id.clone() });
    Ok(password)
}

pub(crate) fn reset_superuser_password(
    inner: &Arc<Mutex<Inner>>,
    id: &InstanceId,
) -> ServiceResult<SecretString> {
    let instance = load(inner, id)?;
    if let Some(err) = unsupported(&instance) {
        return Err(err);
    }
    let mut g = lock(inner);
    // F-DBU-03 AC2: a scripted reset failure leaves the stored password map
    // untouched, so the old password still works and no partial write happened.
    if take_once(&mut g, |scenario| {
        matches!(scenario, AdminScenario::ResetFailure)
    })
    .is_some()
    {
        return Err(UserFacingError::from(
            crate::admin::AdminError::PasswordVerify,
        ));
    }
    let password = SecretString::new(format!("pwd-reset-{:08x}", g.det.next_u64() as u32));
    // Keep the map `connection_uri` reads consistent with the reset, so the
    // printed URI carries the new password (F-DBU-03 AC1).
    g.passwords.insert(id.clone(), password.clone());
    g.emit(ServiceEvent::AdminChanged { id: id.clone() });
    Ok(password)
}
