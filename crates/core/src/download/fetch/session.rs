//! Transfer session: range-aware requests, streaming, retries, verification.

use std::path::Path;
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, ETAG, HeaderMap, IF_RANGE, RANGE};
use tokio::io::AsyncWriteExt;

use crate::model::Artifact;
use crate::model::DownloadProgress;
use crate::{DblError, Result};

use super::super::verify::{StreamingHasher, hash_prefix, verify_or_reject};
use super::*;

/// Downloads `artifact` to `part_path`, resuming when possible, verifying the
/// SHA-256 against `artifact.sha256()` before returning.
///
/// On checksum mismatch the `.part` is deleted and [`DblError::ChecksumMismatch`]
/// is returned. On cancellation the `.part` is deleted and
/// [`DblError::Cancelled`] is returned.
pub(super) async fn download_artifact_impl(
    artifact: &Artifact,
    part_path: &Path,
    options: &DownloadOptions,
    progress: Option<ProgressSink>,
) -> Result<()> {
    let url = reqwest::Url::parse(artifact.url()).map_err(|e| DblError::InvalidInput {
        field: "url",
        reason: format!("invalid URL: {e}"),
    })?;
    download_from_url(
        &url,
        artifact.id(),
        artifact.sha256(),
        artifact.size_bytes(),
        part_path,
        options,
        progress,
    )
    .await
}

/// Core download routine, parameterised by everything it needs rather than by
/// an [`Artifact`] so it can be exercised over loopback HTTP in tests while
/// [`download_artifact`] remains the HTTPS-only production entry point.
pub(super) async fn download_from_url_impl(
    url: &reqwest::Url,
    label: &str,
    expected_sha256: &str,
    expected_size: u64,
    part_path: &Path,
    options: &DownloadOptions,
    progress: Option<ProgressSink>,
) -> Result<()> {
    if let Some(parent) = part_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| DblError::Io { source })?;
    }

    options.policy.validate_url(url)?;

    let transfer = Transfer {
        url,
        label,
        expected_sha256,
        expected_size,
    };

    let client = build_client(&options.policy)?;

    let mut attempt: u32 = 0;
    loop {
        // Cancellation is checked outside the attempt so the error is never
        // wrapped in a retry decision.
        check_cancel(&options.cancel, part_path).await?;
        let outcome = download_once(&client, &transfer, part_path, options, &progress).await;
        match outcome {
            Ok(()) => return Ok(()),
            Err(AttemptError { error, retryable }) => {
                if matches!(error, DblError::Cancelled { .. }) {
                    cleanup_part(part_path).await;
                    return Err(error);
                }
                if retryable && attempt < options.max_retries {
                    attempt += 1;
                    let delay = backoff_delay(options, attempt);
                    tracing::warn!(
                        attempt,
                        max = options.max_retries,
                        delay_ms = delay.as_millis() as u64,
                        error = %error,
                        "download attempt failed, retrying"
                    );
                    sleep_or_cancel(&options.cancel, delay, part_path).await?;
                    // loop: download_once resumes from whatever `.part` we have
                } else {
                    return Err(error);
                }
            }
        }
    }
}

fn build_client(policy: &UrlPolicy) -> Result<reqwest::Client> {
    let ua = format!("dblocal/{}", env!("CARGO_PKG_VERSION"));
    let builder = reqwest::Client::builder()
        .user_agent(ua)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .https_only(matches!(policy, UrlPolicy::HttpsOnly))
        .redirect(reqwest::redirect::Policy::custom({
            let policy = policy.clone();
            move |attempt| redirect_action(&policy, attempt)
        }));
    // Ensure no system proxy surprises for loopback tests; production can still
    // honour env proxies via defaults if desired, but we keep behaviour explicit.
    // (default reqwest enables system proxy; leave as-is for production.)
    let client = builder.build().map_err(|e| DblError::Network {
        reason: format!("failed to build HTTP client: {e}"),
    })?;
    Ok(client)
}

fn redirect_action(
    policy: &UrlPolicy,
    attempt: reqwest::redirect::Attempt<'_>,
) -> reqwest::redirect::Action {
    if attempt.previous().len() >= MAX_REDIRECTS {
        return attempt.error(format!("too many redirects (max {MAX_REDIRECTS})"));
    }
    let url = attempt.url().clone();
    match policy.validate_url(&url) {
        Ok(()) => attempt.follow(),
        Err(e) => attempt.error(e.to_string()),
    }
}

/// Everything a single transfer needs, independent of [`Artifact`].
struct Transfer<'a> {
    url: &'a reqwest::Url,
    label: &'a str,
    expected_sha256: &'a str,
    expected_size: u64,
}

