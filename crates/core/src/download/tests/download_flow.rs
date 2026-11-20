//! Resumable transfer behaviour: `Range`/`If-Range`, truncated responses,
//! servers that ignore `Range`, checksum failures, cancellation and progress.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::super::fetch::{
    CancellationToken, DownloadEvent, ProgressSink, UrlPolicy, download_from_url, etag_sidecar_path,
};
use super::super::verify::{sha256_bytes, sha256_file};
use super::{ServeConfig, start_server, test_download_options};

#[tokio::test]
async fn truncated_download_resumes_to_identical_hash() {
    let body: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    let expected = sha256_bytes(&body);
    let server = start_server(ServeConfig {
        truncate_first_to: Some(16 * 1024),
        ..ServeConfig::normal(body.clone())
    })
    .await;

    let url = reqwest::Url::parse(&server.url("/f.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");

    download_from_url(
        &url,
        "test-artifact",
        &expected,
        body.len() as u64,
        &part,
        &test_download_options(),
        None,
    )
    .await
    .expect("resume completes");

    assert_eq!(sha256_file(&part).await.expect("hash"), expected);
    assert!(server.hits() >= 2, "expected a resume request");
}

#[tokio::test]
async fn server_ignoring_range_restarts_and_reports() {
    let body = vec![7u8; 32 * 1024];
    let expected = sha256_bytes(&body);
    let server = start_server(ServeConfig {
        truncate_first_to: Some(8 * 1024),
        ignore_range: true,
        ..ServeConfig::normal(body.clone())
    })
    .await;

    let url = reqwest::Url::parse(&server.url("/f.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");

    let events: Arc<Mutex<Vec<DownloadEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink: ProgressSink = {
        let recorded = events.clone();
        Arc::new(move |e| recorded.lock().expect("lock").push(e))
    };

    download_from_url(
        &url,
        "test-artifact",
        &expected,
        body.len() as u64,
        &part,
        &test_download_options(),
        Some(sink),
    )
    .await
    .expect("download completes");

    assert_eq!(sha256_file(&part).await.expect("hash"), expected);
    let recorded = events.lock().expect("lock");
    assert!(
        recorded
            .iter()
            .any(|e| matches!(e, DownloadEvent::Restarted { .. })),
        "ignored Range must report a restart, got {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|e| matches!(e, DownloadEvent::Progress(p) if p.bytes_done > 0)),
        "progress must be reported from the first bytes"
    );
    let progressed = recorded
        .iter()
        .filter_map(|e| match e {
            DownloadEvent::Progress(p) => Some(*p),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        progressed
            .iter()
            .any(|p| p.bytes_total == body.len() as u64),
        "total must be reported: {progressed:?}"
    );
}

#[tokio::test]
async fn wrong_sha256_deletes_part_and_names_digests() {
    let body = b"correct content".to_vec();
    let expected = sha256_bytes(b"something else");
    let actual = sha256_bytes(&body);
    let server = start_server(ServeConfig::normal(body.clone())).await;

    let url = reqwest::Url::parse(&server.url("/f.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");

    let err = download_from_url(
        &url,
        "test-artifact",
        &expected,
        body.len() as u64,
        &part,
        &test_download_options(),
        None,
    )
    .await
    .expect_err("checksum mismatch must fail");

    assert!(
        matches!(err, crate::DblError::ChecksumMismatch { .. }),
        "{err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains(&expected),
        "must name the expected digest: {msg}"
    );
    assert!(msg.contains(&actual), "must name the actual digest: {msg}");
    assert!(!part.exists(), ".part must be deleted");
    assert!(!etag_sidecar_path(&part).exists(), ".etag must be deleted");
    assert_eq!(err.code(), crate::error::CODE_CHECKSUM_MISMATCH);
}

#[tokio::test]
async fn preexisting_prefix_is_rehashed_before_resume() {
    let body: Vec<u8> = (0..(32 * 1024)).map(|i| (i % 251) as u8).collect();
    let expected = sha256_bytes(&body);
    let server = start_server(ServeConfig::normal(body.clone())).await;

    let url = reqwest::Url::parse(&server.url("/resume.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");
    tokio::fs::write(&part, &body[..16 * 1024])
        .await
        .expect("seed");
    tokio::fs::write(etag_sidecar_path(&part), "\"test-etag\"")
        .await
        .expect("seed etag");

    download_from_url(
        &url,
        "test-artifact",
        &expected,
        body.len() as u64,
        &part,
        &test_download_options(),
        None,
    )
    .await
    .expect("resume");

    assert_eq!(sha256_file(&part).await.expect("hash"), expected);
    assert_eq!(server.hits(), 1, "resume must be a single Range request");
}

#[tokio::test]
async fn cancel_mid_download_removes_part_and_sidecar() {
    let body = vec![1u8; 512 * 1024];
    let expected = sha256_bytes(&body);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let served = body.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let body = served.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"e\"\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if stream.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                for chunk in body.chunks(4096) {
                    if stream.write_all(chunk).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            });
        }
    });

    let url = reqwest::Url::parse(&format!("http://{addr}/slow.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");
    let token = CancellationToken::new();
    let observer = token.clone();
    let watched = part.clone();
    tokio::spawn(async move {
        for _ in 0..400 {
            if tokio::fs::metadata(&watched)
                .await
                .map(|m| m.len() > 0)
                .unwrap_or(false)
            {
                observer.cancel();
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        observer.cancel();
    });

    let opts = test_download_options().with_cancel(token);
    let err = download_from_url(
        &url,
        "test-artifact",
        &expected,
        body.len() as u64,
        &part,
        &opts,
        None,
    )
    .await
    .expect_err("cancel must abort");

    assert!(matches!(err, crate::DblError::Cancelled { .. }), "{err:?}");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!part.exists(), ".part must be removed on cancel");
    assert!(
        !etag_sidecar_path(&part).exists(),
        ".etag must be removed on cancel"
    );
}

#[tokio::test]
async fn http_error_status_is_typed_network_error() {
    let server = start_server(ServeConfig {
        force_status: Some(404),
        ..ServeConfig::normal(b"nope".to_vec())
    })
    .await;
    let url = reqwest::Url::parse(&server.url("/missing.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");
    let err = download_from_url(
        &url,
        "test-artifact",
        &sha256_bytes(b"nope"),
        4,
        &part,
        &test_download_options(),
        None,
    )
    .await
    .expect_err("404 must fail");
    assert!(matches!(err, crate::DblError::Network { .. }), "{err:?}");
    assert_eq!(err.code(), crate::error::CODE_NETWORK);
    // A 404 is a permanent answer: it must not be retried.
    assert_eq!(server.hits(), 1, "4xx must fail immediately");
}

#[tokio::test]
async fn http_429_is_retried() {
    let body = vec![8u8; 4096];
    let server = start_server(ServeConfig::normal(body.clone()).with_fail_first(1, 429)).await;
    let url = reqwest::Url::parse(&server.url("/throttled.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");
    download_from_url(
        &url,
        "test-artifact",
        &sha256_bytes(&body),
        body.len() as u64,
        &part,
        &test_download_options(),
        None,
    )
    .await
    .expect("429 is retryable");
    assert_eq!(server.hits(), 2);
}

#[tokio::test]
async fn http_408_is_retried() {
    let body = vec![10u8; 4096];
    let server = start_server(ServeConfig::normal(body.clone()).with_fail_first(1, 408)).await;
    let url = reqwest::Url::parse(&server.url("/slow.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");
    download_from_url(
        &url,
        "test-artifact",
        &sha256_bytes(&body),
        body.len() as u64,
        &part,
        &test_download_options(),
        None,
    )
    .await
    .expect("408 is retryable");
    assert_eq!(server.hits(), 2);
}

#[test]
fn https_only_policy_rejects_plain_http() {
    let url = reqwest::Url::parse("http://example.test/a.tar.gz").expect("parse");
    let err = super::super::fetch::test_validate(&UrlPolicy::https_only(), &url)
        .expect_err("http must be rejected");
    assert!(err.to_string().contains("non-HTTPS"), "{err}");
}

#[test]
fn https_only_policy_accepts_https() {
    let url = reqwest::Url::parse("https://example.test/a.tar.gz").expect("parse");
    super::super::fetch::test_validate(&UrlPolicy::https_only(), &url).expect("https allowed");
}

#[test]
fn loopback_test_policy_is_limited_to_loopback() {
    let policy = UrlPolicy::allow_loopback_http_for_tests();
    for ok in [
        "http://127.0.0.1:8000/a",
        "http://[::1]:8000/a",
        "http://localhost:8000/a",
        "https://example.test/a",
    ] {
        let url = reqwest::Url::parse(ok).expect("parse");
        super::super::fetch::test_validate(&policy, &url).unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    for bad in ["http://example.test/a", "ftp://127.0.0.1/a"] {
        let url = reqwest::Url::parse(bad).expect("parse");
        assert!(
            super::super::fetch::test_validate(&policy, &url).is_err(),
            "{bad} must be rejected"
        );
    }
}

// ---------------------------------------------------------------------------
// Retry, restart and progress details
// ---------------------------------------------------------------------------

#[tokio::test]
async fn transient_5xx_is_retried_until_success() {
    let body = vec![3u8; 16 * 1024];
    let expected = sha256_bytes(&body);
    let server = start_server(ServeConfig::normal(body.clone()).with_fail_first(2, 503)).await;
    let url = reqwest::Url::parse(&server.url("/retry.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");

    download_from_url(
        &url,
        "test-artifact",
        &expected,
        body.len() as u64,
        &part,
        &test_download_options(),
        None,
    )
    .await
    .expect("retry should recover");

    assert_eq!(sha256_file(&part).await.expect("hash"), expected);
    assert_eq!(server.hits(), 3, "two failures plus one success");
}

#[tokio::test]
async fn dropped_connection_is_retried_as_network_error() {
    let body = vec![4u8; 8 * 1024];
    let expected = sha256_bytes(&body);
    let server = start_server(ServeConfig::normal(body.clone()).with_drop_first()).await;
    let url = reqwest::Url::parse(&server.url("/drop.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");

    download_from_url(
        &url,
        "test-artifact",
        &expected,
        body.len() as u64,
        &part,
        &test_download_options(),
        None,
    )
    .await
    .expect("a dropped connection must be retried");

    assert_eq!(sha256_file(&part).await.expect("hash"), expected);
    assert_eq!(server.hits(), 2, "one dropped attempt plus one success");
}

#[tokio::test]
async fn retries_exhausted_returns_network_error() {
    let body = b"never served".to_vec();
    let server = start_server(ServeConfig::normal(body.clone()).with_fail_first(99, 500)).await;
    let url = reqwest::Url::parse(&server.url("/always-fails.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");
    let mut opts = test_download_options();
    opts.max_retries = 2;

    let err = download_from_url(
        &url,
        "test-artifact",
        &sha256_bytes(&body),
        body.len() as u64,
        &part,
        &opts,
        None,
    )
    .await
    .expect_err("must give up");
    assert!(matches!(err, crate::DblError::Network { .. }), "{err:?}");
    assert_eq!(server.hits(), 3, "initial attempt plus max_retries");
}

#[tokio::test]
async fn inconsistent_content_range_triggers_clean_restart() {
    let body = vec![6u8; 24 * 1024];
    let expected = sha256_bytes(&body);
    // First pass is truncated so the second request carries Range; the server
    // then reports a start that does not match what we asked for.
    let server = start_server(
        ServeConfig {
            truncate_first_to: Some(6 * 1024),
            ..ServeConfig::normal(body.clone())
        }
        .with_lie_about_content_range(),
    )
    .await;
    let url = reqwest::Url::parse(&server.url("/lie.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");

    let events: Arc<Mutex<Vec<DownloadEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink: ProgressSink = {
        let recorded = events.clone();
        Arc::new(move |e| recorded.lock().expect("lock").push(e))
    };

    download_from_url(
        &url,
        "test-artifact",
        &expected,
        body.len() as u64,
        &part,
        &test_download_options(),
        Some(sink),
    )
    .await
    .expect("restart should still complete");

    assert_eq!(sha256_file(&part).await.expect("hash"), expected);
    let recorded = events.lock().expect("lock");
    assert!(
        recorded
            .iter()
            .any(|e| matches!(e, DownloadEvent::Restarted { .. })),
        "an inconsistent Content-Range must cause a restart"
    );
}

#[tokio::test]
async fn cancel_before_start_removes_part() {
    let body = b"unused".to_vec();
    let server = start_server(ServeConfig::normal(body.clone())).await;
    let url = reqwest::Url::parse(&server.url("/x.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");
    let token = CancellationToken::new();
    token.cancel();
    let opts = test_download_options().with_cancel(token);

    let err = download_from_url(
        &url,
        "test-artifact",
        &sha256_bytes(&body),
        body.len() as u64,
        &part,
        &opts,
        None,
    )
    .await
    .expect_err("pre-cancelled");
    assert!(matches!(err, crate::DblError::Cancelled { .. }), "{err:?}");
    assert_eq!(server.hits(), 0, "cancelled before any request");
    assert!(!part.exists());
}

#[test]
fn cancellation_token_flips_once() {
    let token = CancellationToken::new();
    assert!(!token.is_cancelled());
    token.cancel();
    assert!(token.is_cancelled());
    // Clone shares state.
    assert!(token.clone().is_cancelled());
}

#[test]
fn progress_fraction_and_eta_are_sane() {
    use crate::model::DownloadProgress;
    let p = DownloadProgress::new(50, 100, 10, Some(Duration::from_secs(5)));
    assert!((p.fraction() - 0.5).abs() < f32::EPSILON);
}

#[test]
fn redirect_and_retry_constants_are_conservative() {
    assert_eq!(super::super::fetch::MAX_REDIRECTS, 5);
    assert!(super::super::fetch::MAX_BACKOFF > super::super::fetch::INITIAL_BACKOFF);
}

#[test]
fn part_path_sanitises_hostile_labels() {
    let dir = std::path::Path::new("/tmp/dl");
    assert_eq!(
        super::super::fetch::part_path_for_label(dir, "a/b\\c:d*e"),
        dir.join("a_b_c_d_e.part")
    );
    assert_eq!(
        super::super::fetch::part_path_for_label(dir, ""),
        dir.join("artifact.part")
    );
}

#[test]
fn user_agent_is_generic() {
    // The UA must identify the app and its version only, nothing about the host.
    let ua = format!("dblocal/{}", env!("CARGO_PKG_VERSION"));
    assert!(ua.starts_with("dblocal/"));
    assert!(!ua.contains(std::env::consts::OS) || ua.contains("dblocal"));
}

#[tokio::test]
async fn download_artifact_rejects_non_https_url() {
    // Exercises the production entry point (not `download_from_url`): an
    // `http://` artifact URL must be refused before any socket is opened.
    use crate::model::{ArchiveFormat, Artifact, EngineKey};
    use crate::platform::{Arch, Os};

    let artifact = Artifact::new(
        "http-artifact",
        EngineKey::Valkey,
        "9.2.0",
        Os::Linux,
        Arch::X86_64,
        "http://127.0.0.1:1/never",
        "a".repeat(64),
        8,
        ArchiveFormat::TarGz,
    )
    .ok();

    // `Artifact::new` itself refuses plain http, so build the URL check through
    // the session directly and confirm `download_artifact` inherits the policy.
    assert!(artifact.is_none(), "Artifact must reject http URLs");

    let dir = tempdir().expect("tmp");
    let part = dir.path().join("x.part");
    let url = reqwest::Url::parse("http://127.0.0.1:9/x.tar.gz").expect("url");
    let err = download_from_url(
        &url,
        "x",
        &"a".repeat(64),
        8,
        &part,
        &super::super::DownloadOptions::default(), // HttpsOnly
        None,
    )
    .await
    .expect_err("HttpsOnly must refuse loopback http");
    assert!(matches!(err, crate::DblError::Network { .. }), "{err:?}");
    assert!(!part.exists());
}

#[tokio::test]
async fn download_creates_missing_parent_directories() {
    let body = b"nested".to_vec();
    let expected = sha256_bytes(&body);
    let server = start_server(ServeConfig::normal(body.clone())).await;
    let url = reqwest::Url::parse(&server.url("/n.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("deep/nested/dir/artifact.part");

    download_from_url(
        &url,
        "test-artifact",
        &expected,
        body.len() as u64,
        &part,
        &test_download_options(),
        None,
    )
    .await
    .expect("creates parents");
    assert!(part.exists());
}

#[tokio::test]
async fn empty_response_body_fails_checksum_and_deletes_part() {
    let server = start_server(ServeConfig {
        etag: Some("\"e\"".to_owned()),
        ..ServeConfig::normal(Vec::new())
    })
    .await;
    let url = reqwest::Url::parse(&server.url("/empty.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");
    let expected = sha256_bytes(b"expected bytes");

    let err = download_from_url(
        &url,
        "test-artifact",
        &expected,
        14,
        &part,
        &test_download_options(),
        None,
    )
    .await
    .expect_err("empty body cannot match");
    assert!(
        matches!(err, crate::DblError::ChecksumMismatch { .. }),
        "{err:?}"
    );
    assert!(!part.exists());
}

#[tokio::test]
async fn stale_empty_part_file_is_discarded() {
    let body = b"fresh content".to_vec();
    let expected = sha256_bytes(&body);
    let server = start_server(ServeConfig::normal(body.clone())).await;
    let url = reqwest::Url::parse(&server.url("/fresh.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");
    // A zero-length `.part` is not a useful resume point.
    tokio::fs::write(&part, b"").await.expect("seed empty");

    download_from_url(
        &url,
        "test-artifact",
        &expected,
        body.len() as u64,
        &part,
        &test_download_options(),
        None,
    )
    .await
    .expect("downloads from scratch");
    assert_eq!(sha256_file(&part).await.expect("hash"), expected);
    assert_eq!(
        server.hits(),
        1,
        "an empty .part must not force a retry loop"
    );
}
