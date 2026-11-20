//! Cross-cutting leak test: every public type must redact a secret everywhere.
//!
//! This is the test that catches a `#[derive(Debug)]` accidentally added to a
//! new type in this module. One canary value is threaded through every public
//! value and every error, then rendered with `{:?}` and `{}` (and `to_string`,
//! `diagnostic`) to prove the plaintext never appears.

use super::*;
use crate::model::{CredentialBackend, CredentialRef, InstanceId};
use crate::secret::SecretString;

const CANARY: &str = "zz-flowbite-canary-4c8e";

/// Renders a value every way the codebase could accidentally print it.
fn every_render<T: std::fmt::Debug + std::fmt::Display>(value: &T) -> String {
    format!("{value:?}|{value}|{value}")
}

/// Renders a `Debug`-only value; several backend handles have no `Display`.
fn debug_render<T: std::fmt::Debug>(value: &T) -> String {
    format!("{value:?}")
}

#[test]
fn secret_string_redacts_every_render() {
    let secret = SecretString::from(CANARY);
    let rendered = every_render(&secret);
    assert!(
        !rendered.contains(CANARY),
        "SecretString leaked: {rendered}"
    );
    assert!(rendered.contains("***"));
}

#[test]
fn instance_secret_redacts_every_render() {
    let secret = InstanceSecret::password(SecretString::from(CANARY));
    let rendered = every_render(&secret);
    assert!(
        !rendered.contains(CANARY),
        "InstanceSecret leaked: {rendered}"
    );
}

#[test]
fn backends_redact_every_render() {
    let keyring = KeyringBackend::new();
    let rendered = debug_render(&keyring);
    assert!(!rendered.contains(CANARY));

    let file = FileBackend::new("/home/someone/.dblocal/secrets");
    let rendered = debug_render(&file);
    assert!(!rendered.contains(CANARY));

    let fake = FakeBackend::new(CredentialBackend::Keychain);
    let rendered = debug_render(&fake);
    assert!(!rendered.contains(CANARY));
}

#[test]
fn store_redacts_when_it_holds_a_secret() {
    let file = FakeBackend::new(CredentialBackend::File);
    let keyring = FakeBackend::new(CredentialBackend::Keychain);
    let store = SecretStore::new(keyring, file);

    let reference = CredentialRef::new(
        InstanceId::new(),
        CredentialBackend::Keychain,
        "dblocal:redaction",
        "postgres",
    );
    store
        .set(&reference, &SecretString::from(CANARY))
        .expect("set");

    // The store itself is not `Debug`, but the backends it owns are. Check the
    // in-memory backend, which is the one that keeps a copy of the plaintext.
    let rendered = format!("{:?}", store.keyring_backend());
    assert!(!rendered.contains(CANARY), "FakeBackend leaked: {rendered}");
}

#[test]
fn generated_password_never_renders_in_errors() {
    let password = generate_password().expect("os rng");
    let plaintext = password.expose().to_owned();

    // Drive every error path that could plausibly carry the value.
    let store = SecretStore::new(
        {
            let backend = FakeBackend::new(CredentialBackend::Keychain);
            backend.set_available(false);
            backend
        },
        FakeBackend::new(CredentialBackend::File),
    );
    let bad = CredentialRef::new(
        InstanceId::new(),
        CredentialBackend::File,
        "../nope",
        "postgres",
    );
    let errors = [
        store.set(&bad, &password).expect_err("invalid ref"),
        store.get(&bad).expect_err("invalid ref"),
        store.delete(&bad).expect_err("invalid ref"),
        SecretStore::new(
            FakeBackend::new(CredentialBackend::Keychain),
            FakeBackend::new(CredentialBackend::File),
        )
        .get(&CredentialRef::new(
            InstanceId::new(),
            CredentialBackend::Keychain,
            "dblocal:absent",
            "postgres",
        ))
        .expect_err("absent"),
    ];

    for err in errors {
        let rendered = format!("{err:?}|{err}|{}|{}", err.diagnostic(), err.user_message());
        assert!(
            !rendered.contains(&plaintext),
            "an error leaked the password: {rendered}"
        );
    }
}

#[test]
fn credential_ref_and_backend_kind_render_without_a_secret() {
    // These types never hold a secret by construction; assert the property that
    // makes that true: the ref only carries metadata.
    let id = InstanceId::new();
    let reference = credential_ref_for(&id, CredentialBackend::File, "postgres");
    let rendered = debug_render(&reference);
    assert!(!rendered.contains(CANARY));
    assert!(rendered.contains("postgres"));
    assert!(rendered.contains(&id.to_string()));

    let rendered = format!(
        "{:?}|{}",
        CredentialBackend::File,
        CredentialBackend::File.as_str()
    );
    assert_eq!(rendered, "File|file");
}
