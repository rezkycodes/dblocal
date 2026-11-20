//! Pre-write response validation.
//!
//! Two checks run before a single body byte reaches the `.part` file:
//!
//! * the response must carry no content encoding other than `identity`, because
//!   catalog digests are over the raw published bytes and this client has no
//!   decompression support (review round 1, C2);
//! * any declared length (`Content-Length`, or a `206`'s `Content-Range` total)
//!   must equal the catalog size (review round 1, C1.2).
//!
//! Both failures are fatal: a wrong size or a compressed body is a server-side
//! fact, not a transient fault, so retrying it would only repeat the mistake.

use std::path::Path;

use reqwest::StatusCode;
use reqwest::header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, HeaderMap};

use crate::DblError;

use super::session::{AttemptError, cleanup_part};

/// Rejects a response whose `Content-Encoding` is anything other than absent or
/// `identity`, before a single body byte is written.
pub(super) async fn ensure_identity_encoding(
    headers: &HeaderMap,
    part_path: &Path,
) -> Result<(), AttemptError> {
    let Some(value) = headers.get(CONTENT_ENCODING) else {
        return Ok(());
    };
    let Ok(encoding) = value.to_str() else {
        cleanup_part(part_path).await;
        return Err(AttemptError::fatal(DblError::Network {
            reason: "server sent a non-UTF-8 Content-Encoding header".to_owned(),
        }));
    };
    let encoding = encoding.trim();
    if encoding.is_empty() || encoding.eq_ignore_ascii_case("identity") {
        return Ok(());
    }
    cleanup_part(part_path).await;
    Err(AttemptError::fatal(DblError::Network {
        reason: format!(
            "server applied Content-Encoding '{encoding}'; artifacts must be fetched as identity \
             so the SHA-256 matches the catalog"
        ),
    }))
}

/// Rejects a response that declares a length contradicting `expected_size`,
/// before a single body byte is written.
///
/// A `206` states the full object size in `Content-Range` (`bytes a-b/total`), so
/// that total must equal the catalog size. A `200` carries the whole body, so its
/// `Content-Length` must equal the catalog size. A missing length is allowed: the
/// streamed guard in [`stream_response`] still bounds what is written.
pub(super) async fn ensure_declared_length(
    status: StatusCode,
    headers: &HeaderMap,
    expected_size: u64,
    part_path: &Path,
) -> Result<(), AttemptError> {
    let declared = if status == StatusCode::PARTIAL_CONTENT {
        content_range_total(headers)
    } else {
        declared_content_length(headers)
    };
    let Some(declared) = declared else {
        return Ok(());
    };
    if declared == expected_size {
        return Ok(());
    }
    cleanup_part(part_path).await;
    Err(AttemptError::fatal(declared_length_mismatch(
        declared,
        expected_size,
    )))
}

fn declared_content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
}

/// The error for a streamed body that runs past the catalog's declared size.
///
/// `received` is the count at the first chunk that crossed the limit, so the
/// message reads "at least" — the server may have had more still to send.
pub(super) fn body_exceeds_expected(received: u64, expected: u64) -> DblError {
    DblError::Network {
        reason: format!(
            "refusing oversized response body: the catalog expects {expected} bytes but the \
             server sent at least {received} bytes"
        ),
    }
}

/// The error for a response whose declared length (`Content-Length`, or a
/// `Content-Range` total) contradicts the catalog. Detected before any body byte
/// is written.
fn declared_length_mismatch(declared: u64, expected: u64) -> DblError {
    DblError::Network {
        reason: format!(
            "response declares {declared} bytes but the catalog expects {expected} bytes \
             (refusing to download a body of the wrong size)"
        ),
    }
}

