//! Tests for the backend-routed credential store.
//!
//! Both backends are fakes, so routing decisions are driven by the
//! `CredentialRef::backend()` record and by injecting `Unsupported` into a
//! fake, never by the host's D-Bus state.

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

/// A reference whose recorded backend is `backend`.
fn reference_for(backend: CredentialBackend, key: &str) -> CredentialRef {
    CredentialRef::new(InstanceId::new(), backend, key, "postgres")
}

/// The same key pointed at a different recorded backend, as a W2-A instance
/// row would look after a fallback write.
fn with_backend(reference: &CredentialRef, backend: CredentialBackend) -> CredentialRef {
    CredentialRef::new(
        reference.instance_id().clone(),
        backend,
        reference.key_ref(),
        reference.username(),
    )
}

fn fake_store(keyring_available: bool) -> SecretStore<FakeBackend, FakeBackend> {
    let keyring = FakeBackend::new(CredentialBackend::Keychain);
    keyring.set_available(keyring_available);
    let file = FakeBackend::new(CredentialBackend::File);
    SecretStore::new(keyring, file)
}

#[test]
fn set_prefers_the_keychain_when_available() {
    let store = fake_store(true);
    let ref_ = reference_for(CredentialBackend::Keychain, "dblocal:kc");
    let backend = store.set(&ref_, &SecretString::from(CANARY)).expect("set");
    assert_eq!(backend, CredentialBackend::Keychain);
    assert_eq!(store.keyring_backend().len(), 1);
    assert!(store.file_backend().is_empty());
}

#[test]
fn set_falls_back_to_file_and_reports_it() {
    let store = fake_store(false);
    let ref_ = reference_for(CredentialBackend::File, "dblocal:fallback");
    let backend = store
        .set(&ref_, &SecretString::from(CANARY))
        .expect("set falls back");
    assert_eq!(
        backend,
        CredentialBackend::File,
        "set must report the backend that actually stored the secret"
    );
    assert!(store.keyring_backend().is_empty());
    assert_eq!(store.file_backend().len(), 1);

    // Readable through a reference recorded as File, which is what W2-A
    // persists after a fallback write.
    let recorded = with_backend(&ref_, CredentialBackend::File);
    assert_eq!(store.get(&recorded).expect("get").expose(), CANARY);
    assert_eq!(store.keyring_backend().get_calls(), 0);
}

#[test]
fn get_routes_by_recorded_backend() {
    // The same key exists in both backends with different values; `get` must
    // return the one named by the reference, not the keychain by default.
    let store = fake_store(true);
    let base = reference_for(CredentialBackend::Keychain, "dblocal:recorded");
    let keychain_ref = with_backend(&base, CredentialBackend::Keychain);
    let file_ref = with_backend(&base, CredentialBackend::File);
    store
        .keyring_backend()
        .set(&keychain_ref, &SecretString::from("from-keychain"))
        .expect("seed keychain");
    store
        .file_backend()
        .set(&file_ref, &SecretString::from("from-file"))
        .expect("seed file");

    assert_eq!(
        store.get(&keychain_ref).expect("keychain").expose(),
        "from-keychain"
    );
    assert_eq!(store.get(&file_ref).expect("file").expose(), "from-file");

    // Exactly one read per backend: routing never falls back across backends.
    assert_eq!(store.keyring_backend().get_calls(), 1);
    assert_eq!(store.file_backend().get_calls(), 1);
}

#[test]
fn file_secret_readable_when_keychain_available_but_empty() {
    // Regression (E1): a secret stored in the file fallback while the keychain
    // was unavailable must stay readable once the keychain comes back but has
    // no entry. Routing by the record is what makes this work.
    let store = fake_store(true);
    let ref_ = reference_for(CredentialBackend::File, "dblocal:headless-then-desktop");
    store
        .file_backend()
        .set(&ref_, &SecretString::from("from-file"))
        .expect("seed file");

    assert_eq!(store.get(&ref_).expect("get").expose(), "from-file");
    assert_eq!(
        store.keyring_backend().get_calls(),
        0,
        "an available-but-empty keychain must not be consulted for a File credential"
    );
}

#[test]
fn unavailable_recorded_backend_returns_its_error_without_touching_the_other() {
    let store = fake_store(false);
    let ref_ = reference_for(CredentialBackend::Keychain, "dblocal:locked");
    store
        .file_backend()
        .set(&ref_, &SecretString::from("stale-file-copy"))
        .expect("seed file");

    let err = store.get(&ref_).expect_err("keychain unavailable");
    assert_eq!(
        err.code(),
        CODE_UNSUPPORTED,
        "the keychain's unavailable error must reach the caller unchanged"
    );
    assert_eq!(
        store.file_backend().get_calls(),
        0,
        "a read must never cross to the other backend"
    );
}

