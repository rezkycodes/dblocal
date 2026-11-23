//! Tests for config backups and the last-good marker.

use super::*;
use std::os::unix::fs::PermissionsExt;

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("meta")
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn take_copies_the_file_privately_and_returns_the_backup() {
    let dir = tempfile::tempdir().expect("tmp");
    let home = dir.path();
    let source = home.join("data").join("postgresql.conf");
    fs::create_dir_all(source.parent().expect("parent")).expect("data");
    fs::write(&source, b"max_connections = 100\n").expect("write");

    let store = BackupStore::for_instance(home, "app");
    let info = store.take(&source, 1_700_000_000).expect("take");
    assert_eq!(info.taken_at, 1_700_000_000);
    assert_eq!(
        fs::read(&info.path).expect("read"),
        b"max_connections = 100\n"
    );
    assert_eq!(mode(&info.path), 0o600, "backup must be 0600");
    assert_eq!(mode(store.dir()), 0o700, "backup dir must be 0700");
}

#[test]
fn valkey_requirepass_is_copied_verbatim_and_never_parsed() {
    let dir = tempfile::tempdir().expect("tmp");
    let home = dir.path();
    let source = home.join("run").join("valkey.conf");
    fs::create_dir_all(source.parent().expect("parent")).expect("run");
    let body = b"port 6379\nrequirepass hunter2-SECRET\n";
    fs::write(&source, body).expect("write");
    let store = BackupStore::for_instance(home, "vk");
    let info = store.take(&source, 1).expect("take");
    assert_eq!(fs::read(&info.path).expect("read"), body);
    assert_eq!(mode(&info.path), 0o600);
}

#[test]
fn prune_keeps_the_newest_five() {
    let dir = tempfile::tempdir().expect("tmp");
    let home = dir.path();
    let source = home.join("data").join("my.cnf");
    fs::create_dir_all(source.parent().expect("parent")).expect("data");
    fs::write(&source, b"[mysqld]\nport=3307\n").expect("write");
    let store = BackupStore::for_instance(home, "m");
    for ts in 1..=8 {
        store.take(&source, ts).expect("take");
    }
    let mut names: Vec<String> = fs::read_dir(store.dir())
        .expect("list")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".bak"))
        .collect();
    names.sort();
    assert_eq!(names.len(), KEEP, "{names:?}");
    // The oldest three were removed; the newest five (4..=8) remain.
    assert!(!names.iter().any(|n| n.starts_with("my.cnf.1.")));
    assert!(!names.iter().any(|n| n.starts_with("my.cnf.2.")));
    assert!(!names.iter().any(|n| n.starts_with("my.cnf.3.")));
    assert!(names.iter().any(|n| n.starts_with("my.cnf.8.")));
}

#[test]
fn newest_returns_the_largest_timestamp() {
    let dir = tempfile::tempdir().expect("tmp");
    let home = dir.path();
    let source = home.join("data").join("my.cnf");
    fs::create_dir_all(source.parent().expect("parent")).expect("data");
    fs::write(&source, b"x").expect("write");
    let store = BackupStore::for_instance(home, "m");
    store.take(&source, 10).expect("a");
    store.take(&source, 30).expect("b");
    store.take(&source, 20).expect("c");
    let newest = store.newest("my.cnf").expect("newest");
    assert_eq!(newest.taken_at, 30);
}

#[test]
fn newest_is_none_without_backups() {
    let dir = tempfile::tempdir().expect("tmp");
    let store = BackupStore::for_instance(dir.path(), "none");
    assert!(store.newest("my.cnf").is_none());
}

#[test]
fn last_good_marker_round_trips() {
    let dir = tempfile::tempdir().expect("tmp");
    let home = dir.path();
    let store = BackupStore::for_instance(home, "m");
    assert!(store.last_good("my.cnf").is_none());
    store.record_last_good("my.cnf", "abc123").expect("record");
    assert_eq!(store.last_good("my.cnf"), Some("abc123".to_owned()));
    assert_eq!(mode(&store.marker_path("my.cnf")), 0o600);
}

#[test]
fn sha256_hex_matches_a_known_digest() {
    // SHA-256 of "abc".
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn file_sha256_reads_a_file_and_returns_none_when_missing() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("f");
    fs::write(&path, b"abc").expect("write");
    assert_eq!(file_sha256(&path), Some(sha256_hex(b"abc")));
    assert_eq!(file_sha256(&dir.path().join("nope")), None);
}

#[test]
fn take_of_a_missing_source_is_an_error() {
    let dir = tempfile::tempdir().expect("tmp");
    let store = BackupStore::for_instance(dir.path(), "m");
    assert!(store.take(&dir.path().join("nope.conf"), 1).is_err());
}
