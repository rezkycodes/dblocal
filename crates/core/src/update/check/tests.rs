#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Mutex;

use super::*;

struct MockFetcher {
    responses: Mutex<Vec<std::result::Result<FetchResponse, String>>>,
}

impl MockFetcher {
    fn new(responses: Vec<std::result::Result<FetchResponse, String>>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

impl UpdateFetcher for MockFetcher {
    fn get(
        &self,
        _url: String,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<FetchResponse, String>> + Send + '_>> {
        let resp = self.responses.lock().expect("lock").remove(0);
        Box::pin(async move { resp })
    }
}

fn asset(name: &str, url: &str) -> GhAsset {
    GhAsset {
        name: name.to_owned(),
        browser_download_url: url.to_owned(),
        size: 123,
    }
}

fn linux_options() -> UpdateCheckOptions {
    UpdateCheckOptions::for_repo("rezkycodes/dblocal").with_platform(Os::Linux, Arch::X86_64)
}

fn sample_release_json(tag: &str) -> String {
    serde_json::json!([{
        "tag_name": tag,
        "draft": false,
        "prerelease": false,
        "body": "Hello",
        "assets": [
            {"name":"dblocal-linux-x86_64.tar.gz","browser_download_url":"https://github.com/a/b/releases/download/v1/dblocal-linux-x86_64.tar.gz","size":100},
            {"name":"checksums.txt","browser_download_url":"https://github.com/a/b/releases/download/v1/checksums.txt","size":10}
        ]
    }]).to_string()
}

#[tokio::test]
async fn check_returns_up_to_date_when_no_newer_release() {
    let json = sample_release_json("v1.0.0");
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, json.into_bytes()))]);
    let result = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .expect("check");
    assert!(matches!(result, UpdateCheck::UpToDate { .. }));
}

#[tokio::test]
async fn check_returns_available_for_newer_release() {
    let json = sample_release_json("v1.1.0");
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, json.into_bytes()))]);
    let result = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .expect("check");
    match result {
        UpdateCheck::Available(u) => {
            assert_eq!(u.version(), &Version::parse("1.1.0").unwrap());
            assert_eq!(u.asset().name(), "dblocal-linux-x86_64.tar.gz");
            assert_eq!(u.checksums_asset().name(), "checksums.txt");
        }
        _ => panic!("expected available"),
    }
}

#[tokio::test]
async fn check_ignores_draft_and_prerelease_by_default() {
    let json = serde_json::json!([
        {
            "tag_name":"v2.0.0",
            "draft": true,
            "prerelease": false,
            "body":"draft",
            "assets":[
                {"name":"dblocal-linux-x86_64.tar.gz","browser_download_url":"https://github.com/a/b/releases/download/v2/dblocal-linux-x86_64.tar.gz","size":1},
                {"name":"checksums.txt","browser_download_url":"https://github.com/a/b/releases/download/v2/checksums.txt","size":1}
            ]
        },
        {
            "tag_name":"v1.5.0-rc.1",
            "draft": false,
            "prerelease": true,
            "body":"rc",
            "assets":[
                {"name":"dblocal-linux-x86_64.tar.gz","browser_download_url":"https://github.com/a/b/releases/download/v1.5/dblocal-linux-x86_64.tar.gz","size":1},
                {"name":"checksums.txt","browser_download_url":"https://github.com/a/b/releases/download/v1.5/checksums.txt","size":1}
            ]
        },
        {
            "tag_name":"v1.4.0-rc.1",
            "draft": false,
            "prerelease": false,
            "body":"rc-marked-stable",
            "assets":[
                {"name":"dblocal-linux-x86_64.tar.gz","browser_download_url":"https://github.com/a/b/releases/download/v1.4/dblocal-linux-x86_64.tar.gz","size":1},
                {"name":"checksums.txt","browser_download_url":"https://github.com/a/b/releases/download/v1.4/checksums.txt","size":1}
            ]
        },
        {
            "tag_name":"v1.1.0",
            "draft": false,
            "prerelease": false,
            "body":"stable",
            "assets":[
                {"name":"dblocal-linux-x86_64.tar.gz","browser_download_url":"https://github.com/a/b/releases/download/v1.1/dblocal-linux-x86_64.tar.gz","size":1},
                {"name":"checksums.txt","browser_download_url":"https://github.com/a/b/releases/download/v1.1/checksums.txt","size":1}
            ]
        }
    ]).to_string();
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, json.into_bytes()))]);
    let result = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .unwrap();
    match result {
        UpdateCheck::Available(u) => assert_eq!(u.version().to_string(), "1.1.0"),
        _ => panic!("expected stable update"),
    }
}

