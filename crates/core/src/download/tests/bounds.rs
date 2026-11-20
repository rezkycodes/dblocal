//! Body bounds and content encoding (review round 1: C1 and C2).
//!
//! These guard two ways a download can quietly do the wrong thing:
//!
//! * **C1 — unbounded body.** The declared size must bound what we write. A
//!   server (or CDN, or middlebox) that streams more than the catalog says must
//!   be stopped and rejected, not silently written to disk until it fills it.
//! * **C2 — content encoding.** Catalog digests are over the raw published
//!   bytes. We must ask for `identity` and refuse anything else rather than
//!   hash a compressed body and report a confusing checksum mismatch.

use tempfile::tempdir;

use super::super::DownloadOptions;
use super::super::fetch::{download_from_url, etag_sidecar_path};
use super::super::verify::{sha256_bytes, sha256_file};
use super::{ServeConfig, start_server, test_download_options};

/// Runs a download of `body` (expected hash/size from the caller) against the
/// given server and returns the `.part` path, its directory, and the result.
async fn download(
    server: &super::TestServer,
    path: &str,
    expected: &str,
    expected_size: u64,
) -> (std::path::PathBuf, tempfile::TempDir, crate::Result<()>) {
    let url = reqwest::Url::parse(&server.url(path)).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");
    let result = download_from_url(
        &url,
        "test-artifact",
        expected,
        expected_size,
        &part,
        &test_download_options(),
        None,
    )
    .await;
    (part, dir, result)
}

/// Like [`download`] but with a caller-supplied `.part` already in place, so the
/// very first request carries a `Range` header.
async fn download_resuming(
    server: &super::TestServer,
    path: &str,
    expected: &str,
    expected_size: u64,
    prefix: &[u8],
) -> (std::path::PathBuf, tempfile::TempDir, crate::Result<()>) {
    let url = reqwest::Url::parse(&server.url(path)).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");
    tokio::fs::write(&part, prefix).await.expect("seed prefix");
    tokio::fs::write(etag_sidecar_path(&part), "\"test-etag\"")
        .await
        .expect("seed etag");
    let result = download_from_url(
        &url,
        "test-artifact",
        expected,
        expected_size,
        &part,
        &test_download_options(),
        None,
    )
    .await;
    (part, dir, result)
}

fn body_of(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

// ---------------------------------------------------------------------------
// C1.1 — an overlong body is stopped, the part is deleted, and it is fatal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn overlong_body_is_fatal_and_deletes_part() {
    let body = body_of(16 * 1024);
    let expected = sha256_bytes(&body);
    // Chunked framing with no Content-Length, plus 8 KiB the catalog never
    // promised: only the streamed size guard can catch this one.
    let server = start_server(
        ServeConfig::normal(body.clone())
            .with_extra_bytes(8 * 1024)
            .with_chunked(),
    )
    .await;
    let (part, _dir, result) =
        download(&server, "/over.tar.gz", &expected, body.len() as u64).await;

    let err = result.expect_err("an overlong body must be rejected");
    // A longer body is a server-side fact, not a transient fault: one attempt.
    assert_eq!(server.hits(), 1, "an overlong body must not be retried");
    let message = err.to_string();
    assert!(
        message.contains(&body.len().to_string()),
        "error must name the expected size: {message}"
    );
    assert!(
        message.contains("at least"),
        "error must say the body ran past the limit: {message}"
    );
    assert!(
        message.to_ascii_lowercase().contains("oversized")
            || message.to_ascii_lowercase().contains("refusing"),
        "error must state the body was refused: {message}"
    );
    assert!(!part.exists(), ".part must be removed on an overlong body");
    assert!(
        !etag_sidecar_path(&part).exists(),
        ".etag sidecar must be removed on an overlong body"
    );
}

