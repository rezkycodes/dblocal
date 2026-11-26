#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::update::verify::sha256_file_hex;
use crate::update::{
    AvailableUpdate, MAX_CHECKSUMS_BODY_BYTES, ReleaseAsset, UpdateAssetKind, download_update,
};

struct TestServer {
    addr: SocketAddr,
    hits: Arc<AtomicUsize>,
}

impl TestServer {
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

async fn start_server(body: Vec<u8>) -> TestServer {
    start_server_with_status(200, body).await
}

async fn start_server_with_status(status: u16, body: Vec<u8>) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let body = body.clone();
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                counter.fetch_add(1, Ordering::SeqCst);
                let resp = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    TestServer { addr, hits }
}

#[tokio::test]
async fn download_and_verify_accepts_valid_file() {
    let body = b"hello-update".to_vec();
    let server = start_server(body.clone()).await;
    let dir = tempdir().expect("tmp");
    let dest = dir.path().join("asset.bin");
    tokio::fs::write(&dest.with_extension("tmp-source"), &body)
        .await
        .unwrap();
    let expected = sha256_file_hex(&dest.with_extension("tmp-source"))
        .await
        .unwrap();
    tokio::fs::remove_file(dest.with_extension("tmp-source"))
        .await
        .unwrap();

    download_and_verify(
        &server.url("/asset.bin"),
        &dest,
        &expected,
        body.len() as u64,
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect("download");
    assert_eq!(tokio::fs::read(&dest).await.unwrap(), body);
    assert!(server.hits() >= 1);
}

#[tokio::test]
async fn download_and_verify_rejects_mismatch() {
    let body = b"hello-update".to_vec();
    let server = start_server(body.clone()).await;
    let dir = tempdir().expect("tmp");
    let dest = dir.path().join("asset.bin");
    let err = download_and_verify(
        &server.url("/asset.bin"),
        &dest,
        &"0".repeat(64),
        body.len() as u64,
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect_err("mismatch");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_VERIFY_FAILED);
    assert!(!dest.exists());
}

#[test]
fn validate_url_rejects_non_allowed_host() {
    let url = reqwest::Url::parse("https://evil.example.com/file.tar.gz").unwrap();
    let err = validate_url(&url, &UpdateDownloadOptions::default()).expect_err("host");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
}

#[test]
fn validate_url_rejects_http_in_production() {
    let url = reqwest::Url::parse("http://github.com/file.tar.gz").unwrap();
    let err = validate_url(&url, &UpdateDownloadOptions::default()).expect_err("http");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
}

#[test]
fn validate_url_allows_github_hosts() {
    let opts = UpdateDownloadOptions::default();
    let url = reqwest::Url::parse("https://github.com/a/b/releases/download/v1/x.tar.gz").unwrap();
    validate_url(&url, &opts).expect("github");
    let url = reqwest::Url::parse("https://objects.githubusercontent.com/a/b").unwrap();
    validate_url(&url, &opts).expect("objects");
}

#[test]
fn redirect_policy_rejects_evil_hosts_and_http() {
    let cases = [
        "https://github.com.evil.example/a.tar.gz",
        "https://github.com@evil.example/a.tar.gz",
        "http://github.com/a.tar.gz",
        "http://objects.githubusercontent.com/a.tar.gz",
    ];
    for case in cases {
        let url = reqwest::Url::parse(case).unwrap();
        assert!(!redirect_allowed(&url, false), "case {case}");
        assert!(!redirect_allowed(&url, true), "loopback case {case}");
    }
}

#[test]
fn redirect_policy_allows_expected_hosts_and_loopback() {
    let https = [
        "https://github.com/a/b/releases/download/v1/x.tar.gz",
        "https://objects.githubusercontent.com/a/b",
        "https://github-releases.githubusercontent.com/a/b",
        "https://release-assets.githubusercontent.com/a/b",
    ];
    for case in https {
        let url = reqwest::Url::parse(case).unwrap();
        assert!(redirect_allowed(&url, false), "case {case}");
    }
    let loopback = reqwest::Url::parse("http://127.0.0.1:1234/a").unwrap();
    assert!(redirect_allowed(&loopback, true));
    assert!(!redirect_allowed(&loopback, false));
}

#[test]
fn effective_max_rejects_declared_over_hard_cap() {
    let err = effective_max(MAX_UPDATE_BYTES + 1, MAX_UPDATE_BYTES).expect_err("over");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_MALFORMED);
}

