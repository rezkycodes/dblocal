//! Credential storage: OS keychain, `0600` file fallback, password generation.
//!
//! Invariants this module enforces (AGENTS.md 4 and 5, PRD `F-CRD-01`):
//!
//! - a password never reaches SQLite, a log line, an error message, a command
//!   line or `Debug` output;
//! - the OS keychain is preferred on write and the `0600` file is the
//!   documented fallback, reported honestly through [`BackendKind`];
//! - a read routes to the backend recorded on the instance
//!   ([`crate::model::CredentialRef::backend`]) and never guesses;
//! - every buffer that holds a secret is zeroed on drop.
//!
//! Layout: [`backend`] defines the [`SecretBackend`] trait and the in-memory
//! fake, [`keyring`] wraps the `keyring` crate, [`file`] owns the `0600`
//! fallback, [`store`] combines them and [`password`] generates values.
//!
//! Build note: `keyring` 4.2's default `v1` feature resolves to
//! `zbus-secret-service-keyring-store` on Linux, which is pure Rust, so the
//! workspace builds without `libsecret-1` or `libdbus` development packages.
//! See `docs/architecture/secrets.md`.

mod backend;
mod file;
mod keyring;
mod password;
mod store;

pub use backend::{BackendKind, MAX_KEY_REF_LEN, SecretBackend};
pub use file::FileBackend;
pub use keyring::KeyringBackend;
pub use password::{ALPHABET, InstanceSecret, generate_password};
pub use store::{SecretStore, backend_unavailable, credential_ref_for};

#[cfg(any(test, feature = "mock"))]
pub use backend::FakeBackend;

#[cfg(test)]
#[path = "secrets/redaction/tests.rs"]
mod redaction_tests;