#[tokio::test]
async fn overlong_body_writes_no_more_than_one_chunk_past_the_limit() {
    // The reader must stop at the first chunk that crosses the limit, so the
    // reported "at least N bytes" is at most expected + one chunk, never the
    // whole flood. A 256 KiB overrun proves the loop actually cuts off rather
    // than draining the server.
    let body = body_of(4 * 1024);
    let expected = sha256_bytes(&body);
    let server = start_server(
        ServeConfig::normal(body.clone())
            .with_extra_bytes(256 * 1024)
            .with_chunked(),
    )
    .await;
    let (part, _dir, result) =
        download(&server, "/flood.tar.gz", &expected, body.len() as u64).await;

    let err = result.expect_err("flood must fail");
    let message = err.to_string();
    let received = reported_received_bytes(&message).expect("error must state received bytes");
    let limit = body.len() as u64 + ONE_CHUNK_UPPER_BOUND;
    assert!(
        received <= limit,
        "must stop within one chunk past the limit: received {received}, limit {limit}"
    );
    assert!(
        received < body.len() as u64 + 256 * 1024,
        "must not drain the whole flood: received {received}"
    );
    assert!(!part.exists(), "flood must not leave a .part behind");
    assert!(
        tokio::fs::metadata(&part).await.is_err(),
        "the .part must be gone from disk before the error is reported"
    );
}

/// The chunk size `reqwest` delivers for a chunked body is bounded by its read
/// buffer; `ONE_CHUNK_UPPER_BOUND` is a generous ceiling used to prove the
/// streamed guard trips within one write rather than after the whole body.
const ONE_CHUNK_UPPER_BOUND: u64 = 64 * 1024;

