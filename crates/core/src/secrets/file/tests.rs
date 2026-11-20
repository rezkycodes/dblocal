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

/// The on-disk name for a key, which is what every test below must use.
fn name(key: &str) -> String {
    file_name_for(key).expect("valid key")
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

// --- E2: Windows-safe, injective file names ---------------------------------

#[test]
fn file_name_never_contains_a_colon() {
    for key in [
        "dblocal:abc",
        "dblocal:0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
        "a:b:c",
        "with_underscore",
        "dotted.name",
    ] {
        let name = name(key);
        assert!(!name.contains(':'), "name for {key:?} contains ':': {name}");
        // A colon on NTFS would name an alternate data stream of `dblocal`.
        assert_ne!(name, "dblocal", "name must never collapse to the bare stem");
    }
}

#[test]
fn file_name_is_prefixed_so_it_cannot_be_a_device_name() {
    // Reserved Windows device names (CON, NUL, AUX, COM1, ...) are exact file
    // names; a `cred-` prefix makes them unreachable by construction.
    for key in ["CON", "NUL", "AUX", "COM1", "LPT1", "PRN"] {
        let name = name(key);
        assert!(
            name.starts_with("cred-"),
            "name for {key:?} must be prefixed: {name}"
        );
        assert_ne!(name.to_ascii_uppercase(), key);
    }
}

#[test]
fn distinct_keys_give_distinct_names() {
    // `:` and `_` are both legal key characters, so this is the pair most
    // likely to collide if the encoding were lossy. `%` is not a legal key
    // character, so `%3A` can only ever come from `:`.
    let keys = [
        "dblocal:aaaa-1111",
        "dblocal_aaaa-1111",
        "dblocal:aaaa_1111",
        "dblocal:aaaa.1111",
        "dblocal:bbbb-2222",
        "dblocal:bbbb_2222",
        "dblocal:cccc-3333",
        "dblocal-cccc-3333",
        "dblocal:dddd:eeee",
        "dblocal:dddd_eeee",
        "dblocal:ffff",
        "dblocal:FFFY",
        "cred-x",
        "cred_x",
        "cred:x",
        "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
        "0f1e2d3c_4b5a_6978_8796_a5b4c3d2e1f0",
        "a",
        "ab",
        "abc",
    ];

    let mut names: Vec<String> = keys.iter().map(|key| name(key)).collect();
    let total = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), total, "the encoding collided: {names:?}");
}

#[test]
fn encoded_name_is_stable_and_predictable() {
    // A literal golden value pins the format so a future refactor cannot drift
    // the on-disk layout without an explicit decision.
    assert_eq!(name("dblocal:abc"), "cred-dblocal%3Aabc");
    assert_eq!(name("plain-key_1.2"), "cred-plain-key_1.2");
    assert_eq!(name("a:b"), "cred-a%3Ab");
}

#[test]
fn file_name_validates_the_key() {
    for key in ["../escape", "a/b", "", "..", "a b"] {
        assert!(
            file_name_for(key).is_err(),
            "key {key:?} must be rejected before naming a file"
        );
    }
}

#[test]
fn round_trip_uses_the_encoded_name_on_disk() {
    let dir = tempdir().expect("tempdir");
    let secrets = dir.path().join("secrets");
    let backend = FileBackend::new(&secrets);
    let ref_ = reference("dblocal:abc");
    backend.set(&ref_, &secret()).expect("set");

    let expected = secrets.join("cred-dblocal%3Aabc");
    assert!(
        expected.is_file(),
        "expected file at {}",
        expected.display()
    );
    assert_eq!(
        fs::read_to_string(&expected).expect("read"),
        CANARY,
        "the encoded file must hold the plaintext"
    );
    assert_eq!(backend.get(&ref_).expect("get").expose(), CANARY);
}

// --- E3: an existing directory is tightened to 0700 -------------------------

#[cfg(unix)]
#[test]
fn tightens_a_preexisting_world_readable_directory() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().expect("tempdir");
    let secrets = dir.path().join("secrets");
    fs::create_dir(&secrets).expect("secrets");
    fs::set_permissions(&secrets, fs::Permissions::from_mode(0o755)).expect("loosen");
    assert_eq!(
        fs::metadata(&secrets).expect("meta").permissions().mode() & 0o777,
        0o755
    );

    let backend = FileBackend::new(&secrets);
    backend
        .set(&reference("dblocal:tighten"), &secret())
        .expect("set");

    assert_eq!(
        fs::metadata(&secrets).expect("meta").permissions().mode() & 0o777,
        0o700,
        "an existing secrets directory must be tightened to 0700 on write"
    );
}

#[cfg(unix)]
#[test]
fn still_refuses_a_symlinked_directory_when_tightening() {
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
}

// --- File modes and symlink refusal ----------------------------------------

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

    let file = secrets.join(name("dblocal:modes"));
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
fn refuses_a_symlinked_file() {
    use std::os::unix::fs::symlink;

    let dir = tempdir().expect("tempdir");
    let secrets = dir.path().join("secrets");
    fs::create_dir(&secrets).expect("secrets dir");
    let target = dir.path().join("outside.txt");
    fs::write(&target, b"do not touch").expect("write target");
    let link = secrets.join(name("dblocal:link"));
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
    fs::create_dir(secrets.join(name("dblocal:dir"))).expect("nested dir");

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
    fs::write(secrets.join(name("dblocal:bin")), b"\xff\xfe\x00\x01").expect("write");

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
    for payload in [first.clone(), second.clone()] {
        let backend = Arc::clone(&backend);
        let ref_ = ref_.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..60 {
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
fn deleting_a_directory_returns_a_secret_error() {
    let dir = tempdir().expect("tempdir");
    let secrets = dir.path().join("secrets");
    fs::create_dir(&secrets).expect("secrets");
    fs::create_dir(secrets.join(name("dblocal:dir"))).expect("nested");

    let backend = FileBackend::new(&secrets);
    let err = backend
        .delete(&reference("dblocal:dir"))
        .expect_err("directory");
    assert!(
        matches!(err, DblError::Secret { .. }),
        "a directory is not a credential file: {err}"
    );
}
