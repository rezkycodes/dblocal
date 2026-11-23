//! The engine-facing admin operations, one trait both executors implement.
//!
//! Kept in `admin/` (not in the private `instance::admin_ops` module) so the
//! PostgreSQL and MySQL/MariaDB executors can implement it without reaching into
//! the service wiring, and so the wiring can inject a fake in tests.

use super::error::AdminResult;
use super::postgres::{DatabaseInfo, UserInfo};
use super::reset::CredentialSink;
use super::sql::{Identifier, PrivilegeLevel};

/// The engine-facing half of admin, abstracted so the wiring is testable
/// without a live engine.
pub trait AdminExecutor: Send + Sync {
    /// Lists databases with owner and size.
    fn list_databases(&self) -> AdminResult<Vec<DatabaseInfo>>;
    /// Lists login-capable roles.
    fn list_users(&self) -> AdminResult<Vec<UserInfo>>;
    /// Creates a database.
    fn create_database(&self, db: &Identifier) -> AdminResult<()>;
    /// Drops a database, forcing only when `force`.
    fn drop_database(&self, db: &Identifier, force: bool) -> AdminResult<()>;
    /// Creates a login role and returns its password once.
    fn create_user(&self, user: &Identifier) -> AdminResult<crate::secret::SecretString>;
    /// Whether `db` exists, checked before a role is created in it.
    fn database_exists(&self, db: &Identifier) -> AdminResult<bool>;
    /// Grants `level` on `db` to `user`.
    fn grant(&self, user: &Identifier, db: &Identifier, level: PrivilegeLevel) -> AdminResult<()>;
    /// Drops a role (used to undo a half-created user).
    fn drop_user(&self, user: &Identifier) -> AdminResult<()>;
    /// Resets the superuser password with the F-DBU-03 AC2 guarantee.
    fn reset_superuser_password(
        &self,
        sink: &dyn CredentialSink,
    ) -> AdminResult<crate::secret::SecretString>;
}