async fn download_once(
    client: &reqwest::Client,
    transfer: &Transfer<'_>,
    part_path: &Path,
    options: &DownloadOptions,
    progress: &Option<ProgressSink>,
) -> Result<(), AttemptError> {
    // Determine resume state.
    let (mut offset, mut hasher, stored_etag) =
        resume_state(part_path).await.map_err(AttemptError::fatal)?;

    let mut req = client.get(transfer.url.clone());
    if offset > 0 {
        req = req.header(RANGE, format!("bytes={offset}-"));
        if let Some(ref etag) = stored_etag {
            req = req.header(IF_RANGE, etag.as_str());
        }
    }

    let resp = send_request(req).await?;
    let status = resp.status();

    // Handle validator mismatch / range not satisfiable in a bad way: restart.
    if offset > 0 && status == StatusCode::PRECONDITION_FAILED {
        emit_restart(progress, "resource changed (If-Range precondition failed)");
        (offset, hasher) = restart_clean(part_path)
            .await
            .map_err(AttemptError::fatal)?;
        return download_fresh(
            client, transfer, part_path, options, progress, offset, hasher,
        )
        .await;
    }
    if offset > 0 && status == StatusCode::RANGE_NOT_SATISFIABLE {
        // Could mean our offset == total (already complete) or server confused.
        // Try a fresh probe: if total matches and hash matches, accept; else restart.
        // Safest: restart cleanly.
        emit_restart(progress, "server returned 416 Range Not Satisfiable");
        (offset, hasher) = restart_clean(part_path)
            .await
            .map_err(AttemptError::fatal)?;
        return download_fresh(
            client, transfer, part_path, options, progress, offset, hasher,
        )
        .await;
    }

    if status == StatusCode::PARTIAL_CONTENT && offset > 0 {
        // Validate the range starts where we asked; if not, restart.
        if let Some(start) = content_range_start(resp.headers())
            && start != offset
        {
            emit_restart(
                progress,
                "server returned an unexpected Content-Range start",
            );
            (offset, hasher) = restart_clean(part_path)
                .await
                .map_err(AttemptError::fatal)?;
            return download_fresh(
                client, transfer, part_path, options, progress, offset, hasher,
            )
            .await;
        }
        // Persist ETag from this response if present (helps subsequent resumes).
        save_etag_from_response(resp.headers(), part_path).await;
        return stream_response(
            resp,
            StreamPass {
                transfer,
                part_path,
                options,
                progress,
                offset,
                hasher,
                appending: true,
            },
        )
        .await;
    }

    if status.is_success() {
        if offset > 0 {
            // Server ignored Range and sent the full body (200). Restart cleanly.
            emit_restart(progress, "server ignored Range; restarting from beginning");
            (offset, hasher) = restart_clean(part_path)
                .await
                .map_err(AttemptError::fatal)?;
        }
        save_etag_from_response(resp.headers(), part_path).await;
        return stream_response(
            resp,
            StreamPass {
                transfer,
                part_path,
                options,
                progress,
                offset,
                hasher,
                appending: false,
            },
        )
        .await;
    }

    Err(status_attempt_error(status, transfer.url))
}

async fn download_fresh(
    client: &reqwest::Client,
    transfer: &Transfer<'_>,
    part_path: &Path,
    options: &DownloadOptions,
    progress: &Option<ProgressSink>,
    offset: u64,
    hasher: StreamingHasher,
) -> Result<(), AttemptError> {
    // Re-issue a plain GET after a clean restart.
    let req = client.get(transfer.url.clone());
    let resp = send_request(req).await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(status_attempt_error(status, transfer.url));
    }
    save_etag_from_response(resp.headers(), part_path).await;
    stream_response(
        resp,
        StreamPass {
            transfer,
            part_path,
            options,
            progress,
            offset,
            hasher,
            appending: false,
        },
    )
    .await
}

async fn send_request(builder: reqwest::RequestBuilder) -> Result<reqwest::Response, AttemptError> {
    builder
        .send()
        .await
        .map_err(|e| AttemptError::retryable(map_reqwest_error(e)))
}

/// Inputs for one streaming pass over a response body.
struct StreamPass<'a> {
    transfer: &'a Transfer<'a>,
    part_path: &'a Path,
    options: &'a DownloadOptions,
    progress: &'a Option<ProgressSink>,
    offset: u64,
    hasher: StreamingHasher,
    appending: bool,
}

