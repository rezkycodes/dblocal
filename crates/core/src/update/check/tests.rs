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
    UpdateCheckOptions::for_repo("dblocal-org/dblocal").with_platform(Os::Linux, Arch::X86_64)
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
    let selected = select_platform_asset(&assets, Os::Linux, Arch::X86_64)
        .unwrap()
        .unwrap();
    assert_eq!(selected.kind(), UpdateAssetKind::AppImage);
}

#[tokio::test]
async fn check_reports_non_200_status() {
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(500, b"oops".to_vec()))]);
    let err = check(&Version::parse("1.0.0").unwrap(), &fetcher, &linux_options())
        .await
        .expect_err("500");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
}

#[tokio::test]
async fn check_reports_oversized_body() {
    let body = vec![b'a'; MAX_RELEASES_BODY_BYTES + 1];
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(200, body))]);
    let err = check(&Version::parse("1.0.0").unwrap(), &fetcher, &linux_options())
        .await
        .expect_err("oversized");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_MALFORMED);
}

#[tokio::test]
async fn check_reports_429_rate_limit() {
    let fetcher = MockFetcher::new(vec![Ok(FetchResponse::new(429, b"rate".to_vec()))]);
    let err = check(&Version::parse("1.0.0").unwrap(), &fetcher, &linux_options())
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
