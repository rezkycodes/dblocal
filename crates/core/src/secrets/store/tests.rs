//! Tests for the keychain-first store and its file fallback.
//!
//! Both backends are fakes, so the fallback decision is driven by returning
//! `Unsupported` from the keychain rather than by the host's D-Bus state.

use std::sync::Arc;
use std::thread;

use tempfile::tempdir;

use super::*;
use crate::error::{CODE_INVALID_INPUT, CODE_NOT_FOUND, CODE_SECRET, CODE_UNSUPPORTED};
use crate::model::{CredentialBackend, CredentialRef, InstanceId};
use crate::secret::SecretString;

use crate::secrets::backend::FakeBackend;
use crate::secrets::file::FileBackend;
use crate::secrets::keyring::KeyringBackend;

const CANARY: &str = "store-canary-1f4d";

fn reference(key: &str) -> CredentialRef {
    CredentialRef::new(
        InstanceId::new(),
        CredentialBackend::Keychain,
        key,
        "postgres",
    )
}

fn fake_store(keyring_available: bool) -> SecretStore<FakeBackend, FakeBackend> {
    let keyring = FakeBackend::new(CredentialBackend::Keychain);
    keyring.set_available(keyring_available);
    let file = FakeBackend::new(CredentialBackend::File);
    SecretStore::new(keyring, file)
}

#[test]
fn uses_keychain_when_available() {
    let store = fake_store(true);
    let ref_ = reference("dblocal:kc");
    let backend = store.set(&ref_, &SecretString::from(CANARY)).expect("set");
    assert_eq!(backend, CredentialBackend::Keychain);
    assert_eq!(store.get(&ref_).expect("get").expose(), CANARY);
    // The file fallback was never touched.
    assert!(store.file_backend().is_empty());
    assert_eq!(store.keyring_backend().len(), 1);
}

#[test]
fn falls_back_to_file_when_keychain_is_unavailable() {
    let store = fake_store(false);
    let ref_ = reference("dblocal:fallback");
    let backend = store
        .set(&ref_, &SecretString::from(CANARY))
        .expect("set falls back");
    assert_eq!(
        backend,
        CredentialBackend::File,
        "kind() must report the backend that actually stored the secret"
    );
    assert!(store.keyring_backend().is_empty());
    assert_eq!(store.file_backend().len(), 1);
    assert_eq!(store.get(&ref_).expect("get via fallback").expose(), CANARY);
}

#[test]
fn get_prefers_the_keychain_value() {
    let store = fake_store(true);
    let ref_ = reference("dblocal:prefer");
    store
        .keyring_backend()
        .set(&ref_, &SecretString::from("from-keychain"))
        .expect("seed keychain");
    store
        .file_backend()
        .set(&ref_, &SecretString::from("from-file"))
        .expect("seed file");
    assert_eq!(store.get(&ref_).expect("get").expose(), "from-keychain");
}

#[test]
fn missing_keychain_entry_is_not_a_reason_to_read_the_file() {
    // A credential lives in exactly one backend. If the keychain is healthy and
    // has no entry, reading the file would silently resurrect a stale value.
    let store = fake_store(true);
    let ref_ = reference("dblocal:stale-file");
    store
        .file_backend()
        .set(&ref_, &SecretString::from("stale"))
        .expect("seed file");
    let err = store.get(&ref_).expect_err("keychain has no entry");
    assert_eq!(err.code(), CODE_NOT_FOUND);
    // The file held the plaintext "stale"; a NotFound from the healthy
    // keychain must not surface or read it.
    assert!(!err.user_message().contains("stale"));
}

#[test]
fn delete_uses_the_keychain_when_available() {
    let store = fake_store(true);
    let ref_ = reference("dblocal:del");
    store
        .keyring_backend()
        .set(&ref_, &SecretString::from(CANARY))
        .expect("seed");
    store.delete(&ref_).expect("delete");
    assert!(store.keyring_backend().is_empty());
}