#[test]
fn effective_max_rejects_zero() {
    let err = effective_max(0, MAX_UPDATE_BYTES).expect_err("zero");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_MALFORMED);
}

#[test]
fn redact_url_strips_query() {
    let url = reqwest::Url::parse("https://github.com/a/b?token=secret#frag").unwrap();
    let r = redact_url(&url);
    assert!(!r.contains("secret"));
    assert!(!r.contains("frag"));
}

#[test]
fn validate_url_allows_loopback_http_in_test_mode() {
    let url = reqwest::Url::parse("http://127.0.0.1:1234/file").unwrap();
    validate_url(&url, &UpdateDownloadOptions::loopback_for_tests()).expect("loopback");
}

#[tokio::test]
async fn download_rejects_non_success_status() {
    let body = b"nope".to_vec();
    let server = start_server_with_status(404, body).await;
    let dir = tempdir().expect("tmp");
    let dest = dir.path().join("asset.bin");
    let err = download_and_verify(
        &server.url("/asset.bin"),
        &dest,
        &"1".repeat(64),
        4,
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect_err("404");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
}

#[test]
fn effective_max_allows_slack_under_hard_cap() {
    let got = effective_max(100, MAX_UPDATE_BYTES).unwrap();
    assert_eq!(got, 100 + 1024 * 1024);
}

#[test]
fn effective_max_caps_at_hard_cap() {
    let got = effective_max(64 * 1024 * 1024, MAX_UPDATE_BYTES).unwrap();
    assert!(got <= MAX_UPDATE_BYTES);
}

#[test]
fn url_label_fallback() {
    assert_eq!(url_label("https://github.com/a/b/x.tar.gz"), "x.tar.gz");
}

#[test]
fn validate_url_rejects_unknown_scheme() {
    let url = reqwest::Url::parse("ftp://github.com/a/b").unwrap();
    let err = validate_url(&url, &UpdateDownloadOptions::default()).expect_err("ftp");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
}

#[tokio::test]
async fn download_rejects_zero_declared_size() {
    let dir = tempdir().expect("tmp");
    let dest = dir.path().join("asset.bin");
    let err = download_and_verify(
        "http://127.0.0.1:1/asset.bin",
        &dest,
        &"2".repeat(64),
        0,
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect_err("zero");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_MALFORMED);
}

#[tokio::test]
async fn download_rejects_oversized_declared_size() {
    let dir = tempdir().expect("tmp");
    let dest = dir.path().join("asset.bin");
    let err = download_and_verify(
        "http://127.0.0.1:1/asset.bin",
        &dest,
        &"3".repeat(64),
        MAX_UPDATE_BYTES + 1,
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect_err("big");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_MALFORMED);
}

#[tokio::test]
async fn download_rejects_disallowed_https_host() {
    let dir = tempdir().expect("tmp");
    let dest = dir.path().join("asset.bin");
    let err = download_and_verify(
        "https://example.com/asset.bin",
        &dest,
        &"4".repeat(64),
        10,
        &UpdateDownloadOptions::default(),
    )
    .await
    .expect_err("host");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
}

#[tokio::test]
async fn redirect_to_disallowed_second_server_is_not_contacted() {
    let second_hits = Arc::new(AtomicUsize::new(0));
    let second_hits_clone = second_hits.clone();
    let second = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let second_addr = second.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = second.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            second_hits_clone.fetch_add(1, Ordering::SeqCst);
            let body = b"evil";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            let _ = socket.write_all(body).await;
        }
    });

    let first = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let first_addr = first.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = first.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let location = format!("https://127.0.0.1:{}/evil", second_addr.port());
            let resp = format!(
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        }
    });

    let dir = tempdir().expect("tmp");
    let dest = dir.path().join("asset.bin");
    let err = download_and_verify(
        &format!("http://{first_addr}/asset"),
        &dest,
        &"5".repeat(64),
        4,
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect_err("redirect blocked");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(second_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn download_update_uses_checksums_and_fixed_staging_name() {
    let body = b"hello-update".to_vec();
    let checksum = sha2_hex(&body);
    let checksums_body = format!("{checksum}  dblocal-linux-x86_64.tar.gz\n").into_bytes();
    let checksums_server = start_server(checksums_body).await;
    let asset_server = start_server(body.clone()).await;
    let dir = tempdir().expect("tmp");
    let update = AvailableUpdate::new(
        semver::Version::parse("1.2.3").unwrap(),
        "v1.2.3",
        "notes",
        ReleaseAsset::new(
            "dblocal-linux-x86_64.tar.gz",
            asset_server.url("/asset"),
            body.len() as u64,
            UpdateAssetKind::TarGz,
        ),
        ReleaseAsset::new(
            "checksums.txt",
            checksums_server.url("/checksums.txt"),
            64,
            UpdateAssetKind::Checksums,
        ),
    );
    let staged = download_update(
        &update,
        dir.path(),
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect("download");
    assert_eq!(
        staged.staged_path().file_name().unwrap(),
        "dblocal-update.bin"
    );
    assert_eq!(tokio::fs::read(staged.staged_path()).await.unwrap(), body);
}

#[tokio::test]
async fn download_update_rejects_missing_checksum_entry() {
    let checksums_server =
        start_server(format!("{}  other.tar.gz\n", "a".repeat(64)).into_bytes()).await;
    let dir = tempdir().expect("tmp");
    let update = AvailableUpdate::new(
        semver::Version::parse("1.2.3").unwrap(),
        "v1.2.3",
        "notes",
        ReleaseAsset::new(
            "dblocal-linux-x86_64.tar.gz",
            "http://127.0.0.1:9/asset",
            10,
            UpdateAssetKind::TarGz,
        ),
        ReleaseAsset::new(
            "checksums.txt",
            checksums_server.url("/checksums.txt"),
            64,
            UpdateAssetKind::Checksums,
        ),
    );
    let err = download_update(
        &update,
        dir.path(),
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect_err("missing checksum");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_CHECKSUM);
}

#[tokio::test]
async fn download_update_rejects_oversized_checksums() {
    let big = vec![b'a'; MAX_CHECKSUMS_BODY_BYTES + 1];
    let checksums_server = start_server(big).await;
    let dir = tempdir().expect("tmp");
    let update = AvailableUpdate::new(
        semver::Version::parse("1.2.3").unwrap(),
        "v1.2.3",
        "notes",
        ReleaseAsset::new(
            "dblocal-linux-x86_64.tar.gz",
            "http://127.0.0.1:9/asset",
            10,
            UpdateAssetKind::TarGz,
        ),
        ReleaseAsset::new(
            "checksums.txt",
            checksums_server.url("/checksums.txt"),
            (MAX_CHECKSUMS_BODY_BYTES + 1) as u64,
            UpdateAssetKind::Checksums,
        ),
    );
    let err = download_update(
        &update,
        dir.path(),
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect_err("big checksums");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_CHECKSUM);
}

#[tokio::test]
async fn download_update_rejects_wrong_digest() {
    let body = b"hello-update".to_vec();
    let checksums_body = format!("{}  dblocal-linux-x86_64.tar.gz\n", "0".repeat(64)).into_bytes();
    let checksums_server = start_server(checksums_body).await;
    let asset_server = start_server(body.clone()).await;
    let dir = tempdir().expect("tmp");
    let update = AvailableUpdate::new(
        semver::Version::parse("1.2.3").unwrap(),
        "v1.2.3",
        "notes",
        ReleaseAsset::new(
            "dblocal-linux-x86_64.tar.gz",
            asset_server.url("/asset"),
            body.len() as u64,
            UpdateAssetKind::TarGz,
        ),
        ReleaseAsset::new(
            "checksums.txt",
            checksums_server.url("/checksums.txt"),
            64,
            UpdateAssetKind::Checksums,
        ),
    );
    let err = download_update(
        &update,
        dir.path(),
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect_err("bad digest");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_VERIFY_FAILED);
    assert!(!dir.path().join("dblocal-update.bin").exists());
}

#[tokio::test]
async fn download_update_rejects_oversized_asset() {
    let body = vec![0u8; 4096];
    let checksum = sha2_hex(&body);
    let checksums_body = format!("{checksum}  dblocal-linux-x86_64.tar.gz\n").into_bytes();
    let checksums_server = start_server(checksums_body).await;
    let asset_server = start_server(body.clone()).await;
    let dir = tempdir().expect("tmp");
    let update = AvailableUpdate::new(
        semver::Version::parse("1.2.3").unwrap(),
        "v1.2.3",
        "notes",
        ReleaseAsset::new(
            "dblocal-linux-x86_64.tar.gz",
            asset_server.url("/asset"),
            1024,
            UpdateAssetKind::TarGz,
        ),
        ReleaseAsset::new(
            "checksums.txt",
            checksums_server.url("/checksums.txt"),
            64,
            UpdateAssetKind::Checksums,
        ),
    );
    let err = download_update(
        &update,
        dir.path(),
        &UpdateDownloadOptions::loopback_for_tests_with_max(1024),
    )
    .await
    .expect_err("oversized asset");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
    assert!(!dir.path().join("dblocal-update.bin").exists());
}

#[tokio::test]
async fn download_update_rejects_truncated_asset() {
    let body = b"short".to_vec();
    let checksum = sha2_hex(b"much-longer-expected-body");
    let checksums_body = format!("{checksum}  dblocal-linux-x86_64.tar.gz\n").into_bytes();
    let checksums_server = start_server(checksums_body).await;
    let asset_server = start_server(body.clone()).await;
    let dir = tempdir().expect("tmp");
    let update = AvailableUpdate::new(
        semver::Version::parse("1.2.3").unwrap(),
        "v1.2.3",
        "notes",
        ReleaseAsset::new(
            "dblocal-linux-x86_64.tar.gz",
            asset_server.url("/asset"),
            24,
            UpdateAssetKind::TarGz,
        ),
        ReleaseAsset::new(
            "checksums.txt",
            checksums_server.url("/checksums.txt"),
            64,
            UpdateAssetKind::Checksums,
        ),
    );
    let err = download_update(
        &update,
        dir.path(),
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect_err("truncated asset");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_VERIFY_FAILED);
}

fn sha2_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn is_loopback_recognizes_localhost() {
    let url = reqwest::Url::parse("http://localhost:1234/x").unwrap();
    assert!(is_loopback(&url));
    let url = reqwest::Url::parse("http://127.0.0.1/x").unwrap();
    assert!(is_loopback(&url));
}

#[test]
fn staged_update_accessors() {
    let dir = tempdir().expect("tmp");
    let path = dir.path().join("dblocal-update.bin");
    let staged = StagedUpdate {
        version: semver::Version::parse("2.0.0").unwrap(),
        staged_path: path.clone(),
        run_dir: dir.path().to_path_buf(),
        asset_name: "dblocal-linux-x86_64.tar.gz".to_owned(),
        asset_kind: super::super::model::UpdateAssetKind::TarGz,
    };
    assert_eq!(staged.version().to_string(), "2.0.0");
    assert_eq!(staged.staged_path(), path.as_path());
    assert_eq!(staged.asset_name(), "dblocal-linux-x86_64.tar.gz");
    assert_eq!(
        staged.asset_kind(),
        super::super::model::UpdateAssetKind::TarGz
    );
}

#[tokio::test]
async fn read_bounded_bytes_rejects_oversize_and_missing() {
    let dir = tempdir().expect("tmp");
    let big = dir.path().join("big.txt");
    tokio::fs::write(&big, vec![b'a'; 32]).await.unwrap();
    let err = read_bounded_bytes(&big, 16).await.expect_err("oversize");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_CHECKSUM);

    let missing = dir.path().join("missing.txt");
    let err = read_bounded_bytes(&missing, 16).await.expect_err("missing");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_CHECKSUM);
}

#[test]
fn parse_and_validate_url_rejects_garbage() {
    let err =
        parse_and_validate_url("not a url", &UpdateDownloadOptions::default()).expect_err("bad");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_MALFORMED);
}

#[test]
fn is_loopback_handles_ipv6_and_non_network_scheme() {
    let url = reqwest::Url::parse("https://[::1]:8443/x").unwrap();
    assert!(is_loopback(&url));
    let url = reqwest::Url::parse("file:///tmp/x").unwrap();
    assert!(!is_loopback(&url));
}

#[tokio::test]
async fn download_resumes_a_partial_file() {
    let body = b"hello-update-full".to_vec();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().unwrap();
    let full = body.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let full = full.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                if let Some(rest) = req.split("range: bytes=").nth(1) {
                    let start = rest
                        .split('-')
                        .next()
                        .and_then(|s| s.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let slice = &full[start..];
                    let head = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                        slice.len(),
                        start,
                        full.len() - 1,
                        full.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(slice).await;
                } else {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        full.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&full).await;
                }
            });
        }
    });

    let dir = tempdir().expect("tmp");
    let dest = dir.path().join("asset.bin");
    tokio::fs::write(&dest, &body[..5]).await.unwrap();
    let hash = sha2_hex(&body);
    download_and_verify(
        &format!("http://{addr}/asset"),
        &dest,
        &hash,
        body.len() as u64,
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect("resume");
    assert_eq!(tokio::fs::read(&dest).await.unwrap(), body);
}
