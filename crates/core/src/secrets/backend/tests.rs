use super::*;
use crate::error::CODE_INVALID_INPUT;
use crate::model::InstanceId;

fn reference(key: &str) -> CredentialRef {
    CredentialRef::new(InstanceId::new(), CredentialBackend::File, key, "postgres")
}

#[test]
fn key_ref_validation_accepts_realistic_keys() {
    for key in [
        "dblocal:0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
        "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0.key",
        "abc",
    ] {
        assert!(validate_key_ref(&reference(key)).is_ok(), "{key}");
    }
}

#[test]
fn key_ref_validation_rejects_traversal_and_junk() {
    for key in [
        "",
        ".",
        "..",
        "../../etc/passwd",
        "a/b",
        "a\\b",
        "a b",
        "a\0b",
        "café",
    ] {
        let err = validate_key_ref(&reference(key)).expect_err(key);
        assert_eq!(err.code(), CODE_INVALID_INPUT, "{key}");
    }
    let long = "a".repeat(MAX_KEY_REF_LEN + 1);
    assert!(validate_key_ref(&reference(&long)).is_err());
    assert!(validate_key_ref(&reference(&"a".repeat(MAX_KEY_REF_LEN))).is_ok());
}

#[test]
fn backend_kind_matches_the_credential_backend_alias() {
    // `BackendKind` must be `CredentialBackend` itself, not a parallel enum.
    let kind: BackendKind = CredentialBackend::Keychain;
    assert_eq!(kind.as_str(), "keychain");
}

#[test]
fn fake_backend_round_trips_and_reports_unavailability() {
    let backend = FakeBackend::new(BackendKind::Keychain);
    assert_eq!(backend.kind(), BackendKind::Keychain);
    assert!(backend.is_empty());

    let ref_ = reference("dblocal:abc");
    let secret = SecretString::from("s3cret");
    backend.set(&ref_, &secret).expect("set");
    assert_eq!(backend.len(), 1);
    assert_eq!(backend.get(&ref_).expect("get").expose(), "s3cret");

    backend.delete(&ref_).expect("delete");
    assert!(backend.is_empty());
    assert!(matches!(
        backend.delete(&ref_),
        Err(DblError::NotFound { .. })
    ));

    backend.set_available(false);
    for err in [
        backend.get(&ref_).expect_err("get unavailable"),
        backend.set(&ref_, &secret).expect_err("set unavailable"),
        backend.delete(&ref_).expect_err("delete unavailable"),
    ] {
        assert!(matches!(err, DblError::Unsupported { .. }));
    }
}

#[test]
fn fake_backend_rejects_unsafe_keys_before_storing() {
    let backend = FakeBackend::new(BackendKind::File);
    let bad = reference("../escape");
    assert!(backend.set(&bad, &SecretString::from("x")).is_err());
    assert!(backend.get(&bad).is_err());
    assert!(backend.delete(&bad).is_err());
    assert!(backend.is_empty());
}

#[test]
fn fake_backend_defaults_to_file_kind() {
    let backend = FakeBackend::default();
    assert_eq!(backend.kind(), BackendKind::File);
    assert!(backend.is_empty());
}

#[test]
fn error_helpers_shape_the_taxonomy() {
    let ref_ = reference("dblocal:shaped");
    let missing = not_found(&ref_);
    assert_eq!(missing.code(), crate::error::CODE_NOT_FOUND);
    let unsupported = unavailable(BackendKind::Keychain);
    assert_eq!(unsupported.code(), crate::error::CODE_UNSUPPORTED);
    assert!(!unsupported.to_string().contains("dblocal:shaped"));
}
