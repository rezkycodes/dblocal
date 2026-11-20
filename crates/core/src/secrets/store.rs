//! Backend-routed credential store with a keychain-preferred writer.
//!
//! [`SecretStore`] is the only place that knows about both backends:
//!
//! - **read** routes strictly by [`CredentialRef::backend()`], the backend
//!   recorded on the instance. A credential lives in exactly one backend, so a
//!   read never crosses over: an available-but-empty keychain must not mask a
//!   secret that was written to the file fallback while the keychain was down
//!   (the "created over SSH, then opened on the desktop" case);
//! - **write** prefers the keychain and falls back to the file when the
//!   keychain reports itself unavailable (`F-CRD-01` AC2), returning the
//!   backend that actually stored the secret so the caller can persist it;
//! - **delete** clears the credential from both backends, because tearing an
//!   instance down must not leave a copy behind on the other side.
//!
//! Migration between backends is deliberately out of scope.

use crate::DblError;
use crate::error::Result;
use crate::model::{CredentialBackend, CredentialRef};
use crate::paths::DblPaths;
use crate::secret::SecretString;

use super::backend::{BackendKind, SecretBackend, unavailable};
use super::file::FileBackend;
use super::keyring::KeyringBackend;

/// Routes secret operations to the keychain and the file fallback.
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
    /// means the keychain was unavailable. The caller records the returned
    /// backend with the instance, so later reads route back here.
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

    /// Reads the secret from the backend recorded on `reference`.
    ///
    /// Routing is by [`CredentialRef::backend()`] and nothing else. If that
    /// backend is unavailable (locked or absent keychain), its `Unsupported`
    /// error is returned unchanged and the other backend is never consulted.
    pub fn get(&self, reference: &CredentialRef) -> Result<SecretString> {
        self.backend_for(reference.backend()).get(reference)
    }

    /// Removes the credential from both backends.
    ///
    /// `F-CRD-01` AC3 requires the stored secret to disappear when an instance
    /// is deleted, and the instance record may be stale or the backend may have
    /// changed since the write. Removing from both is therefore unconditional;
    /// a missing entry or an unavailable backend is not an error, but any real
    /// failure (invalid key reference, I/O error) is reported.
    pub fn delete(&self, reference: &CredentialRef) -> Result<()> {
        let keyring_result = self.keyring.delete(reference);
        let file_result = self.file.delete(reference);

        // A real keychain failure takes precedence over a file failure, but
        // neither may be hidden.
        let keyring_result = absorbed(keyring_result);
        let file_result = absorbed(file_result);
        match (keyring_result, file_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(err), _) => Err(err),
            (Ok(()), Err(err)) => Err(err),
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

    /// The backend that owns `kind`, used to route a read.
    fn backend_for(&self, kind: BackendKind) -> &dyn SecretBackend {
        match kind {
            CredentialBackend::Keychain => &self.keyring,
            CredentialBackend::File => &self.file,
        }
    }
}

/// A `NotFound` is a real answer; an `Unsupported` keychain means "try the
/// file" on write. Every other error is returned to the caller unchanged.
fn should_fallback(err: &DblError) -> bool {
    matches!(err, DblError::Unsupported { .. })
}

/// Whether a delete outcome is a success for the purpose of "remove everywhere".
///
/// A credential that is already gone, or a backend that cannot be reached at
/// all, leaves nothing behind; only a real failure is propagated.
fn absorbed(result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(DblError::NotFound { .. }) | Err(DblError::Unsupported { .. }) => Ok(()),
        Err(err) => Err(err),
    }
}

/// Builds a [`CredentialRef`] that follows the `dblocal:<instance-id>` key
/// convention, pointing at the backend that actually stored the secret.
///
/// This is the canonical way to produce the reference recorded in
/// `InstanceView.credential_backend` and in the instance row. After a `set`,
/// pass the returned backend here so the next `get` routes to the same place.
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
