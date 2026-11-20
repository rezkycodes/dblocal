//! Tests for the keychain backend.
//!
//! Everything here runs offline: [`KeyringBackend`] is driven through a
//! scripted [`KeychainOpener`], so no Secret Service, Keychain or Credential
//! Manager is touched. The single test that talks to a real keychain is
//! `#[ignore]`d, because it needs a live store and may prompt the user to
//! unlock it.

use super::*;
use crate::model::InstanceId;

fn reference(key: &str) -> CredentialRef {
    CredentialRef::new(
        InstanceId::new(),
        CredentialBackend::Keychain,
        key,
        "postgres",
    )
}

/// One scripted answer for one entry operation.
type Answer<T> = std::result::Result<T, fn() -> keyring::Error>;

/// A `KeychainEntry` whose three operations answer from a script.
struct ScriptedEntry {
    get: Answer<String>,
    set: Answer<()>,
    delete: Answer<()>,
}

impl KeychainEntry for ScriptedEntry {
    fn get_password(&self) -> keyring::Result<String> {
        match &self.get {
            Ok(value) => Ok(value.clone()),
            Err(make) => Err(make()),
        }
    }

    fn set_password(&self, password: &str) -> keyring::Result<()> {
        assert!(
            !password.is_empty(),
            "the backend must pass the value through"
        );
        match &self.set {
            Ok(()) => Ok(()),
            Err(make) => Err(make()),
        }
    }

    fn delete_credential(&self) -> keyring::Result<()> {
        match &self.delete {
            Ok(()) => Ok(()),
            Err(make) => Err(make()),
        }
    }
}

/// A `KeychainOpener` that hands out a copy of one scripted entry.
struct ScriptedOpener {
    entry: ScriptedEntry,
}

impl KeychainOpener for ScriptedOpener {
    fn open(&self, _reference: &CredentialRef) -> Result<Box<dyn KeychainEntry + Send>> {
        Ok(Box::new(ScriptedEntry {
            get: self.entry.get.clone(),
            set: self.entry.set,
            delete: self.entry.delete,
        }))
    }
}

fn backend_with(get: Answer<String>, set: Answer<()>, delete: Answer<()>) -> KeyringBackend {
    KeyringBackend::opened_by(Box::new(ScriptedOpener {
        entry: ScriptedEntry { get, set, delete },
    }))
}

fn ok_backend() -> KeyringBackend {
    backend_with(Ok("value-from-keychain".to_owned()), Ok(()), Ok(()))
}

#[test]
fn kind_is_keychain() {
    assert_eq!(KeyringBackend::new().kind(), CredentialBackend::Keychain);
}

#[test]
fn default_and_debug_do_not_leak() {
    let backend = KeyringBackend::default();
    assert_eq!(backend.kind(), CredentialBackend::Keychain);
    assert_eq!(format!("{backend:?}"), "KeyringBackend");
}

#[test]
fn store_status_is_reported_without_panicking() {
    // Either answer is valid on a build host; the point is that asking never
    // panics and that a failure is visible through the same predicate the
    // store uses to decide whether to fall back.
    assert_eq!(
        KeyringBackend::is_available(),
        keyring::Entry::store_status().is_ok(),
        "the cached status must match the crate's own answer"
    );
}

#[test]
fn get_returns_a_secret_that_redacts() {
    let backend = ok_backend();
    let secret = backend.get(&reference("dblocal:get-ok")).expect("get");
    assert_eq!(secret.expose(), "value-from-keychain");
    assert_eq!(format!("{secret:?}"), "SecretString(***)");
    assert_eq!(format!("{secret}"), "***");
}

#[test]
fn set_and_delete_report_success() {
    let backend = ok_backend();
    let ref_ = reference("dblocal:ok");
    backend
        .set(&ref_, &SecretString::from("pass-through-value"))
        .expect("set");
    backend.delete(&ref_).expect("delete");
}

#[test]
fn unsafe_reference_is_rejected_at_the_boundary() {
    // The platform opener validates before constructing a keyring entry, so a
    // bad key is rejected even on a host with no usable keychain.
    let backend = KeyringBackend::new();
    let bad = reference("../escape");
    assert_eq!(
        backend.get(&bad).expect_err("get").code(),
        crate::error::CODE_INVALID_INPUT
    );
    assert_eq!(
        backend
            .set(&bad, &SecretString::from("x"))
            .expect_err("set")
            .code(),
        crate::error::CODE_INVALID_INPUT
    );
    assert_eq!(
        backend.delete(&bad).expect_err("delete").code(),
        crate::error::CODE_INVALID_INPUT
    );
}