async fn stream_response(
    mut resp: reqwest::Response,
    pass: StreamPass<'_>,
) -> Result<(), AttemptError> {
    let StreamPass {
        transfer,
        part_path,
        options,
        progress,
        offset,
        mut hasher,
        appending,
    } = pass;
    // Determine total size for progress reporting.
    let remaining = response_content_length(&resp);
    let total = if appending {
        if let Some(rem) = remaining {
            offset.saturating_add(rem)
        } else {
            transfer.expected_size
        }
    } else {
        remaining.unwrap_or(transfer.expected_size)
    };

    let mut file = open_part_for_write(part_path, appending)
        .await
        .map_err(AttemptError::fatal)?;
    let mut downloaded: u64 = offset;
    let started = Instant::now();
    let mut last_report = Instant::now();
    let mut bytes_since_report: u64 = 0;
    let mut last_speed: u64 = 0;

    // Emit an initial sample from the first byte / start of this attempt.
    emit_progress(
        progress,
        downloaded,
        total,
        0,
        &started,
        downloaded.saturating_sub(offset),
    );

    loop {
        check_cancel(&options.cancel, part_path)
            .await
            .map_err(AttemptError::fatal)?;
        let chunk = match resp.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => return Err(AttemptError::retryable(map_reqwest_error(e))),
        };
        if chunk.is_empty() {
            continue;
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|source| AttemptError::retryable(DblError::Io { source }))?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        bytes_since_report = bytes_since_report.saturating_add(chunk.len() as u64);

        let now = Instant::now();
        if now.duration_since(last_report) >= Duration::from_millis(200) || downloaded == total {
            let elapsed = now.duration_since(last_report).as_secs_f64();
            if elapsed > 0.0 {
                last_speed = (bytes_since_report as f64 / elapsed) as u64;
            }
            last_report = now;
            bytes_since_report = 0;
            emit_progress(
                progress,
                downloaded,
                total,
                last_speed,
                &started,
                downloaded.saturating_sub(offset),
            );
        }
    }

    file.flush()
        .await
        .map_err(|source| AttemptError::retryable(DblError::Io { source }))?;
    file.sync_all()
        .await
        .map_err(|source| AttemptError::retryable(DblError::Io { source }))?;
    drop(file);

    // Final progress sample.
    emit_progress(
        progress,
        downloaded,
        total,
        last_speed,
        &started,
        downloaded.saturating_sub(offset),
    );

    let actual = hasher.finalize_hex();
    verify_or_reject(transfer.label, transfer.expected_sha256, &actual, part_path)
        .await
        .map_err(AttemptError::fatal)
}

async fn open_part_for_write(part_path: &Path, append: bool) -> Result<tokio::fs::File> {
    let mut opts = tokio::fs::OpenOptions::new();
    opts.create(true).write(true);
    if append {
        opts.append(true);
    } else {
        opts.truncate(true);
    }
    opts.open(part_path)
        .await
        .map_err(|source| DblError::Io { source })
}