/// The full object size from a `Content-Range` header (`bytes a-b/total`).
///
/// A server may answer `*` for the total when it does not know it; that yields
/// `None`, leaving the streamed guard to bound the body.
fn content_range_total(headers: &HeaderMap) -> Option<u64> {
    let v = headers.get(CONTENT_RANGE)?.to_str().ok()?;
    let v = v.trim();
    let v = v.strip_prefix("bytes ")?;
    let (_range, total) = v.split_once('/')?;
    total.trim().parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use reqwest::header::HeaderValue;
    use tempfile::tempdir;

    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_str(value).expect("header value"));
        }
        map
    }

    #[tokio::test]
    async fn absent_content_encoding_is_allowed() {
        let dir = tempdir().expect("tmp");
        let part = dir.path().join("x.part");
        tokio::fs::write(&part, b"partial").await.expect("seed");
        ensure_identity_encoding(&headers(&[]), &part)
            .await
            .expect("absent encoding is fine");
        assert!(part.exists(), "a valid response must not touch the .part");
    }

    #[tokio::test]
    async fn identity_content_encoding_is_allowed_case_insensitively() {
        let dir = tempdir().expect("tmp");
        let part = dir.path().join("x.part");
        tokio::fs::write(&part, b"partial").await.expect("seed");
        for value in ["identity", "IDENTITY", " Identity "] {
            ensure_identity_encoding(&headers(&[("content-encoding", value)]), &part)
                .await
                .unwrap_or_else(|e| panic!("{value} must be allowed: {e:?}"));
        }
        assert!(part.exists());
    }

    #[tokio::test]
    async fn any_other_content_encoding_is_fatal_and_deletes_the_part() {
        let dir = tempdir().expect("tmp");
        for value in ["gzip", "br", "zstd", "deflate"] {
            let part = dir.path().join("x.part");
            tokio::fs::write(&part, b"partial").await.expect("seed");
            let err = ensure_identity_encoding(&headers(&[("content-encoding", value)]), &part)
                .await
                .expect_err("must reject");
            assert!(!err.retryable, "{value} must be fatal");
            let message = err.error.to_string();
            assert!(message.contains(value), "must name {value}: {message}");
            assert!(
                message.contains("identity"),
                "must mention identity: {message}"
            );
            assert!(!part.exists(), "{value} must delete the .part");
        }
    }

    #[tokio::test]
    async fn declared_length_is_checked_for_a_200() {
        let dir = tempdir().expect("tmp");
        let part = dir.path().join("x.part");
        tokio::fs::write(&part, b"partial").await.expect("seed");

        ensure_declared_length(
            StatusCode::OK,
            &headers(&[("content-length", "100")]),
            100,
            &part,
        )
        .await
        .expect("matching length");
        assert!(part.exists(), "a matching length must not touch the .part");

        let err = ensure_declared_length(
            StatusCode::OK,
            &headers(&[("content-length", "101")]),
            100,
            &part,
        )
        .await
        .expect_err("mismatched length");
        assert!(!err.retryable, "a wrong declared length is fatal");
        let message = err.error.to_string();
        assert!(
            message.contains("101") && message.contains("100"),
            "{message}"
        );
        assert!(!part.exists(), "must delete the .part");
    }

    #[tokio::test]
    async fn declared_length_is_checked_for_a_206_via_content_range_total() {
        let dir = tempdir().expect("tmp");
        let part = dir.path().join("x.part");
        tokio::fs::write(&part, b"partial").await.expect("seed");

        // The bytes in this response are a slice; only the total matters.
        ensure_declared_length(
            StatusCode::PARTIAL_CONTENT,
            &headers(&[("content-range", "bytes 100-199/200")]),
            200,
            &part,
        )
        .await
        .expect("matching total");
        assert!(part.exists());

        let err = ensure_declared_length(
            StatusCode::PARTIAL_CONTENT,
            &headers(&[("content-range", "bytes 100-299/300")]),
            200,
            &part,
        )
        .await
        .expect_err("mismatched total");
        let message = err.error.to_string();
        assert!(
            message.contains("300") && message.contains("200"),
            "{message}"
        );
        assert!(!part.exists());
    }

    #[tokio::test]
    async fn an_unknown_total_is_left_to_the_streamed_guard() {
        let dir = tempdir().expect("tmp");
        let part = dir.path().join("x.part");
        tokio::fs::write(&part, b"partial").await.expect("seed");
        // `bytes 0-99/*` means the server does not know the total.
        ensure_declared_length(
            StatusCode::PARTIAL_CONTENT,
            &headers(&[("content-range", "bytes 0-99/*")]),
            200,
            &part,
        )
        .await
        .expect("unknown total is allowed");
        assert!(part.exists());
    }

    #[tokio::test]
    async fn absent_length_is_allowed() {
        let dir = tempdir().expect("tmp");
        let part = dir.path().join("x.part");
        ensure_declared_length(StatusCode::OK, &headers(&[]), 200, &part)
            .await
            .expect("chunked bodies have no length");
    }

    #[test]
    fn body_exceeds_expected_names_both_sizes() {
        let message = body_exceeds_expected(300, 200).to_string();
        assert!(message.contains("300"), "{message}");
        assert!(message.contains("200"), "{message}");
        assert!(message.contains("at least"), "{message}");
    }

    #[test]
    fn content_range_total_parses_the_common_shapes() {
        assert_eq!(
            content_range_total(&headers(&[("content-range", "bytes 0-9/10")])),
            Some(10)
        );
        assert_eq!(
            content_range_total(&headers(&[("content-range", "bytes 5-9/*")])),
            None
        );
        assert_eq!(content_range_total(&headers(&[])), None);
    }
}
