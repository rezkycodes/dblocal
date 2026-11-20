//! Keychain-first credential store with an automatic file fallback.
//!
//! [`SecretStore`] tries the OS keychain for every operation and transparently
//! retries against the file backend when the keychain is unavailable
//! (`F-CRD-01` AC2). The backend actually used is returned to the caller and is
//! the value that must be recorded as `Instance.credential_backend` so the UI
//! can show where the secret lives. Migration between backends is deliberately
//! out of scope.

use crate::DblError;
use crate::error::Result;
use crate::model::{CredentialBackend, CredentialRef};
use crate::paths::DblPaths;
use crate::secret::SecretString;

use super::backend::{BackendKind, SecretBackend, unavailable};
use super::file::FileBackend;
use super::keyring::KeyringBackend;

/// Routes secret operations to the keychain and falls back to the file store.
///
/// The type is generic so unit tests can inject fakes for both sides and force
/// the keychain to report "unavailable" deterministically, without depending on
/// a running Secret Service.
pub struct SecretStore<K = KeyringBackend, F = FileBackend> {
    keyring: K,
    file: F,
}

impl SecretStore<KeyringBackend, FileBackend> {
    /// Builds a store for the configured DBLocal home.
    pub fn for_paths(paths: &DblPaths) -> Self {
        Self::new(KeyringBackend::new(), FileBackend::from_paths(paths))
    }

    /// `true` when the OS keychain backend is expected to be usable.
    pub fn keychain_available() -> bool {
        KeyringBackend::is_available()
    }

    /// Returns which backend `set` would try to use right now.
    pub fn preferred_backend(&self) -> BackendKind {
        if KeyringBackend::is_available() {
            CredentialBackend::Keychain
        } else {
            CredentialBackend::File
        }
    }
}

impl<K, F> SecretStore<K, F>
where
    K: SecretBackend,
    F: SecretBackend,
{
    /// Builds a store from explicit backends.
    pub fn new(keyring: K, file: F) -> Self {
        Self { keyring, file }
    }

    /// Stores a secret, preferring the keychain.
    ///
    /// Returns the backend that actually holds the secret. A successful
    /// keychain write never touches the file; a successful file fallback write
    /// means the keychain was unavailable.
    pub fn set(&self, reference: &CredentialRef, secret: &SecretString) -> Result<BackendKind> {
        match self.keyring.set(reference, secret) {
            Ok(()) => Ok(self.keyring.kind()),
            Err(err) if should_fallback(&err) => {
                tracing::info!(
                    reference = %reference.key_ref(),
                    "keychain unavailable, storing credential in the file fallback"
                );
                self.file.set(reference, secret)?;
                Ok(self.file.kind())
            }
            Err(err) => Err(err),
        }
    }

    /// Reads the secret stored for `reference`.
    ///
    /// The keychain is tried first. If it reports "unavailable", the file
    /// fallback is consulted. A `NotFound` from the keychain is not treated as
    /// a reason to look in the file: each credential lives in exactly one
    /// backend, recorded on the instance.
    pub fn get(&self, reference: &CredentialRef) -> Result<SecretString> {
        match self.keyring.get(reference) {
            Ok(secret) => Ok(secret),
            Err(err) if should_fallback(&err) => {
                tracing::info!(
                    reference = %reference.key_ref(),
                    "keychain unavailable, reading credential from the file fallback"
                );
                self.file.get(reference)
            }
            Err(err) => Err(err),
        }
    }

    /// Deletes the secret from the backend that holds it.
    ///
    /// When the keychain is available this removes it there; when it is not,
    /// the file fallback is tried instead. A `NotFound` means nothing was stored
    /// in the consulted backend. To be safe during teardown (for example when
    /// an instance is deleted after the keychain went away), callers that need
    /// "remove from everywhere" should try the other backend too when the first
    /// reports `NotFound`; the service layer owns that policy.
    pub fn delete(&self, reference: &CredentialRef) -> Result<()> {
        match self.keyring.delete(reference) {
            Ok(()) => Ok(()),
            Err(err) if should_fallback(&err) => {
                tracing::info!(
                    reference = %reference.key_ref(),
                    "keychain unavailable, deleting credential from the file fallback"
                );
                self.file.delete(reference)
            }
            Err(err) => Err(err),
        }
    }

    /// Deletes the credential from every backend, ignoring a missing entry.
    ///
    /// `F-CRD-01` AC3 requires the stored secret to be removed when an instance
    /// is deleted. Because the instance record is the source of truth for which
    /// backend was used, this is a convenience for callers that do not know (or
    /// no longer trust) that record: it removes from the keychain when possible
    /// and always attempts the file, returning success once neither holds the
    /// secret.
    pub fn delete_everywhere(&self, reference: &CredentialRef) -> Result<()> {
        let keyring_result = match self.keyring.delete(reference) {
            Ok(()) => Ok(()),
            Err(DblError::NotFound { .. }) | Err(DblError::Unsupported { .. }) => Ok(()),
            Err(err) => Err(err),
        };
        let file_result = match self.file.delete(reference) {
            Ok(()) => Ok(()),
            Err(DblError::NotFound { .. }) => Ok(()),
            Err(err) => Err(err),
        };

        // Prefer the more specific keychain failure (a real secret error)
        // over a file I/O failure, but never hide either.
        match (keyring_result, file_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(err), _) => Err(err),
            (Ok(()), Err(err)) => Err(err),
        }
    }

    /// Reads the secret from the backend named by the instance record.
    ///
    /// When the recorded backend is unavailable the store falls back to the
    /// other one, which keeps an instance readable after the keychain
    /// disappears (for example on a headless login). The caller that needs to
    /// persist a backend change after such a recovery is outside this module.
    pub fn get_from(&self, kind: BackendKind, reference: &CredentialRef) -> Result<SecretString> {
        let (primary, secondary): (&dyn SecretBackend, &dyn SecretBackend) = match kind {
            CredentialBackend::Keychain => (&self.keyring, &self.file),
            CredentialBackend::File => (&self.file, &self.keyring),
        };
        match primary.get(reference) {
            Ok(secret) => Ok(secret),
            Err(err) if should_fallback(&err) => secondary.get(reference),
            Err(err) => Err(err),
        }
    }

    /// Reference to the file backend, for callers that need its path.
    pub fn file_backend(&self) -> &F {
        &self.file
    }

    /// Reference to the keychain backend.
    pub fn keyring_backend(&self) -> &K {
        &self.keyring
    }
}

/// A `NotFound` is a real answer; an `Unsupported` keychain means "try the
/// file". Every other error is returned to the caller unchanged.
fn should_fallback(err: &DblError) -> bool {
    matches!(err, DblError::Unsupported { .. })
}

/// Builds a [`CredentialRef`] that follows the `dblocal:<instance-id>` key
/// convention, pointing at the backend that actually stored the secret.
///
/// This is the canonical way to produce the reference recorded in
/// `InstanceView.credential_backend` and in the instance row.
pub fn credential_ref_for(
    instance_id: &crate::model::InstanceId,
    backend: BackendKind,
    username: impl Into<String>,
) -> CredentialRef {
    let key_ref = format!("dblocal:{instance_id}");
    CredentialRef::new(instance_id.clone(), backend, key_ref, username)
}

/// Explicitly marks that a backend is unusable; exposed so callers outside this
/// module can construct the same error shape in tests without reaching into the
/// private helper.
pub fn backend_unavailable(kind: BackendKind) -> DblError {
    unavailable(kind)
}

#[cfg(test)]
#[path = "store/tests.rs"]
mod tests;
