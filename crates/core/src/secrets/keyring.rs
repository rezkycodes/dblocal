//! OS keychain backend, backed by the `keyring` crate.
//!
//! The crate is built with its default `v1` feature, which selects
//! `apple-native-keyring-store` on macOS, `windows-native-keyring-store` on
//! Windows and `zbus-secret-service-keyring-store` on Linux. The Linux store
//! speaks the FreeDesktop Secret Service over D-Bus through `zbus` and ships
//! its crypto in Rust (`crypto-rust`), so this machine's missing `libsecret-1`
//! and `libdbus` development packages are irrelevant to the build.
//!
//! Every keychain call is classified: a credential that simply does not exist
//! becomes [`DblError::NotFound`], while a locked, absent or unreachable store
//! becomes [`DblError::Unsupported`] so [`super::SecretStore`] can fall back to
//! the file without treating a transient environment problem as data loss.
//!
//! [`KeyringBackend`] opens entries through a [`KeychainOpener`]. Production uses
//! [`PlatformKeychain`]; tests inject a scripted opener so every branch runs
//! offline, which is why `cargo test` never touches a real keychain.

use std::sync::LazyLock;

use crate::error::Result;
use crate::model::{CredentialBackend, CredentialRef};
use crate::secret::SecretString;

use super::backend::{BackendKind, SecretBackend, not_found, unavailable, validate_key_ref};

/// Result of the crate's one-time platform store initialisation.
///
/// We only keep a boolean: the upstream error type is not `Clone` and its text
/// is not needed by callers (it may also be platform-specific). A `false` value
/// means the keychain cannot be opened at all on this host.
static STORE_AVAILABLE: LazyLock<bool> = LazyLock::new(|| keyring::Entry::store_status().is_ok());

/// The three `keyring::Entry` calls this module makes.
trait KeychainEntry {
    fn get_password(&self) -> keyring::Result<String>;
    fn set_password(&self, password: &str) -> keyring::Result<()>;
    fn delete_credential(&self) -> keyring::Result<()>;
}

impl KeychainEntry for keyring::Entry {
    fn get_password(&self) -> keyring::Result<String> {
        keyring::Entry::get_password(self)
    }

    fn set_password(&self, password: &str) -> keyring::Result<()> {
        keyring::Entry::set_password(self, password)
    }

    fn delete_credential(&self) -> keyring::Result<()> {
        keyring::Entry::delete_credential(self)
    }
}

/// Opens a keychain entry for one reference.
///
/// This is the seam that keeps [`KeyringBackend`] testable without a live
/// credential store.
trait KeychainOpener: Send + Sync {
    fn open(&self, reference: &CredentialRef) -> Result<Box<dyn KeychainEntry + Send>>;
}

/// Opens real `keyring` entries through the platform store.
#[derive(Debug, Default, Clone, Copy)]
struct PlatformKeychain;

impl KeychainOpener for PlatformKeychain {
    fn open(&self, reference: &CredentialRef) -> Result<Box<dyn KeychainEntry + Send>> {
        Ok(Box::new(entry_for(reference)?))
    }
}

/// Stores credentials in the operating system keychain.
pub struct KeyringBackend {
    opener: Box<dyn KeychainOpener>,
}

impl KeyringBackend {
    /// Builds a backend that uses the platform keychain.
    pub fn new() -> Self {
        Self {
            opener: Box::new(PlatformKeychain),
        }
    }

    /// Builds a backend with an injected opener.
    ///
    /// Used by tests to drive every branch offline without a real keychain.
    #[cfg(test)]
    fn opened_by(opener: Box<dyn KeychainOpener>) -> Self {
        Self { opener }
    }

    /// `true` when the platform store was initialised successfully.
    ///
    /// A successful initialisation only means the store object exists; an
    /// individual operation can still fail because the session bus is gone or
    /// the collection is locked, which is why `get`/`set`/`delete` classify
    /// errors too.
    pub fn is_available() -> bool {
        *STORE_AVAILABLE
    }
}

impl Default for KeyringBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for KeyringBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyringBackend")
    }
}

impl SecretBackend for KeyringBackend {
    fn kind(&self) -> BackendKind {
        CredentialBackend::Keychain
    }

    fn get(&self, reference: &CredentialRef) -> Result<SecretString> {
        let entry = self.opener.open(reference)?;
        match entry.get_password() {
            Ok(secret) => Ok(SecretString::new(secret)),
            Err(err) => Err(classify(err, reference)),
        }
    }

    fn set(&self, reference: &CredentialRef, secret: &SecretString) -> Result<()> {
        let entry = self.opener.open(reference)?;
        entry
            .set_password(secret.expose())
            .map_err(|err| classify(err, reference))
    }

    fn delete(&self, reference: &CredentialRef) -> Result<()> {
        let entry = self.opener.open(reference)?;
        entry
            .delete_credential()
            .map_err(|err| classify(err, reference))
    }
}

/// Builds a keyring entry for a validated reference.
///
/// The `service` argument becomes the collection/label on most platforms and is
/// fixed to `dblocal`. The `user` argument is the lookup key; we use the
/// reference's `key_ref` so the keyring and the file fallback name the same
/// secret, which also keeps the schema-agnostic `dblocal:<instance-id>` shape
/// easy to audit.
fn entry_for(reference: &CredentialRef) -> Result<keyring::Entry> {
    validate_key_ref(reference)?;
    keyring::Entry::new(crate::consts::HOME_DIR_NAME, reference.key_ref())
        .map_err(|err| classify(err, reference))
}

/// Maps a keyring error onto the contract's error taxonomy.
///
/// "Not there" is a normal outcome and must round-trip as `NotFound` so
/// `delete` and `get` behave like the file backend. Everything that means "the
/// keychain is locked, gone, or unusable on this host" is reported as
/// `Unsupported`, which the store treats as the signal to fall back. The
/// upstream error is never copied into the returned message because a store
/// implementation may include a secret value in its text; we log the mapping
/// decision instead.
fn classify(err: keyring::Error, reference: &CredentialRef) -> crate::DblError {
    let mapped = match err {
        keyring::Error::NoEntry => not_found(reference),
        keyring::Error::NoDefaultStore
        | keyring::Error::NoStorageAccess(_)
        | keyring::Error::PlatformFailure(_) => unavailable(BackendKind::Keychain),
        keyring::Error::Ambiguous(_)
        | keyring::Error::BadEncoding(_)
        | keyring::Error::BadDataFormat(_, _)
        | keyring::Error::BadStoreFormat(_)
        | keyring::Error::TooLong(_, _)
        | keyring::Error::Invalid(_, _)
        | keyring::Error::NotSupportedByStore(_) => crate::DblError::Secret {
            reason: "the operating system keychain rejected the credential".to_owned(),
        },
        // keyring-core is #[non_exhaustive]; treat an unknown variant as
        // "backend unusable" so we fall back rather than failing hard.
        _ => unavailable(BackendKind::Keychain),
    };

    tracing::debug!(code = mapped.code(), "keychain error classified");
    mapped
}

#[cfg(test)]
#[path = "keyring/tests.rs"]
mod tests;
