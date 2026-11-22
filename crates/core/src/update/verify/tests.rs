#![allow(clippy::unwrap_used, clippy::expect_used)]

use tempfile::tempdir;

use super::*;

#[test]
fn parse_checksums_basic() {
    let hash = "a".repeat(64);
    let body = format!("{hash}  dblocal-linux-x86_64.tar.gz\n");
    let c = Checksums::parse(&body).expect("parse");
    assert_eq!(c.len(), 1);
    assert_eq!(
        c.digest_for("dblocal-linux-x86_64.tar.gz"),
        Some(hash.as_str())
    );
}

#[test]
fn parse_checksums_ignores_comments_and_blank_lines() {
    let hash = "b".repeat(64);
    let body = format!("# comment\n\n{hash} *dblocal.zip\n");
    let c = Checksums::parse(&body).expect("parse");
    assert_eq!(c.digest_for("dblocal.zip"), Some(hash.as_str()));
}

#[test]
fn parse_checksums_rejects_invalid_line() {
    let err = Checksums::parse("garbage").expect_err("invalid");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_CHECKSUM);
}

#[test]
fn parse_checksums_rejects_bad_hash() {
    let err = Checksums::parse("zzzz  file.tar.gz").expect_err("bad hash");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_CHECKSUM);
}

#[test]
fn parse_checksums_rejects_path_in_name() {
    let hash = "c".repeat(64);
    let err = Checksums::parse(&format!("{hash} ../evil.tar.gz")).expect_err("path");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_CHECKSUM);
}

#[test]
fn parse_checksums_lowercases_hash_and_rejects_duplicates() {
    let hash = "A".repeat(64);
    let body = format!("{hash}  file.tar.gz\n{hash}  file.tar.gz\n");
    let err = Checksums::parse(&body).expect_err("dup");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_CHECKSUM);

    let body = format!("{hash}  file.tar.gz\n");
    let c = Checksums::parse(&body).expect("parse");
    assert_eq!(c.digest_for("file.tar.gz"), Some("a".repeat(64).as_str()));
}

#[test]
fn parse_checksums_missing_asset() {
    let hash = "d".repeat(64);
    let c = Checksums::parse(&format!("{hash}  other.tar.gz")).expect("parse");
    assert_eq!(c.digest_for("missing.tar.gz"), None);
}

#[tokio::test]
async fn verify_sha256_file_accepts_match() {
    let dir = tempdir().expect("tmp");
    let path = dir.path().join("asset.bin");
    tokio::fs::write(&path, b"hello").await.expect("write");
    let expected = sha256_file_hex(&path).await.expect("hash");
    verify_sha256_file(&path, &expected, "asset.bin")
        .await
        .expect("match");
    assert!(path.exists());
}

#[tokio::test]
async fn verify_sha256_file_rejects_mismatch_and_deletes() {
    let dir = tempdir().expect("tmp");
    let path = dir.path().join("asset.bin");
    tokio::fs::write(&path, b"hello").await.expect("write");
    let err = verify_sha256_file(&path, &"0".repeat(64), "asset.bin")
        .await
        .expect_err("mismatch");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_VERIFY_FAILED);
    assert!(!path.exists(), "mismatch must delete file");
}

#[test]
fn constant_time_eq_hex_works() {
    let a = "a".repeat(64);
    let b = "a".repeat(64);
    let c = "b".repeat(64);
    assert!(constant_time_eq_hex(&a, &b));
    assert!(!constant_time_eq_hex(&a, &c));
    assert!(!constant_time_eq_hex(&a, "a"));
}

#[test]
fn parse_checksum_line_rejects_extra_fields_and_empty() {
    let hash = "a".repeat(64);
    assert!(parse_checksum_line(&format!("{hash}  name extra")).is_none());
    assert!(parse_checksum_line("").is_none());
    assert!(parse_checksum_line(&hash).is_none());
    assert!(parse_checksum_line(&format!("{hash}  ")).is_none());
}

#[test]
fn checksums_len_and_is_empty() {
    let empty = Checksums::default();
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
    let body = format!("{}  x.tar.gz\n", "a".repeat(64));
    let c = Checksums::parse(&body).unwrap();
    assert!(!c.is_empty());
    assert_eq!(c.len(), 1);
}

#[tokio::test]
async fn sha256_file_hex_reports_missing_file() {
    let dir = tempdir().expect("tmp");
    let missing = dir.path().join("nope");
    let err = sha256_file_hex(&missing).await.expect_err("missing");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
}