/// Pulls the `N` out of "... sent at least N bytes".
fn reported_received_bytes(message: &str) -> Option<u64> {
    let after = message.split("at least ").nth(1)?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

#[tokio::test]
async fn overlong_body_on_a_resumed_transfer_is_fatal() {
    let body = body_of(32 * 1024);
    let expected = sha256_bytes(&body);
    let prefix = &body[..8 * 1024];
    let server = start_server(
        ServeConfig::normal(body.clone())
            .with_extra_bytes(4 * 1024)
            .with_chunked(),
    )
    .await;
    let (part, _dir, result) = download_resuming(
        &server,
        "/over-resume.tar.gz",
        &expected,
        body.len() as u64,
        prefix,
    )
    .await;

    let err = result.expect_err("overlong resumed body must be rejected");
    assert_eq!(server.hits(), 1, "fatal, not retried");
    let message = err.to_string();
    assert!(
        message.contains("at least"),
        "error must say the body ran past the limit: {message}"
    );
    assert!(
        message.contains(&body.len().to_string()),
        "error must name the expected size: {message}"
    );
    assert!(!part.exists());
}

// ---------------------------------------------------------------------------
// C1.2 — a contradicting Content-Length fails before writing a byte
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oversize_content_length_fails_before_writing() {
    let body = body_of(8 * 1024);
    let expected = sha256_bytes(&body);
    // The server promises 4 KiB more than the catalog's size.
    let server =
        start_server(ServeConfig::normal(body.clone()).with_advertised_extra_length(4 * 1024))
            .await;
    let (part, _dir, result) =
        download(&server, "/long-cl.tar.gz", &expected, body.len() as u64).await;

    let err = result.expect_err("an oversized Content-Length must be rejected");
    assert_eq!(server.hits(), 1, "fatal, not retried");
    let message = err.to_string();
    assert!(
        message.contains(&(body.len() as u64 + 4 * 1024).to_string()),
        "error must name the declared length: {message}"
    );
    assert!(
        message.contains(&body.len().to_string()),
        "error must name the expected size: {message}"
    );
    assert!(!part.exists(), "no bytes may be written and no .part left");
}

#[tokio::test]
async fn oversize_content_length_on_a_resumed_transfer_fails_before_writing() {
    let body = body_of(16 * 1024);
    let expected = sha256_bytes(&body);
    let prefix = &body[..4 * 1024];
    let server =
        start_server(ServeConfig::normal(body.clone()).with_advertised_extra_length(8 * 1024))
            .await;
    let (part, _dir, result) = download_resuming(
        &server,
        "/long-cl-resume.tar.gz",
        &expected,
        body.len() as u64,
        prefix,
    )
    .await;

    assert!(
        result.is_err(),
        "oversized resumed Content-Length must fail"
    );
    assert_eq!(server.hits(), 1, "fatal, not retried");
    assert!(!part.exists());
}

// ---------------------------------------------------------------------------
// C1.3 — a clean early EOF is a retryable truncation, not a checksum mismatch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn clean_early_eof_is_retried_as_truncation() {
    let body = body_of(24 * 1024);
    let expected = sha256_bytes(&body);
    // Every response is chunked and stops after 10 KiB: no transport error, just
    // a short body. The downloader must treat this as a truncation and keep
    // resuming; the server's Range support lets it finish.
    let server =
        start_server(ServeConfig::normal(body.clone()).with_clean_short_body_to(10 * 1024)).await;
    let url = reqwest::Url::parse(&server.url("/short.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");
    let mut options = test_download_options();
    options.max_retries = 6;

    let result = download_from_url(
        &url,
        "test-artifact",
        &expected,
        body.len() as u64,
        &part,
        &options,
        None,
    )
    .await;

    if result.is_ok() {
        assert_eq!(
            sha256_file(&part).await.expect("hash"),
            expected,
            "a resumed truncation must produce the identical hash"
        );
        assert!(
            server.hits() >= 2,
            "a truncation must be resumed, not accepted"
        );
    } else {
        // With a server that always stops at 10 KiB the single clean-EOF retry
        // cannot complete; what matters is that the failure is a truncation and
        // not a checksum mismatch.
        let err = result.expect_err("unreachable");
        assert!(
            !matches!(err, crate::DblError::ChecksumMismatch { .. }),
            "a short body is a truncation, not a checksum mismatch: {err:?}"
        );
    }
}

#[tokio::test]
async fn clean_early_eof_then_resume_reaches_identical_hash() {
    // Only the first response is short (a complete chunked body that stops
    // early); every later response is full. That is the "clean EOF then resume"
    // shape, and it must end with the identical hash.
    let body = body_of(20 * 1024);
    let expected = sha256_bytes(&body);
    let server = start_server(ServeConfig::normal(body.clone()).with_short_first(8 * 1024)).await;
    let url = reqwest::Url::parse(&server.url("/short-then-full.tar.gz")).expect("url");
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
    .expect("short then full must complete");
    assert_eq!(sha256_file(&part).await.expect("hash"), expected);
    assert_eq!(server.hits(), 2, "one short response then one resume");
}

#[tokio::test]
async fn complete_body_with_wrong_hash_is_still_a_fatal_mismatch() {
    // Regression guard: the truncation handling must not soften a genuine
    // mismatch. A body of exactly the expected size with the wrong digest is
    // fatal, deletes the `.part`, and names both digests.
    let body = body_of(8 * 1024);
    let wrong = sha256_bytes(b"something else entirely");
    let server = start_server(ServeConfig::normal(body.clone())).await;
    let (part, _dir, result) = download(&server, "/wrong.tar.gz", &wrong, body.len() as u64).await;

    let err = result.expect_err("wrong digest must fail");
    assert!(
        matches!(err, crate::DblError::ChecksumMismatch { .. }),
        "complete body with wrong hash is a mismatch: {err:?}"
    );
    assert_eq!(server.hits(), 1, "a mismatch is not retried");
    assert!(!part.exists(), "a mismatch deletes the .part");
}

// ---------------------------------------------------------------------------
// C2 — content encoding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn identity_request_gets_the_raw_body_and_verifies() {
    // The server gzips unless asked for identity. We ask for identity, so the
    // digest must match the raw bytes.
    let body = body_of(12 * 1024);
    let expected = sha256_bytes(&body);
    let server = start_server(ServeConfig::normal(body.clone()).with_gzip_unless_identity()).await;
    let (part, _dir, result) =
        download(&server, "/identity.tar.gz", &expected, body.len() as u64).await;

    result.expect("identity download must succeed");
    assert_eq!(sha256_file(&part).await.expect("hash"), expected);
}

#[tokio::test]
async fn identity_request_on_a_resumed_transfer_also_gets_raw_bytes() {
    let body = body_of(20 * 1024);
    let expected = sha256_bytes(&body);
    let prefix = &body[..8 * 1024];
    let server = start_server(ServeConfig::normal(body.clone()).with_gzip_unless_identity()).await;
    let (part, _dir, result) = download_resuming(
        &server,
        "/identity-resume.tar.gz",
        &expected,
        body.len() as u64,
        prefix,
    )
    .await;

    result.expect("identity resume must succeed");
    assert_eq!(sha256_file(&part).await.expect("hash"), expected);
}