async fn resume_state(part_path: &Path) -> Result<(u64, StreamingHasher, Option<String>)> {
    match tokio::fs::metadata(part_path).await {
        Ok(meta) if meta.is_file() && meta.len() > 0 => {
            let len = meta.len();
            let hasher = hash_prefix(part_path, len).await?;
            let etag = read_etag_sidecar(part_path).await;
            Ok((len, hasher, etag))
        }
        Ok(_) => {
            // Empty or odd; start fresh.
            let _ = tokio::fs::remove_file(part_path).await;
            Ok((0, StreamingHasher::new(), None))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((0, StreamingHasher::new(), None)),
        Err(source) => Err(DblError::Io { source }),
    }
}

async fn restart_clean(part_path: &Path) -> Result<(u64, StreamingHasher)> {
    cleanup_part(part_path).await;
    Ok((0, StreamingHasher::new()))
}

async fn cleanup_part(part_path: &Path) {
    let _ = tokio::fs::remove_file(part_path).await;
    let _ = tokio::fs::remove_file(etag_sidecar_path(part_path)).await;
}

async fn read_etag_sidecar(part_path: &Path) -> Option<String> {
    let side = etag_sidecar_path(part_path);
    let bytes = tokio::fs::read(&side).await.ok()?;
    let s = String::from_utf8(bytes).ok()?;
    let trimmed = s.trim().to_owned();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

async fn save_etag_from_response(headers: &HeaderMap, part_path: &Path) {
    if let Some(val) = headers.get(ETAG)
        && let Ok(s) = val.to_str()
    {
        let side = etag_sidecar_path(part_path);
        // Best-effort; failure to persist ETag just means the next resume may
        // fall back to a clean restart.
        let _ = tokio::fs::write(side, s.as_bytes()).await;
    }
}

fn response_content_length(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| resp.content_length())
}

fn content_range_start(headers: &HeaderMap) -> Option<u64> {
    // Content-Range: bytes <start>-<end>/<total>
    let v = headers.get(CONTENT_RANGE)?.to_str().ok()?;
    let v = v.trim();
    let v = v.strip_prefix("bytes ")?;
    let (range, _total) = v.split_once('/')?;
    let (start, _end) = range.split_once('-')?;
    start.trim().parse::<u64>().ok()
}

fn emit_progress(
    sink: &Option<ProgressSink>,
    bytes_done: u64,
    bytes_total: u64,
    bytes_per_sec: u64,
    started: &Instant,
    bytes_this_attempt: u64,
) {
    let Some(sink) = sink else { return };
    let eta = if bytes_per_sec > 0 && bytes_total > bytes_done {
        let remaining = bytes_total - bytes_done;
        Some(Duration::from_secs_f64(
            remaining as f64 / bytes_per_sec as f64,
        ))
    } else if bytes_this_attempt > 0 {
        let elapsed = started.elapsed().as_secs_f64();
        if elapsed > 0.5 {
            let rate = bytes_this_attempt as f64 / elapsed;
            if rate > 0.0 && bytes_total > bytes_done {
                Some(Duration::from_secs_f64(
                    (bytes_total - bytes_done) as f64 / rate,
                ))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    sink(DownloadEvent::Progress(DownloadProgress::new(
        bytes_done,
        bytes_total,
        bytes_per_sec,
        eta,
    )));
}

fn emit_restart(sink: &Option<ProgressSink>, reason: &str) {
    tracing::info!(%reason, "restarting download from beginning");
    if let Some(sink) = sink {
        sink(DownloadEvent::Restarted {
            reason: reason.to_owned(),
        });
    }
}

async fn check_cancel(token: &CancellationToken, part_path: &Path) -> Result<()> {
    if token.is_cancelled() {
        cleanup_part(part_path).await;
        return Err(DblError::Cancelled {
            operation: "download".to_owned(),
        });
    }
    Ok(())
}

async fn sleep_or_cancel(
    token: &CancellationToken,
    delay: Duration,
    part_path: &Path,
) -> Result<()> {
    // Sleep in small increments so cancellation is responsive.
    let step = Duration::from_millis(100);
    let mut remaining = delay;
    while remaining > Duration::ZERO {
        check_cancel(token, part_path).await?;
        let chunk = if remaining < step { remaining } else { step };
        tokio::time::sleep(chunk).await;
        remaining = remaining.saturating_sub(chunk);
    }
    Ok(())
}

fn backoff_delay(options: &DownloadOptions, attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(16);
    let factor = 1u32 << shift;
    let d = options.initial_backoff.saturating_mul(factor);
    if d > options.max_backoff {
        options.max_backoff
    } else {
        d
    }
}

/// A single transfer attempt's outcome, with the retry decision already made.
///
/// Transport failures (connect, timeout, reset, unexpected EOF) are retryable;
/// protocol failures (an unexpected HTTP status, a checksum mismatch, a
/// cancellation) are not. Without this distinction a 404 would be retried five
/// times and a bad digest would be retried instead of reported.
struct AttemptError {
    error: DblError,
    retryable: bool,
}

impl AttemptError {
    fn retryable(error: DblError) -> Self {
        Self {
            error,
            retryable: true,
        }
    }

    fn fatal(error: DblError) -> Self {
        Self {
            error,
            retryable: false,
        }
    }
}

fn map_reqwest_error(e: reqwest::Error) -> DblError {
    let reason = if e.is_timeout() {
        format!("request timed out: {e}")
    } else if e.is_connect() {
        format!("connection failed: {e}")
    } else if e.is_redirect() {
        format!("redirect error: {e}")
    } else if e.is_body() {
        format!("response body error: {e}")
    } else if e.is_decode() {
        format!("response decode error: {e}")
    } else {
        format!("HTTP error: {e}")
    };
    DblError::Network { reason }
}

/// Maps an unexpected HTTP status to an attempt error.
///
/// Server-side and throttling failures (5xx, 408, 429) are worth retrying; every
/// other 4xx is a permanent answer and must be reported immediately, not hammered
/// five times.
fn status_attempt_error(status: StatusCode, url: &reqwest::Url) -> AttemptError {
    let error = DblError::Network {
        reason: format!("unexpected HTTP status {status} from {}", redact_url(url)),
    };
    let retryable = status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS;
    AttemptError { error, retryable }
}