#[test]
fn delete_falls_back_without_hiding_a_real_error() {
    let store = fake_store(false);
    let ref_ = reference("dblocal:del-file");
    store
        .file_backend()
        .set(&ref_, &SecretString::from(CANARY))
        .expect("seed");
    store.delete(&ref_).expect("delete via fallback");
    assert!(store.file_backend().is_empty());

    // Nothing stored anywhere: the file backend answers NotFound.
    let err = store.delete(&ref_).expect_err("nothing to delete");
    assert_eq!(err.code(), CODE_NOT_FOUND);
}

#[test]
fn delete_everywhere_removes_both_copies_and_tolerates_missing() {
    let store = fake_store(true);
    let ref_ = reference("dblocal:everywhere");
    store
        .keyring_backend()
        .set(&ref_, &SecretString::from("kc"))
        .expect("seed keychain");
    store
        .file_backend()
        .set(&ref_, &SecretString::from("file"))
        .expect("seed file");

    store.delete_everywhere(&ref_).expect("delete everywhere");
    assert!(store.keyring_backend().is_empty());
    assert!(store.file_backend().is_empty());

    // Calling it again is a no-op, not an error (F-CRD-01 AC3 on retry).
    store.delete_everywhere(&ref_).expect("second delete");
}

#[test]
fn delete_everywhere_surfaces_a_real_keychain_failure() {
    let keyring = FakeBackend::new(CredentialBackend::Keychain);
    keyring.set_available(true);
    // Poison-free injection: an unsafe key fails validation inside the backend
    // and is neither NotFound nor Unsupported, so it must be reported.
    let store = SecretStore::new(keyring, FakeBackend::new(CredentialBackend::File));
    let bad = reference("../escape");
    let err = store.delete_everywhere(&bad).expect_err("invalid key");
    assert_eq!(err.code(), CODE_INVALID_INPUT);
}

#[test]
fn get_from_honours_the_recorded_backend() {
    let store = fake_store(true);
    let ref_ = reference("dblocal:recorded");
    store
        .file_backend()
        .set(&ref_, &SecretString::from("in-file"))
        .expect("seed file");

    let from_file = store
        .get_from(CredentialBackend::File, &ref_)
        .expect("file read");
    assert_eq!(from_file.expose(), "in-file");

    let err = store
        .get_from(CredentialBackend::Keychain, &ref_)
        .expect_err("keychain has no entry");
    assert_eq!(err.code(), CODE_NOT_FOUND);
}

#[test]
fn get_from_falls_back_when_the_recorded_backend_disappears() {
    // A credential recorded as keychain-backed must still be readable on a
    // host where the keychain is gone (F-CRD edge case: keychain locked/absent).
    let store = fake_store(false);
    let ref_ = reference("dblocal:recovered");
    store
        .file_backend()
        .set(&ref_, &SecretString::from("recovered"))
        .expect("seed file");
    let secret = store
        .get_from(CredentialBackend::Keychain, &ref_)
        .expect("fallback from a vanished keychain");
    assert_eq!(secret.expose(), "recovered");
}

#[test]
fn credential_ref_for_uses_the_dblocal_prefix_and_backend() {
    let id = InstanceId::new();
    let ref_ = credential_ref_for(&id, CredentialBackend::File, "postgres");
    assert_eq!(ref_.key_ref(), format!("dblocal:{id}"));
    assert_eq!(ref_.backend(), CredentialBackend::File);
    assert_eq!(ref_.username(), "postgres");
    assert_eq!(ref_.keychain_key(&id), ref_.key_ref());
}

#[test]
fn unsupported_is_the_only_fallback_signal() {
    let store = fake_store(true);
    assert!(should_fallback(&backend_unavailable(
        CredentialBackend::Keychain
    )));
    assert!(!should_fallback(&DblError::Secret {
        reason: "locked".to_owned()
    }));
    assert!(!should_fallback(&DblError::NotFound {
        kind: "credential",
        name: "x".to_owned()
    }));
    assert!(!should_fallback(&DblError::Io {
        source: std::io::Error::other("boom")
    }));
    // A write that fails for a real reason must not silently move to the file.
    store.keyring_backend().set_available(true);
    let err = store
        .set(&reference("../escape"), &SecretString::from(CANARY))
        .expect_err("invalid key");
    assert_eq!(err.code(), CODE_INVALID_INPUT);
}

