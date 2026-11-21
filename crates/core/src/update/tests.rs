#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use semver::Version;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::platform::{Arch, Os};

struct MockFetcher {
    responses: std::sync::Mutex<Vec<std::result::Result<FetchResponse, String>>>,
}

impl MockFetcher {
    fn new(responses: Vec<std::result::Result<FetchResponse, String>>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
        }
    }
}

impl UpdateFetcher for MockFetcher {
    fn get(
        &self,
        url: String,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::result::Result<FetchResponse, String>>
                + Send
                + '_,
        >,
    > {
        let _ = url;
        let resp = self.responses.lock().expect("lock").remove(0);
        Box::pin(async move { resp })
    }
}

fn release_json(tag: &str, asset_name: &str, asset_url: &str, checksums_url: &str) -> String {
    serde_json::json!([{
        "tag_name": tag,
        "draft": false,
        "prerelease": false,
        "body": "notes",
        "assets": [
            {"name": asset_name, "browser_download_url": asset_url, "size": 100},
            {"name": "checksums.txt", "browser_download_url": checksums_url, "size": 80}
        ]
    }])
    .to_string()
}

#[tokio::test]
async fn check_rejects_downgrade_candidate() {
    let json = release_json(
        "v0.9.0",
        "dblocal-linux-x86_64.tar.gz",
        "https://github.com/a/b/releases/download/v0.9.0/dblocal-linux-x86_64.tar.gz",
        "https://github.com/a/b/releases/download/v0.9.0/checksums.txt",
    );
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, json.into_bytes()))]);
    let opts = UpdateCheckOptions::for_repo("a/b").with_platform(Os::Linux, Arch::X86_64);
    let result = check(&Version::parse("1.0.0").unwrap(), &fetcher, &opts)
        .await
        .unwrap();
    assert!(matches!(result, UpdateCheck::UpToDate { .. }));
}

#[tokio::test]
async fn check_ignores_garbage_version_tags() {
    let json = serde_json::json!([{
        "tag_name": "not-a-version",
        "draft": false,
        "prerelease": false,
        "body": "notes",
        "assets": [
            {"name":"dblocal-linux-x86_64.tar.gz","browser_download_url":"https://github.com/a/b/releases/download/x/a.tar.gz","size":1},
            {"name":"checksums.txt","browser_download_url":"https://github.com/a/b/releases/download/x/checksums.txt","size":1}
        ]
    }]).to_string();
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, json.into_bytes()))]);
    let opts = UpdateCheckOptions::for_repo("a/b").with_platform(Os::Linux, Arch::X86_64);
    let result = check(&Version::parse("1.0.0").unwrap(), &fetcher, &opts)
        .await
        .unwrap();
    assert!(matches!(result, UpdateCheck::UpToDate { .. }));
}

#[test]
fn checksums_missing_asset_is_reported() {
    let body = format!("{}  other.tar.gz\n", "a".repeat(64));
    let checksums = Checksums::parse(&body).unwrap();
    assert!(
        checksums
            .digest_for("dblocal-linux-x86_64.tar.gz")
            .is_none()
    );
}

#[tokio::test]
async fn download_rejects_redirect_to_disallowed_host() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await;
            counter.fetch_add(1, Ordering::SeqCst);
            let resp = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/evil\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = socket.write_all(resp.as_bytes()).await;
        }
    });
    let dir = tempdir().unwrap();
    let dest = dir.path().join("asset");
    let err = download_and_verify(
        &format!("http://{addr}/asset"),
        &dest,
        &"b".repeat(64),
        10,
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect_err("redirect should be rejected");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
    assert!(!dest.exists());
}

#[tokio::test]
async fn download_rejects_oversized_body() {
    let body = vec![0u8; 2048];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await;
            let head = "HTTP/1.1 200 OK\r\nContent-Length: 2048\r\nConnection: close\r\n\r\n";
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(&body).await;
        }
    });
    let dir = tempdir().unwrap();
    let dest = dir.path().join("asset");
    let mut opts = UpdateDownloadOptions::loopback_for_tests();
    opts.max_bytes = 1024;
    let err = download_and_verify(
        &format!("http://{addr}/asset"),
        &dest,
        &"c".repeat(64),
        1024,
        &opts,
    )
    .await
    .expect_err("oversized");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
    assert!(!dest.exists());
}

#[test]
fn host_allowlist_rejects_evil_host() {
    let url = reqwest::Url::parse("https://github.com.evil.com/a.tar.gz").unwrap();
    let err = validate_url(&url, &UpdateDownloadOptions::default()).expect_err("evil");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
}