#[tokio::test]
async fn gzip_content_encoding_is_rejected_before_writing() {
    // The server ignores our identity request and gzips anyway, marking it.
    let body = body_of(10 * 1024);
    let expected = sha256_bytes(&body);
    let server = start_server(ServeConfig::normal(body.clone()).with_always_gzip()).await;
    let (part, _dir, result) =
        download(&server, "/gzip.tar.gz", &expected, body.len() as u64).await;

    let err = result.expect_err("a content-encoded body must be rejected");
    assert_eq!(server.hits(), 1, "a content encoding is fatal, not retried");
    let message = err.to_string();
    assert!(
        message.to_ascii_lowercase().contains("content-encoding")
            || message.to_ascii_lowercase().contains("identity"),
        "error must explain the encoding requirement: {message}"
    );
    assert!(
        message.contains("gzip"),
        "error must name the encoding: {message}"
    );
    assert!(!part.exists(), "no bytes may be written and no .part left");
    assert!(!etag_sidecar_path(&part).exists());
}

#[tokio::test]
async fn gzip_content_encoding_on_a_resumed_transfer_is_rejected() {
    let body = body_of(20 * 1024);
    let expected = sha256_bytes(&body);
    let prefix = &body[..8 * 1024];
    let server = start_server(ServeConfig::normal(body.clone()).with_always_gzip()).await;
    let (part, _dir, result) = download_resuming(
        &server,
        "/gzip-resume.tar.gz",
        &expected,
        body.len() as u64,
        prefix,
    )
    .await;

    assert!(result.is_err(), "gzip resume must fail");
    assert_eq!(server.hits(), 1, "fatal, not retried");
    assert!(!part.exists());
}

#[tokio::test]
async fn explicit_identity_content_encoding_is_accepted() {
    // `Content-Encoding: identity` is equivalent to the header being absent: it
    // must verify, not be rejected as "some encoding".
    let body = body_of(4096);
    let expected = sha256_bytes(&body);
    let server =
        start_server(ServeConfig::normal(body.clone()).with_explicit_identity_encoding()).await;
    let (part, _dir, result) =
        download(&server, "/plain.tar.gz", &expected, body.len() as u64).await;
    result.expect("an explicit identity encoding must verify");
    assert_eq!(sha256_file(&part).await.expect("hash"), expected);
}

#[test]
fn downloads_do_not_enable_reqwest_decompression() {
    // C2.3: the `reqwest` dependency line in the workspace manifest must not
    // enable any decompression feature, otherwise the body we hash could be
    // transparently decoded and never match the catalog.
    let manifest = include_str!("../../../../../Cargo.toml");
    let reqwest_line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with("reqwest ="))
        .expect("workspace manifest must declare reqwest");
    for forbidden in ["gzip", "brotli", "zstd", "deflate"] {
        assert!(
            !reqwest_line.contains(forbidden),
            "reqwest must not enable {forbidden} decompression: {reqwest_line}"
        );
    }
    let _ = DownloadOptions::default();
}

#[tokio::test]
async fn stale_oversized_part_is_discarded_and_restarted() {
    // A `.part` larger than the catalog says can only be a stale run (the
    // catalog resized the artifact). It must be discarded and restarted, not
    // reported as an oversized body on a transfer we never began.
    let body = body_of(8 * 1024);
    let expected = sha256_bytes(&body);
    let server = start_server(ServeConfig::normal(body.clone())).await;
    let url = reqwest::Url::parse(&server.url("/stale.tar.gz")).expect("url");
    let dir = tempdir().expect("tmp");
    let part = dir.path().join("artifact.part");
    // Seed a `.part` bigger than body.len() with bytes that cannot match.
    tokio::fs::write(&part, vec![0xFFu8; body.len() + 4096])
        .await
        .expect("seed stale part");

    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink: super::super::ProgressSink = {
        let recorded = events.clone();
        std::sync::Arc::new(move |e| recorded.lock().expect("lock").push(e))
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
    .expect("a stale oversized .part must be discarded, not fatal");

    assert_eq!(sha256_file(&part).await.expect("hash"), expected);
    let recorded = events.lock().expect("lock");
    assert!(
        recorded
            .iter()
            .any(|e| matches!(e, super::super::DownloadEvent::Restarted { reason } if reason.contains("larger"))),
        "expected a restart because the .part was oversized: {recorded:?}"
    );
}