#[test]
fn present_keychain_secret_is_read() {
    let store = fake_store(true);
    let ref_ = reference_for(CredentialBackend::Keychain, "dblocal:present");
    store
        .keyring_backend()
        .set(&ref_, &SecretString::from(CANARY))
        .expect("seed");
    assert_eq!(store.get(&ref_).expect("get").expose(), CANARY);
    assert_eq!(store.file_backend().get_calls(), 0);
}

#[test]
fn missing_recorded_secret_is_not_found_from_that_backend() {
    // Separate stores so each counter reflects exactly one read.
    let file_store = fake_store(true);
    let file_ref = reference_for(CredentialBackend::File, "dblocal:absent");
    let err = file_store.get(&file_ref).expect_err("missing");
    assert_eq!(err.code(), CODE_NOT_FOUND);
    assert_eq!(file_store.keyring_backend().get_calls(), 0);

    let keychain_store = fake_store(true);
    let keyring_ref = reference_for(CredentialBackend::Keychain, "dblocal:absent-kc");
    let err = keychain_store.get(&keyring_ref).expect_err("missing");
    assert_eq!(err.code(), CODE_NOT_FOUND);
    assert_eq!(keychain_store.file_backend().get_calls(), 0);
}

#[test]
fn delete_removes_from_every_backend() {
    // `delete` must not depend on the verdict of whichever backend answered
    // first, so a stale copy in the other backend can never survive.
    let store = fake_store(true);
    let ref_ = reference_for(CredentialBackend::Keychain, "dblocal:del");
    store
        .keyring_backend()
        .set(&ref_, &SecretString::from("kc"))
        .expect("seed keychain");
    store
        .file_backend()
        .set(&ref_, &SecretString::from("file"))
        .expect("seed file");

    store.delete(&ref_).expect("delete");
    assert!(store.keyring_backend().is_empty());
    assert!(store.file_backend().is_empty());
}

#[test]
fn delete_tolerates_an_unavailable_backend() {
    let store = fake_store(false);
    let ref_ = reference_for(CredentialBackend::File, "dblocal:del-fallback");
    store
        .file_backend()
        .set(&ref_, &SecretString::from(CANARY))
        .expect("seed file");
    store.delete(&ref_).expect("delete via fallback");
    assert!(store.file_backend().is_empty());
}

#[test]
fn delete_is_idempotent_and_tolerates_missing_entries() {
    let store = fake_store(true);
    let ref_ = reference_for(CredentialBackend::Keychain, "dblocal:everywhere");

    // Nothing stored anywhere yet: F-CRD-01 AC3 teardown must not fail.
    store.delete(&ref_).expect("delete when absent");
    store.delete(&ref_).expect("delete twice when absent");
}

#[test]
fn delete_surfaces_an_invalid_key_rather_than_swallowing_it() {
    let keyring = FakeBackend::new(CredentialBackend::Keychain);
    keyring.set_available(true);
    // An unsafe key fails validation inside every backend and is neither
    // NotFound nor Unsupported, so it must be reported.
    let store = SecretStore::new(keyring, FakeBackend::new(CredentialBackend::File));
    let bad = reference_for(CredentialBackend::Keychain, "../escape");
    let err = store.delete(&bad).expect_err("invalid key");
    assert_eq!(err.code(), CODE_INVALID_INPUT);
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
        .set(
            &reference_for(CredentialBackend::Keychain, "../escape"),
            &SecretString::from(CANARY),
        )
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

    let ref_ = reference_for(CredentialBackend::File, "dblocal:real-file");
    let backend = store.set(&ref_, &SecretString::from(CANARY)).expect("set");
    assert_eq!(backend, CredentialBackend::File);
    assert_eq!(store.get(&ref_).expect("get").expose(), CANARY);
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
    let ref_ = reference_for(CredentialBackend::File, "dblocal:race");

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
    let err = store
        .set(
            &reference_for(CredentialBackend::Keychain, "../bad-key"),
            &SecretString::from(CANARY),
        )
        .expect_err("invalid ref");
    let text = format!("{err:?} {err} {}", err.diagnostic());
    assert!(!text.contains(CANARY));

    let missing = store
        .get(&reference_for(CredentialBackend::File, "dblocal:absent"))
        .expect_err("missing");
    let text = format!("{missing:?} {missing} {}", missing.diagnostic());
    assert!(!text.contains(CANARY));
    assert_eq!(missing.code(), CODE_NOT_FOUND);

    let unavailable = fake_store(false)
        .get(&reference_for(CredentialBackend::Keychain, "dblocal:down"))
        .expect_err("keychain down");
    assert_eq!(unavailable.code(), CODE_UNSUPPORTED);
    assert_ne!(unavailable.code(), CODE_SECRET);
    assert!(!unavailable.user_message().contains(CANARY));
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