#[tokio::test]
async fn check_includes_prerelease_when_preview_channel() {
    let json = serde_json::json!([{
        "tag_name":"v1.5.0-rc.1",
        "draft": false,
        "prerelease": true,
        "body":"rc",
        "assets":[
            {"name":"dblocal-linux-x86_64.tar.gz","browser_download_url":"https://github.com/a/b/releases/download/v1.5/dblocal-linux-x86_64.tar.gz","size":1},
            {"name":"checksums.txt","browser_download_url":"https://github.com/a/b/releases/download/v1.5/checksums.txt","size":1}
        ]
    }]).to_string();
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, json.into_bytes()))]);
    let opts = linux_options().with_channel(ReleaseChannel::Preview);
    let result = check(&Version::parse("1.0.0").unwrap(), &fetcher, &opts)
        .await
        .unwrap();
    assert!(matches!(result, UpdateCheck::Available(_)));
}

#[tokio::test]
async fn check_reports_rate_limit() {
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(
        403,
        br#"{"message":"API rate limit exceeded"}"#.to_vec(),
    ))]);
    let err = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .expect_err("rate limit");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_RATE_LIMITED);
}

#[tokio::test]
async fn check_reports_network_failure() {
    let fetcher = MockFetcher::new(vec![Err("connection refused".to_owned())]);
    let err = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .expect_err("network");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
}

#[tokio::test]
async fn check_maps_an_unknown_repository_to_source_not_found() {
    // A 404 from the releases API means the repository does not exist or is not
    // public; "check your connection" would be a wrong diagnosis.
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(
        404,
        br#"{"message":"Not Found"}"#.to_vec(),
    ))]);
    let err = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .expect_err("source not found");
    assert_eq!(
        err.code(),
        crate::update::error::CODE_UPDATE_SOURCE_NOT_FOUND
    );
    assert_ne!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
}

#[tokio::test]
async fn check_maps_a_non_rate_limit_403_to_source_not_found() {
    // 403 without a rate-limit body is a private/gone repository, not a rate
    // limit and not a connection failure.
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(
        403,
        br#"{"message":"Forbidden"}"#.to_vec(),
    ))]);
    let err = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .expect_err("source not found");
    assert_eq!(
        err.code(),
        crate::update::error::CODE_UPDATE_SOURCE_NOT_FOUND
    );
}

#[tokio::test]
async fn check_reports_malformed_json() {
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, b"not json".to_vec()))]);
    let err = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .expect_err("malformed");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_MALFORMED);
}

#[test]
fn sanitize_release_notes_bounds_length() {
    let long = "a".repeat(MAX_RELEASE_NOTES_BYTES + 100);
    let out = sanitize_release_notes(&long);
    assert!(out.len() <= MAX_RELEASE_NOTES_BYTES);
}

#[test]
fn releases_url_rejects_bad_repo() {
    assert!(releases_url("../evil/repo").is_err());
    assert!(releases_url("owner").is_err());
    assert!(releases_url("owner/name/extra").is_err());
}

#[test]
fn select_platform_asset_picks_appimage_over_tarball() {
    let assets = vec![
        asset(
            "dblocal-linux-x86_64.tar.gz",
            "https://example.com/a.tar.gz",
        ),
        asset(
            "dblocal-linux-x86_64.AppImage",
            "https://example.com/a.AppImage",
        ),
        asset("checksums.txt", "https://example.com/checksums.txt"),
    ];
    let selected = select_platform_asset(&assets, Os::Linux, Arch::X86_64, None)
        .unwrap()
        .unwrap();
    assert_eq!(selected.kind(), UpdateAssetKind::AppImage);
}