#[test]
fn operations_map_no_entry_to_not_found() {
    let backend = backend_with(
        Err(|| keyring::Error::NoEntry),
        Err(|| keyring::Error::NoEntry),
        Err(|| keyring::Error::NoEntry),
    );
    let ref_ = reference("dblocal:missing");
    assert_eq!(
        backend.get(&ref_).expect_err("get").code(),
        crate::error::CODE_NOT_FOUND
    );
    assert_eq!(
        backend
            .set(&ref_, &SecretString::from("x"))
            .expect_err("set")
            .code(),
        crate::error::CODE_NOT_FOUND
    );
    assert_eq!(
        backend.delete(&ref_).expect_err("delete").code(),
        crate::error::CODE_NOT_FOUND
    );
}

#[test]
fn operations_map_an_unusable_store_to_unsupported() {
    let backend = backend_with(
        Err(|| keyring::Error::NoDefaultStore),
        Err(|| keyring::Error::PlatformFailure("bus down".into())),
        Err(|| keyring::Error::NoStorageAccess("locked".into())),
    );
    let ref_ = reference("dblocal:unusable");
    assert_eq!(
        backend.get(&ref_).expect_err("get").code(),
        crate::error::CODE_UNSUPPORTED
    );
    assert_eq!(
        backend
            .set(&ref_, &SecretString::from("x"))
            .expect_err("set")
            .code(),
        crate::error::CODE_UNSUPPORTED
    );
    assert_eq!(
        backend.delete(&ref_).expect_err("delete").code(),
        crate::error::CODE_UNSUPPORTED
    );
}

#[test]
fn no_entry_maps_to_not_found() {
    let ref_ = reference("dblocal:missing");
    let err = classify(keyring::Error::NoEntry, &ref_);
    assert_eq!(err.code(), crate::error::CODE_NOT_FOUND);
    // The key reference is metadata, not a secret. The user-facing message must
    // still never carry it.
    assert!(!err.user_message().contains("dblocal:missing"));
}

#[test]
fn absent_store_maps_to_unsupported() {
    let ref_ = reference("dblocal:absent-store");
    for upstream in [
        keyring::Error::NoDefaultStore,
        keyring::Error::NoStorageAccess("locked".into()),
        keyring::Error::PlatformFailure("bus down".into()),
    ] {
        let err = classify(upstream, &ref_);
        assert_eq!(
            err.code(),
            crate::error::CODE_UNSUPPORTED,
            "a missing store must be reported as unsupported so the file fallback engages"
        );
    }
}

#[test]
fn malformed_store_data_maps_to_secret_error() {
    let ref_ = reference("dblocal:malformed");
    for upstream in [
        keyring::Error::BadEncoding(vec![0xff]),
        keyring::Error::BadStoreFormat("junk".to_owned()),
        keyring::Error::TooLong("user".to_owned(), 1024),
        keyring::Error::Invalid("user".to_owned(), "empty".to_owned()),
        keyring::Error::NotSupportedByStore("nope".to_owned()),
    ] {
        let err = classify(upstream, &ref_);
        assert_eq!(err.code(), crate::error::CODE_SECRET);
    }

    let backend = backend_with(
        Err(|| keyring::Error::BadDataFormat(vec![0xff], "junk".into())),
        Ok(()),
        Ok(()),
    );
    assert_eq!(
        backend.get(&ref_).expect_err("malformed").code(),
        crate::error::CODE_SECRET
    );
}

#[test]
fn classification_never_leaks_upstream_text() {
    let ref_ = reference("dblocal:redaction");
    let err = classify(keyring::Error::PlatformFailure("bus down".into()), &ref_);
    let text = format!("{err:?} {err} {}", err.diagnostic());
    assert!(!text.contains("bus down"));
    assert_eq!(err.code(), crate::error::CODE_UNSUPPORTED);
}

#[test]
fn entry_for_validates_keys() {
    assert!(entry_for(&reference("../escape")).is_err());

    // Built when a store exists, Unsupported on a bare host. Neither path may
    // panic or carry secret material.
    match entry_for(&reference("dblocal:entry-for")) {
        Ok(_) => {}
        Err(err) => assert_eq!(err.code(), crate::error::CODE_UNSUPPORTED),
    }
}

/// Real-keychain smoke test. Ignored because it needs a live credential store
/// and may block on a desktop unlock prompt:
///
/// ```sh
/// cargo test -p dblocal-core --lib secrets::keyring -- --ignored
/// ```
#[test]
#[ignore = "e2e: needs a live OS keychain (Secret Service / Keychain / Credential Manager)"]
fn real_keychain_round_trip() {
    let backend = KeyringBackend::new();
    if !KeyringBackend::is_available() {
        eprintln!("skipping: no keychain store on this host");
        return;
    }

    let ref_ = reference(&format!("dblocal:smoke-{}", std::process::id()));
    let secret = SecretString::from("keychain-smoke-value");
    backend.set(&ref_, &secret).expect("set");
    assert_eq!(backend.get(&ref_).expect("get").expose(), secret.expose());
    backend.delete(&ref_).expect("delete");
    assert!(matches!(
        backend.get(&ref_),
        Err(crate::DblError::NotFound { .. })
    ));
}
