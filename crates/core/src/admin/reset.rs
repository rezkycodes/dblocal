//! The F-DBU-03 AC2 password-reset state machine, shared by every engine.
//!
//! `apply → verify → store`, rolling back to the old password on any failure
//! after the engine changed. Kept pure over two trait seams so both the
//! PostgreSQL (`PgAdmin`) and MySQL/MariaDB (`MysqlAdmin`) executors reuse one
//! tested implementation instead of duplicating the rollback rules.

use crate::secret::SecretString;

use super::error::{AdminError, AdminResult, RollbackState};

/// Sink for persisting a newly applied credential.
///
/// The real implementation is the keychain, injected by the wiring task; tests
/// inject failures. It must never log the value it receives.
pub trait CredentialSink: Send + Sync {
    /// Stores the new password. Returns `Err` if persistence fails.
    fn store(&self, password: &SecretString) -> crate::error::Result<()>;
}

/// The engine-facing half of a password reset, abstracted so the state machine
/// can be unit-tested with injected failures instead of a live server.
///
/// `apply` takes the password to **connect with** as well as the one to set,
/// because the connection credential and the role password diverge mid-reset:
/// after the engine holds the new password, a rollback must connect with the new
/// one to set the old one back.
pub trait PasswordEngine {
    /// Connects using `connect_as` and sets the superuser password to `target`.
    fn apply(&self, connect_as: &SecretString, target: &SecretString) -> AdminResult<()>;
    /// Opens a fresh connection with `password` and confirms it authenticates.
    fn verify(&self, password: &SecretString) -> AdminResult<()>;
}

/// The pure F-DBU-03 AC2 state machine: apply → verify → store, rolling back to
/// `old_password` on any failure after the engine changed.
///
/// Every failure point returns a typed error carrying only booleans, and the
/// rollback path itself is exercised by the injected-failure tests.
pub(crate) fn reset_with(
    engine: &dyn PasswordEngine,
    sink: &dyn CredentialSink,
    old_password: &SecretString,
    new_password: SecretString,
) -> AdminResult<SecretString> {
    // 1. Apply. Connect with the old password, set the new one. If this fails
    //    nothing changed yet, so there is nothing to roll back.
    engine.apply(old_password, &new_password)?;

    let state = RollbackState {
        engine_changed: true,
        sink_failed: false,
        rollback_failed: false,
    };

    // 2. The engine now holds the new password. Prove it before persisting it.
    if engine.verify(&new_password).is_err() {
        return Err(rollback(
            engine,
            &new_password,
            old_password,
            state,
            AdminError::PasswordVerify,
        ));
    }

    // 3. Persist. If the sink refuses, restore the old password so the stored
    //    credential and the engine never disagree.
    if sink.store(&new_password).is_err() {
        let state = RollbackState {
            sink_failed: true,
            ..state
        };
        return Err(rollback(
            engine,
            &new_password,
            old_password,
            state,
            AdminError::CredentialStore,
        ));
    }

    Ok(new_password)
}

/// Restores the old password and returns the error that explains the failure.
///
/// The rollback connects with `current` (the password the engine now holds) to
/// set `old_password` back. When the restore also fails, the returned error is
/// `RollbackFailed` and says the state is inconsistent. Only booleans travel,
/// never a secret.
fn rollback(
    engine: &dyn PasswordEngine,
    current: &SecretString,
    old_password: &SecretString,
    state: RollbackState,
    cause: AdminError,
) -> AdminError {
    let restored = engine
        .apply(current, old_password)
        .and_then(|()| engine.verify(old_password));
    match restored {
        Ok(()) => cause,
        Err(_) => AdminError::RollbackFailed(RollbackState {
            rollback_failed: true,
            ..state
        }),
    }
}

#[cfg(test)]
#[path = "reset/tests.rs"]
mod tests;