#[tokio::test]
async fn check_reports_non_200_status() {
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(500, b"oops".to_vec()))]);
    let err = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .expect_err("500");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
}

#[tokio::test]
async fn check_reports_oversized_body() {
    let body = vec![b'a'; MAX_RELEASES_BODY_BYTES + 1];
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, body))]);
    let err = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .expect_err("oversized");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_MALFORMED);
}

#[tokio::test]
async fn check_reports_429_rate_limit() {
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(429, b"rate".to_vec()))]);
    let err = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .expect_err("429");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_RATE_LIMITED);
}

#[test]
fn releases_url_rejects_whitespace_and_backslash() {
    assert!(releases_url("a /b").is_err());
    assert!(releases_url("a\\b").is_err());
    assert!(releases_url("").is_err());
}

#[test]
fn sanitize_release_notes_handles_control_and_newlines() {
    let out = sanitize_release_notes("a\r\nb\tc");
    assert_eq!(out, "a b c");
}

#[test]
fn parse_release_version_rejects_garbage() {
    assert!(parse_release_version("nope").is_err());
}

#[test]
fn is_repo_segment_rejects_empty() {
    assert!(!is_repo_segment(""));
}

#[test]
fn looks_rate_limited_by_body() {
    assert!(looks_rate_limited(403, "API rate limit exceeded"));
    assert!(!looks_rate_limited(403, "forbidden"));
}

#[tokio::test]
async fn check_returns_up_to_date_when_no_matching_platform_asset() {
    let json = serde_json::json!([{
        "tag_name": "v1.1.0",
        "draft": false,
        "prerelease": false,
        "body": "notes",
        "assets": [
            {"name":"dblocal-windows-x86_64.zip","browser_download_url":"https://github.com/a/b/releases/download/v1.1/dblocal-windows-x86_64.zip","size":1},
            {"name":"checksums.txt","browser_download_url":"https://github.com/a/b/releases/download/v1.1/checksums.txt","size":1}
        ]
    }]).to_string();
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, json.into_bytes()))]);
    let result = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .unwrap();
    assert!(matches!(result, UpdateCheck::UpToDate { .. }));
}

#[tokio::test]
async fn check_skips_release_when_no_checksums() {
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
    let result = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .unwrap();
    assert!(matches!(result, UpdateCheck::UpToDate { .. }));
}

#[test]
fn sanitize_release_notes_truncates_on_char_boundary() {
    let mut s = String::new();
    while s.len() < MAX_RELEASE_NOTES_BYTES + 10 {
        s.push('é');
    }
    let out = sanitize_release_notes(&s);
    assert!(out.len() <= MAX_RELEASE_NOTES_BYTES);
}

#[test]
fn validate_asset_name_rejects_control_and_slashes() {
    assert!(validate_asset_name("a/b").is_err());
    assert!(validate_asset_name("a\\b").is_err());
    assert!(validate_asset_name("..a").is_err());
    assert!(validate_asset_name("a\0b").is_err());
}

#[test]
fn classify_install_asset_covers_all_platforms() {
    assert_eq!(
        classify_install_asset("dblocal-linux-x86_64.AppImage", Os::Linux),
        Some(UpdateAssetKind::AppImage)
    );
    assert_eq!(
        classify_install_asset("dblocal-linux-x86_64.tgz", Os::Linux),
        Some(UpdateAssetKind::TarGz)
    );
    assert_eq!(classify_install_asset("dblocal.exe", Os::Linux), None);
    assert_eq!(
        classify_install_asset("dblocal-windows-x86_64.zip", Os::Windows),
        Some(UpdateAssetKind::Zip)
    );
    assert_eq!(
        classify_install_asset("dblocal-windows-x86_64.msi", Os::Windows),
        Some(UpdateAssetKind::Msi)
    );
    assert_eq!(
        classify_install_asset("dblocal-windows-x86_64.tar.gz", Os::Windows),
        None
    );
    assert_eq!(
        classify_install_asset("dblocal-macos-universal.dmg", Os::Macos),
        Some(UpdateAssetKind::Dmg)
    );
    assert_eq!(
        classify_install_asset("dblocal-macos-universal.tar.gz", Os::Macos),
        Some(UpdateAssetKind::TarGz)
    );
    assert_eq!(
        classify_install_asset("dblocal-macos-universal.zip", Os::Macos),
        None
    );
}