#[test]
fn real_file_backend_round_trips_through_the_store() {
    let dir = tempdir().expect("tempdir");
    let file = FileBackend::new(dir.path().join("secrets"));
    let keyring = FakeBackend::new(CredentialBackend::Keychain);
    keyring.set_available(false);
    let store = SecretStore::new(keyring, file);

    let ref_ = reference("dblocal:real-file");
    let backend = store.set(&ref_, &SecretString::from(CANARY)).expect("set");
    assert_eq!(backend, CredentialBackend::File);
    assert_eq!(store.get(&ref_).expect("get").expose(), CANARY);
    assert_eq!(
        store
            .get_from(CredentialBackend::File, &ref_)
            .expect("get_from")
            .expose(),
        CANARY
    );
    store.delete(&ref_).expect("delete");
    assert_eq!(store.get(&ref_).expect_err("gone").code(), CODE_NOT_FOUND);
}

#[test]
fn concurrent_store_writers_do_not_lose_the_last_value() {
    let dir = tempdir().expect("tempdir");
    let store = Arc::new(SecretStore::new(
        {
            let backend = FakeBackend::new(CredentialBackend::Keychain);
            backend.set_available(false);
            backend
        },
        FileBackend::new(dir.path().join("secrets")),
    ));
    let ref_ = reference("dblocal:race");

    let mut handles = Vec::new();
    for payload in ["x".repeat(1024), "y".repeat(1024)] {
        let store = Arc::clone(&store);
        let ref_ = ref_.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..40 {
                store
                    .set(&ref_, &SecretString::from(payload.clone()))
                    .expect("concurrent store set");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("writer");
    }

    let stored = store.get(&ref_).expect("get").expose().to_owned();
    assert_eq!(stored.len(), 1024);
    assert!(
        stored.chars().all(|c| c == 'x') || stored.chars().all(|c| c == 'y'),
        "a torn or interleaved value was stored"
    );
}

#[test]
fn error_surfaces_never_contain_the_secret() {
    let store = fake_store(true);
    let ref_ = reference("../bad-key");
    let err = store
        .set(&ref_, &SecretString::from(CANARY))
        .expect_err("invalid ref");
    let text = format!("{err:?} {err} {}", err.diagnostic());
    assert!(!text.contains(CANARY));

    let missing = store
        .get(&reference("dblocal:absent"))
        .expect_err("missing");
    let text = format!("{missing:?} {missing} {}", missing.diagnostic());
    assert!(!text.contains(CANARY));
    assert_eq!(missing.code(), CODE_NOT_FOUND);

    // The keychain's own classification path stays quiet too.
    let store = fake_store(false);
    let err = store
        .get(&reference("dblocal:absent-fallback"))
        .expect_err("missing in fallback");
    assert_eq!(err.code(), CODE_NOT_FOUND);
    assert_ne!(err.code(), CODE_SECRET);
    assert_ne!(err.code(), CODE_UNSUPPORTED);
}

#[test]
fn for_paths_uses_the_real_secrets_directory() {
    let dir = tempdir().expect("tempdir");
    let paths = crate::paths::DblPaths::with_root(dir.path());
    let store = SecretStore::for_paths(&paths);
    assert_eq!(store.file_backend().dir(), paths.secrets_dir());
    let preferred = store.preferred_backend();
    assert!(preferred == CredentialBackend::Keychain || preferred == CredentialBackend::File);
    assert_eq!(
        SecretStore::keychain_available(),
        preferred == CredentialBackend::Keychain
    );
}

#[test]
fn store_is_send_and_sync() {
    // The service layer holds the store behind a trait object requiring
    // `Send + Sync + 'static` (CONTRACT.md: `DblService`), so the concrete
    // store and both real backends must satisfy it.
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<SecretStore>();
    assert_send_sync::<KeyringBackend>();
    assert_send_sync::<FileBackend>();
}