#[test]
fn downgrade_refused_error_code() {
    let err = UpdateError::DowngradeRefused {
        reason: "old".to_owned(),
    };
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_DOWNGRADE);
    assert!(!err.user_message().is_empty());
}

#[test]
fn install_kind_unknown_refused() {
    let plan = plan_install(
        &InstallKind::Unknown {
            exe_path: std::path::PathBuf::from("/tmp/dblocal"),
        },
        Path::new("/tmp/staged"),
        Path::new("/tmp/dblocal"),
        &Version::parse("1.0.0").unwrap(),
    )
    .unwrap();
    assert!(matches!(plan.steps()[0], InstallStep::Refuse { .. }));
}

#[test]
fn downgrade_allowed_only_when_flag_set() {
    let json = release_json(
        "v0.9.0",
        "dblocal-linux-x86_64.tar.gz",
        "https://github.com/a/b/releases/download/v0.9.0/dblocal-linux-x86_64.tar.gz",
        "https://github.com/a/b/releases/download/v0.9.0/checksums.txt",
    );
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, json.clone().into_bytes()))]);
    let opts = UpdateCheckOptions::for_repo("a/b")
        .with_platform(Os::Linux, Arch::X86_64)
        .with_allow_downgrade(true);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(check(&Version::parse("1.0.0").unwrap(), &fetcher, &opts))
        .unwrap();
    assert!(matches!(result, UpdateCheck::Available(_)));
}

#[test]
fn checksums_rejects_path_traversal_name() {
    let hash = "d".repeat(64);
    let err = Checksums::parse(&format!("{hash}  ../evil.tar.gz")).expect_err("bad");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_CHECKSUM);
}

#[test]
fn sanitize_notes_removes_control_chars() {
    let out = sanitize_release_notes("hello\u{0000}world\nline2");
    assert!(!out.contains('\u{0000}'));
    assert!(out.contains("hello"));
}

#[tokio::test]
async fn download_rejects_http_in_production_options() {
    let dir = tempdir().unwrap();
    let dest = dir.path().join("a");
    let err = download_and_verify(
        "http://github.com/a/b/releases/download/v1/a.tar.gz",
        &dest,
        &"e".repeat(64),
        10,
        &UpdateDownloadOptions::default(),
    )
    .await
    .expect_err("http rejected");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
}

#[tokio::test]
async fn check_rejects_release_without_checksums_asset() {
    let json = serde_json::json!([{
        "tag_name": "v1.1.0",
        "draft": false,
        "prerelease": false,
        "body": "notes",
        "assets": [
            {"name":"dblocal-linux-x86_64.tar.gz","browser_download_url":"https://github.com/a/b/releases/download/v1.1/dblocal-linux-x86_64.tar.gz","size":1}
        ]
    }]).to_string();
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, json.into_bytes()))]);
    let opts = UpdateCheckOptions::for_repo("a/b").with_platform(Os::Linux, Arch::X86_64);
    let result = check(&Version::parse("1.0.0").unwrap(), &fetcher, &opts)
        .await
        .unwrap();
    assert!(matches!(result, UpdateCheck::UpToDate { .. }));
}

#[tokio::test]
async fn check_rejects_unsafe_asset_name() {
    let json = serde_json::json!([{
        "tag_name": "v1.1.0",
        "draft": false,
        "prerelease": false,
        "body": "notes",
        "assets": [
            {"name":"../dblocal-linux-x86_64.tar.gz","browser_download_url":"https://github.com/a/b/releases/download/v1.1/dblocal-linux-x86_64.tar.gz","size":1},
            {"name":"checksums.txt","browser_download_url":"https://github.com/a/b/releases/download/v1.1/checksums.txt","size":1}
        ]
    }]).to_string();
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, json.into_bytes()))]);
    let opts = UpdateCheckOptions::for_repo("a/b").with_platform(Os::Linux, Arch::X86_64);
    let err = check(&Version::parse("1.0.0").unwrap(), &fetcher, &opts)
        .await
        .expect_err("bad name");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_MALFORMED);
}

#[tokio::test]
async fn download_rejects_truncated_body_on_clean_eof() {
    // server sends fewer bytes than declared size; download should fail verification or size check path
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await;
            let body = b"short";
            let head =
                format!("HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n");
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(body).await;
        }
    });
    let dir = tempdir().unwrap();
    let dest = dir.path().join("asset");
    let err = download_and_verify(
        &format!("http://{addr}/asset"),
        &dest,
        &"f".repeat(64),
        100,
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect_err("truncated");
    assert!(matches!(
        err.code(),
        crate::update::error::CODE_UPDATE_VERIFY_FAILED
            | crate::update::error::CODE_UPDATE_NETWORK
            | crate::update::error::CODE_UPDATE_APPLY_FAILED
    ));
}