#[test]
fn asset_priority_orders_kinds() {
    assert!(asset_priority(UpdateAssetKind::AppImage) > asset_priority(UpdateAssetKind::TarGz));
    assert!(asset_priority(UpdateAssetKind::TarGz) > asset_priority(UpdateAssetKind::Zip));
    assert!(asset_priority(UpdateAssetKind::Zip) > asset_priority(UpdateAssetKind::Msi));
    assert!(asset_priority(UpdateAssetKind::Msi) > asset_priority(UpdateAssetKind::Checksums));
}

#[test]
fn map_network_classifies_rate_limit_messages() {
    assert_eq!(
        map_network("HTTP 429 too many requests".to_owned()).code(),
        crate::update::error::CODE_UPDATE_RATE_LIMITED
    );
    assert_eq!(
        map_network("API rate limit exceeded".to_owned()).code(),
        crate::update::error::CODE_UPDATE_RATE_LIMITED
    );
    assert_eq!(
        map_network("connection reset".to_owned()).code(),
        crate::update::error::CODE_UPDATE_NETWORK
    );
}

#[test]
fn select_platform_asset_rejects_unsupported_platform() {
    let err = platform_asset_tokens(Os::Windows, Arch::Aarch64).is_some();
    assert!(err);
    let selected = select_platform_asset(&[], Os::Windows, Arch::Aarch64, None).unwrap();
    assert!(selected.is_none());
}

#[tokio::test]
async fn check_skips_release_with_unsafe_checksums_name() {
    let json = serde_json::json!([{
        "tag_name": "v1.1.0",
        "draft": false,
        "prerelease": false,
        "body": "notes",
        "assets": [
            {"name":"dblocal-linux-x86_64.tar.gz","browser_download_url":"https://github.com/a/b/releases/download/v1.1/a.tar.gz","size":1},
            {"name":"../checksums.txt","browser_download_url":"https://github.com/a/b/releases/download/v1.1/checksums.txt","size":1}
        ]
    }]).to_string();
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, json.into_bytes()))]);
    let result = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .expect("check");
    assert!(matches!(result, UpdateCheck::UpToDate { .. }));
}

#[tokio::test]
async fn check_uses_sanitised_notes_and_tag() {
    let json = sample_release_json("v1.1.0").replace("Hello", "Hi\\r\\nthere");
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, json.into_bytes()))]);
    let result = check(
        &Version::parse("1.0.0").unwrap(),
        &fetcher,
        &linux_options(),
    )
    .await
    .unwrap();
    match result {
        UpdateCheck::Available(u) => {
            assert_eq!(u.tag(), "v1.1.0");
            assert_eq!(u.release_notes(), "Hi there");
        }
        _ => panic!("expected available"),
    }
}

#[test]
fn parse_release_version_accepts_leading_v() {
    let v = parse_release_version("v1.2.3").unwrap();
    assert_eq!(v, Version::parse("1.2.3").unwrap());
}

#[tokio::test]
async fn reqwest_fetcher_builds_with_secure_client() {
    let fetcher = ReqwestUpdateFetcher::new();
    assert!(fetcher.is_ok(), "production fetcher must build");
}

#[test]
fn asset_from_gh_rejects_unsafe_name() {
    let asset = GhAsset {
        name: "../evil.tar.gz".to_owned(),
        browser_download_url: "https://github.com/x".to_owned(),
        size: 1,
    };
    assert!(asset_from_gh(&asset).is_err());
}

#[test]
fn should_replace_asset_prefers_higher_priority() {
    let low = ReleaseAsset::new(
        "a.tar.gz",
        "https://github.com/a",
        1,
        UpdateAssetKind::TarGz,
    );
    let high = ReleaseAsset::new(
        "a.AppImage",
        "https://github.com/a",
        1,
        UpdateAssetKind::AppImage,
    );
    assert!(should_replace_asset(None, &low));
    assert!(should_replace_asset(Some(&low), &high));
    assert!(!should_replace_asset(Some(&high), &low));
}
