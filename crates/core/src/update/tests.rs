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
    let second_hits = Arc::new(AtomicUsize::new(0));
    let second_hits_clone = second_hits.clone();
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second_addr = second.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = second.accept().await {
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await;
            second_hits_clone.fetch_add(1, Ordering::SeqCst);
            let resp = "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nevil";
            let _ = socket.write_all(resp.as_bytes()).await;
        }
    });

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1:{}/evil\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                second_addr.port()
            );
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
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(second_hits.load(Ordering::SeqCst), 0);
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
    let opts = UpdateDownloadOptions::loopback_for_tests_with_max(1024);
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
fn redirect_policy_rejects_public_http_and_bad_hosts() {
    for case in [
        "http://github.com/a/b",
        "https://github.com.evil.example/a",
        "https://github.com@evil.example/a",
        "https://example.com/a",
    ] {
        let url = reqwest::Url::parse(case).unwrap();
        assert!(!redirect_allowed(&url, false), "{case}");
    }
}

#[test]
fn downgrade_refused_error_code() {
    let err = UpdateError::DowngradeRefused {
        reason: "old".to_owned(),
    };
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_DOWNGRADE);
    assert!(!err.user_message().is_empty());
    assert_eq!(crate::update::error::CODE_UPDATE_UP_TO_DATE, "DBL-8001");
}

#[test]
fn install_kind_unknown_refused() {
    let plan = plan_install(
        &InstallKind::Unknown {
            exe_path: std::path::PathBuf::from("/tmp/dblocal"),
            reason: "unknown".to_owned(),
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
            let head = "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n";
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

#[test]
fn error_codes_and_messages_are_stable() {
    let cases = vec![
        UpdateError::Malformed { reason: "x".into() },
        UpdateError::RateLimited { reason: "x".into() },
        UpdateError::Network { reason: "x".into() },
        UpdateError::ChecksumMetadata { reason: "x".into() },
        UpdateError::VerifyFailed { reason: "x".into() },
        UpdateError::DowngradeRefused { reason: "x".into() },
        UpdateError::UnsupportedInstallKind { reason: "x".into() },
        UpdateError::AssetMissing { reason: "x".into() },
        UpdateError::ApplyFailed { reason: "x".into() },
    ];
    for e in cases {
        assert!(e.code().starts_with("DBL-8"));
        assert!(!e.user_message().is_empty());
        assert!(e.diagnostic().contains(e.code()));
        let _ = e.to_string();
        let _ = e.detail();
        let _d: crate::DblError = e.clone().into();
    }
}

#[test]
fn model_accessors_and_tokens() {
    let asset = ReleaseAsset::new(
        "a.tar.gz",
        "https://github.com/a/b/a.tar.gz",
        1,
        UpdateAssetKind::TarGz,
    );
    assert_eq!(asset.name(), "a.tar.gz");
    assert_eq!(asset.url(), "https://github.com/a/b/a.tar.gz");
    assert_eq!(asset.size_bytes(), 1);
    assert!(asset.kind().is_installable());
    assert!(!UpdateAssetKind::Checksums.is_installable());
    let _ = format!("{}", UpdateAssetKind::AppImage);
    let u = AvailableUpdate::new(
        Version::parse("1.0.0").unwrap(),
        "v1.0.0",
        "notes",
        asset.clone(),
        asset.with_kind(UpdateAssetKind::Checksums),
    );
    assert_eq!(u.tag(), "v1.0.0");
    assert_eq!(u.release_notes(), "notes");
    assert_eq!(u.checksums_asset().kind(), UpdateAssetKind::Checksums);
    let c = UpdateCheck::UpToDate {
        current: Version::parse("1.0.0").unwrap(),
    };
    assert!(c.available().is_none());
    assert!(platform_asset_tokens(Os::Linux, Arch::X86_64).is_some());
    assert!(platform_asset_tokens(Os::Windows, Arch::Aarch64).is_some());
}

#[test]
fn install_kind_paths_and_hints() {
    let k = InstallKind::MacApp {
        bundle_path: std::path::PathBuf::from("/Applications/DBLocal.app"),
    };
    assert!(!k.can_self_update());
    assert_eq!(k.path(), Path::new("/Applications/DBLocal.app"));
    let k = InstallKind::WindowsZip {
        exe_path: std::path::PathBuf::from("C:\\d\\dblocal.exe"),
    };
    assert!(k.can_self_update());
    let k = InstallKind::WindowsMsi {
        exe_path: std::path::PathBuf::from("C:\\Program Files\\DBLocal\\dblocal.exe"),
    };
    assert!(!k.can_self_update());
    let k_user = InstallKind::WindowsMsi {
        exe_path: std::path::PathBuf::from("C:\\Users\\me\\AppData\\Local\\DBLocal\\dblocal.exe"),
    };
    assert!(k_user.can_self_update());
    let h = PackageManagerHint::Rpm;
    assert!(h.message().contains("rpm"));
    let h = PackageManagerHint::Other {
        manager: "pacman".into(),
    };
    assert!(h.message().contains("pacman"));
}

#[test]
fn asset_kind_display_covers_every_variant() {
    assert_eq!(UpdateAssetKind::AppImage.to_string(), "appimage");
    assert_eq!(UpdateAssetKind::TarGz.to_string(), "tar.gz");
    assert_eq!(UpdateAssetKind::Zip.to_string(), "zip");
    assert_eq!(UpdateAssetKind::Msi.to_string(), "msi");
    assert_eq!(UpdateAssetKind::Dmg.to_string(), "dmg");
    assert_eq!(UpdateAssetKind::Checksums.to_string(), "checksums");
}

#[test]
fn platform_tokens_cover_all_supported_pairs() {
    for (os, arch) in [
        (Os::Linux, Arch::X86_64),
        (Os::Linux, Arch::Aarch64),
        (Os::Macos, Arch::X86_64),
        (Os::Macos, Arch::Aarch64),
        (Os::Windows, Arch::X86_64),
        (Os::Windows, Arch::Aarch64),
    ] {
        assert!(platform_asset_tokens(os, arch).is_some(), "{os}-{arch}");
    }
}

#[test]
fn update_check_available_returns_some() {
    let asset = ReleaseAsset::new(
        "a.tar.gz",
        "https://github.com/a",
        1,
        UpdateAssetKind::TarGz,
    );
    let update = AvailableUpdate::new(
        Version::parse("2.0.0").unwrap(),
        "v2.0.0",
        "notes",
        asset.clone(),
        asset.with_kind(UpdateAssetKind::Checksums),
    );
    let check = UpdateCheck::Available(update);
    assert!(check.available().is_some());
}
