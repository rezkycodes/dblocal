//! The backend contract: one trait, two real implementations, one fake.
//!
//! A [`SecretBackend`] stores exactly one secret per [`CredentialRef`]. It never
//! sees the instance's other metadata, so it cannot accidentally persist a
//! password anywhere the schema does not expect. All three methods return
//! [`crate::Result`]; [`crate::DblError::Unsupported`] is the specific signal
//! that the backend cannot be used *at all* on this machine, which is what
//! lets [`super::SecretStore`] fall back to the file.

use crate::DblError;
use crate::error::Result;
use crate::model::{CredentialBackend, CredentialRef};
use crate::secret::SecretString;

#[cfg(any(test, feature = "mock"))]
use std::collections::BTreeMap;
#[cfg(any(test, feature = "mock"))]
use std::sync::Mutex;

/// The backend a credential lives in.
///
/// This is an alias for [`CredentialBackend`] on purpose: the persisted
/// `Instance.credential_backend` column, `InstanceView.credential_backend` and
/// the trait's `kind()` must be the same type, or a mapping bug becomes
/// possible. `CredentialBackend` already has exactly the two variants the PRD
/// names (`F-CRD-01`).
pub type BackendKind = CredentialBackend;

/// Longest key reference accepted by either backend.
///
/// The Secret Service specification caps an attribute at 1024 bytes and most
/// filesystems cap a name at 255; the smaller limit keeps the reference usable
/// as both.
pub const MAX_KEY_REF_LEN: usize = 255;

/// Storage for one secret per [`CredentialRef`].
pub trait SecretBackend: Send + Sync {
    /// Which backend this is, so the UI can show where the credential lives.
    fn kind(&self) -> BackendKind;

    /// Reads the secret stored for `reference`.
    ///
    /// Returns [`DblError::NotFound`] when nothing is stored, and
    /// [`DblError::Unsupported`] when this backend cannot operate at all.
    fn get(&self, reference: &CredentialRef) -> Result<SecretString>;

    /// Stores `secret`, replacing any previous value atomically.
    fn set(&self, reference: &CredentialRef, secret: &SecretString) -> Result<()>;

    /// Removes the stored secret.
    ///
    /// Returns [`DblError::NotFound`] when there was nothing to remove.
    fn delete(&self, reference: &CredentialRef) -> Result<()>;
}

/// Validates a key reference at the boundary before it reaches a backend.
///
/// Both backends embed the reference in a file name or a keychain lookup key,
/// so it must be short, printable ASCII with no separators or control bytes.
pub(crate) fn validate_key_ref(reference: &CredentialRef) -> Result<()> {
    validate_key_str(reference.key_ref())
}

/// The one rule set for a key reference, shared by both backends and by the
/// file-name encoder, so they can never disagree about what is legal.
pub(crate) fn validate_key_str(key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(DblError::InvalidInput {
            field: "key_ref",
            reason: "must not be empty".to_owned(),
        });
    }
    if key.len() > MAX_KEY_REF_LEN {
        return Err(DblError::InvalidInput {
            field: "key_ref",
            reason: format!("must be at most {MAX_KEY_REF_LEN} bytes"),
        });
    }
    let safe = key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'));
    if !safe {
        return Err(DblError::InvalidInput {
            field: "key_ref",
            reason: "only ASCII letters, digits, '-', '_', '.' and ':' are allowed".to_owned(),
        });
    }
    if key == "." || key == ".." {
        return Err(DblError::InvalidInput {
            field: "key_ref",
            reason: "must not be a relative path component".to_owned(),
        });
    }
    Ok(())
}

/// In-memory backend for tests and for the GUI mock (`F-GUI`, `mock` feature).
///
/// It models the one behaviour the real backends share with the store: it can
/// report itself [`Unavailable`](FakeBackend::set_available), which is how the
/// fallback path is exercised without a broken D-Bus session.
#[cfg(any(test, feature = "mock"))]
#[derive(Debug, Default)]
pub struct FakeBackend {
    kind: Option<BackendKind>,
    entries: Mutex<BTreeMap<String, SecretString>>,
    available: std::sync::atomic::AtomicBool,
    gets: std::sync::atomic::AtomicUsize,
}

#[cfg(any(test, feature = "mock"))]
impl FakeBackend {
    /// Builds an available fake that reports `kind`.
    pub fn new(kind: BackendKind) -> Self {
        Self {
            kind: Some(kind),
            entries: Mutex::new(BTreeMap::new()),
            available: std::sync::atomic::AtomicBool::new(true),
            gets: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Simulates a backend that cannot be used, such as a headless session.
    pub fn set_available(&self, available: bool) {
        self.available
            .store(available, std::sync::atomic::Ordering::SeqCst);
    }

    /// How many credentials are stored, for assertions.
    pub fn len(&self) -> usize {
        self.entries.lock().map(|map| map.len()).unwrap_or_default()
    }

    /// Whether no credential is stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many times `get` reached this backend, so a test can prove a read
    /// was never routed here.
    pub fn get_calls(&self) -> usize {
        self.gets.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn guard(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, SecretString>>> {
        self.entries.lock().map_err(|_| DblError::Secret {
            reason: "the in-memory backend lock was poisoned".to_owned(),
        })
    }
}

#[cfg(any(test, feature = "mock"))]
impl SecretBackend for FakeBackend {
    fn kind(&self) -> BackendKind {
        self.kind.unwrap_or(BackendKind::File)
    }

    fn get(&self, reference: &CredentialRef) -> Result<SecretString> {
        self.gets.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        validate_key_ref(reference)?;
        if !self.available.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(unavailable(self.kind()));
        }
        self.guard()?
            .get(reference.key_ref())
            .cloned()
            .ok_or_else(|| not_found(reference))
    }

    fn set(&self, reference: &CredentialRef, secret: &SecretString) -> Result<()> {
        validate_key_ref(reference)?;
        if !self.available.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(unavailable(self.kind()));
        }
        self.guard()?
            .insert(reference.key_ref().to_owned(), secret.clone());
        Ok(())
    }

    fn delete(&self, reference: &CredentialRef) -> Result<()> {
        validate_key_ref(reference)?;
        if !self.available.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(unavailable(self.kind()));
        }
        self.guard()?
            .remove(reference.key_ref())
            .map(|_| ())
            .ok_or_else(|| not_found(reference))
    }
}

/// The error used when a backend cannot operate on this machine at all.
pub(crate) fn unavailable(kind: BackendKind) -> DblError {
    DblError::Unsupported {
        reason: format!("the {} backend is not available", kind.as_str()),
    }
}

/// The error used when a backend has no credential for a reference.
pub(crate) fn not_found(reference: &CredentialRef) -> DblError {
    DblError::NotFound {
        kind: "credential",
        name: reference.key_ref().to_owned(),
    }
}

#[cfg(test)]
#[path = "backend/tests.rs"]
mod tests;
