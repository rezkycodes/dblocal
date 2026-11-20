use std::fs;
use std::sync::Arc;
use std::thread;

use tempfile::tempdir;

use super::*;
use crate::model::InstanceId;

const CANARY: &str = "file-backend-canary-9f2c";

fn reference(key: &str) -> CredentialRef {
    CredentialRef::new(InstanceId::new(), CredentialBackend::File, key, "postgres")
}

fn secret() -> SecretString {
    SecretString::from(CANARY)
}

#[test]
fn round_trips_a_secret() {
    let dir = tempdir().expect("tempdir");
    let backend = FileBackend::new(dir.path().join("secrets"));
    let ref_ = reference("dblocal:one");

    backend.set(&ref_, &secret()).expect("set");
    assert_eq!(backend.get(&ref_).expect("get").expose(), CANARY);

    backend.delete(&ref_).expect("delete");
    assert!(matches!(backend.get(&ref_), Err(DblError::NotFound { .. })));
    assert!(matches!(
        backend.delete(&ref_),
        Err(DblError::NotFound { .. })
    ));
}

#[test]
fn kind_is_file() {
    let backend = FileBackend::new("/nonexistent");
    assert_eq!(backend.kind(), CredentialBackend::File);
}

#[cfg(unix)]
#[test]
fn directory_is_0700_and_file_is_0600() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().expect("tempdir");
    let secrets = dir.path().join("secrets");
    let backend = FileBackend::new(&secrets);
    let ref_ = reference("dblocal:modes");
    backend.set(&ref_, &secret()).expect("set");

    let dir_mode = fs::metadata(&secrets)
        .expect("dir metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "secrets directory must be 0700");

    let file = secrets.join("dblocal:modes");
    let file_mode = fs::metadata(&file)
        .expect("file metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600, "credential file must be 0600");

    // The directory is never world-readable even after several writes.
    backend
        .set(&ref_, &SecretString::from("replacement"))
        .expect("set");
    assert_eq!(
        fs::metadata(&secrets).expect("meta").permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&file).expect("meta").permissions().mode() & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[test]
fn refuses_a_symlinked_directory() {
    use std::os::unix::fs::symlink;

    let dir = tempdir().expect("tempdir");
    let real = dir.path().join("real");
    fs::create_dir(&real).expect("real dir");
    let link = dir.path().join("secrets");
    symlink(&real, &link).expect("symlink");

    let backend = FileBackend::new(&link);
    let err = backend
        .set(&reference("dblocal:symlink-dir"), &secret())
        .expect_err("must refuse symlinked directory");
    assert!(matches!(err, DblError::Secret { .. }));
    assert!(fs::read_dir(&real).expect("read").next().is_none());
}

#[cfg(unix)]
#[test]
fn refuses_a_symlinked_file() {
    use std::os::unix::fs::symlink;

    let dir = tempdir().expect("tempdir");
    let secrets = dir.path().join("secrets");
    fs::create_dir(&secrets).expect("secrets dir");
    let target = dir.path().join("outside.txt");
    fs::write(&target, b"do not touch").expect("write target");
    let link = secrets.join("dblocal:link");
    symlink(&target, &link).expect("symlink");

    let backend = FileBackend::new(&secrets);
    let ref_ = reference("dblocal:link");
    assert!(matches!(
        backend.set(&ref_, &secret()),
        Err(DblError::Secret { .. })
    ));
    assert!(matches!(backend.get(&ref_), Err(DblError::Secret { .. })));
    assert!(matches!(
        backend.delete(&ref_),
        Err(DblError::Secret { .. })
    ));
    // The file the link pointed at is untouched.
    assert_eq!(fs::read(&target).expect("read target"), b"do not touch");
}

#[cfg(unix)]
#[test]
fn rejects_a_directory_at_the_credential_path() {
    let dir = tempdir().expect("tempdir");
    let secrets = dir.path().join("secrets");
    fs::create_dir(&secrets).expect("secrets");
    fs::create_dir(secrets.join("dblocal:dir")).expect("nested dir");

    let backend = FileBackend::new(&secrets);
    let err = backend
        .set(&reference("dblocal:dir"), &secret())
        .expect_err("must refuse a directory");
    assert!(matches!(err, DblError::Secret { .. }));
}

#[test]
fn rejects_unsafe_reference_keys() {
    let dir = tempdir().expect("tempdir");
    let backend = FileBackend::new(dir.path().join("secrets"));
    for key in ["../escape", "a/b", "a\\b", "", ".."] {
        assert!(
            backend.set(&reference(key), &secret()).is_err(),
            "key {key:?} must be rejected"
        );
    }
}

#[test]
fn reads_a_non_utf8_file_as_a_secret_error() {
    let dir = tempdir().expect("tempdir");
    let secrets = dir.path().join("secrets");
    fs::create_dir(&secrets).expect("secrets");
    fs::write(secrets.join("dblocal:bin"), b"\xff\xfe\x00\x01").expect("write");

    let backend = FileBackend::new(&secrets);
    let err = backend
        .get(&reference("dblocal:bin"))
        .expect_err("binary file");
    assert!(matches!(err, DblError::Secret { .. }));
    assert!(!err.to_string().contains(CANARY));
}

#[test]
fn concurrent_writers_never_leave_a_torn_file() {
    let dir = tempdir().expect("tempdir");
    let secrets = dir.path().join("secrets");
    let backend = Arc::new(FileBackend::new(&secrets));
    let ref_ = reference("dblocal:contended");

    // Two distinct, equally sized payloads: any interleaving would produce a
    // string that is neither, or a length that matches neither.
    let first = "a".repeat(4096);
    let second = "b".repeat(4096);

    let mut handles = Vec::new();
    for (index, payload) in [first.clone(), second.clone()].into_iter().enumerate() {
        let backend = Arc::clone(&backend);
        let ref_ = ref_.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..60 {
                let _ = index;
                backend
                    .set(&ref_, &SecretString::from(payload.clone()))
                    .expect("concurrent set");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("writer thread");
    }

    let stored = backend.get(&ref_).expect("get").expose().to_owned();
    assert_eq!(stored.len(), 4096, "file must not be torn");
    assert!(
        stored == first || stored == second,
        "file contains an interleaving of two writers"
    );

    // No temporary files are left behind.
    for entry in fs::read_dir(&secrets).expect("read dir") {
        let name = entry.expect("entry").file_name();
        let name = name.to_string_lossy().to_string();
        assert!(!name.starts_with(".tmp-"), "leaked temporary file: {name}");
    }
}

#[test]
fn overwrite_replaces_the_previous_value() {
    let dir = tempdir().expect("tempdir");
    let backend = FileBackend::new(dir.path().join("secrets"));
    let ref_ = reference("dblocal:replace");
    backend
        .set(&ref_, &SecretString::from("first"))
        .expect("set");
    backend
        .set(&ref_, &SecretString::from("second"))
        .expect("set");
    assert_eq!(backend.get(&ref_).expect("get").expose(), "second");
}

#[test]
fn missing_parent_is_created_recursively() {
    let dir = tempdir().expect("tempdir");
    let nested = dir.path().join("a").join("b").join("secrets");
    let backend = FileBackend::new(&nested);
    backend
        .set(&reference("dblocal:deep"), &secret())
        .expect("set");
    assert!(nested.is_dir());
}

#[test]
fn from_paths_uses_the_secrets_directory() {
    let dir = tempdir().expect("tempdir");
    let paths = crate::paths::DblPaths::with_root(dir.path());
    let backend = FileBackend::from_paths(&paths);
    assert_eq!(backend.dir(), paths.secrets_dir());
}

#[test]
fn error_paths_do_not_leak_the_secret() {
    let dir = tempdir().expect("tempdir");
    let backend = FileBackend::new(dir.path().join("secrets"));
    let err = backend
        .get(&reference("dblocal:absent"))
        .expect_err("missing");
    let text = format!("{err:?} {err} {}", err.diagnostic());
    assert!(!text.contains(CANARY));
    assert_eq!(err.code(), crate::error::CODE_NOT_FOUND);
}

#[test]
fn deleting_a_directory_returns_an_io_error() {
    let dir = tempdir().expect("tempdir");
    let secrets = dir.path().join("secrets");
    fs::create_dir(&secrets).expect("secrets");
    fs::create_dir(secrets.join("dblocal:dir")).expect("nested");

    let backend = FileBackend::new(&secrets);
    let err = backend
        .delete(&reference("dblocal:dir"))
        .expect_err("directory");
    assert!(
        matches!(err, DblError::Secret { .. }),
        "a directory is not a credential file: {err}"
    );
}
