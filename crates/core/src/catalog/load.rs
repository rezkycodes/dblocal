//! Loading the bundled snapshot and the cache, and refreshing silently.

use std::time::{Duration, SystemTime};

use crate::catalog::cache::{CachedDocument, catalog_cache_path, read_cache, write_cache};
use crate::catalog::dto::CatalogDocument;
use crate::catalog::fetch::{CatalogFetcher, RefreshOutcome};
use crate::catalog::validate::validate_document;
use crate::error::Result;
use crate::model::{CatalogSnapshot, CatalogStatus};
use crate::paths::DblPaths;

pub use crate::catalog::cache::CATALOG_CACHE_FILE;

/// Timeout for one catalog refresh request.
pub const CATALOG_REFRESH_TIMEOUT_SECS: u64 = 15;

/// How long a refreshed snapshot stays fresh.
///
/// `consts::CATALOG_TTL_HOURS` is the single source of truth (PRD D-19); this is
/// the `Duration` form the refresh takes.
pub fn catalog_ttl() -> Duration {
    Duration::from_secs(crate::consts::CATALOG_TTL_HOURS * 60 * 60)
}

/// The snapshot in use plus the document it came from.
///
/// Keeping the document lets a refresh rewrite the cache and keeps
/// `generated_at` available for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogState {
    document: CatalogDocument,
    snapshot: CatalogSnapshot,
}

impl CatalogState {
    /// Wraps a validated document and its domain projection.
    pub fn new(document: CatalogDocument, snapshot: CatalogSnapshot) -> Self {
        Self { document, snapshot }
    }

    /// Snapshot used to build read models.
    pub fn snapshot(&self) -> &CatalogSnapshot {
        &self.snapshot
    }

    /// Raw document, for caching and inspection.
    pub fn document(&self) -> &CatalogDocument {
        &self.document
    }

    /// Freshness of the snapshot in use.
    pub fn status(&self) -> CatalogStatus {
        self.snapshot.status()
    }

    /// Whether the snapshot should be refreshed at `now`.
    pub fn needs_refresh(&self, now: SystemTime, ttl: Duration, force: bool) -> bool {
        force || self.snapshot.is_stale_at(now, ttl)
    }

    /// Returns a copy with a different freshness.
    pub fn with_status(&self, status: CatalogStatus) -> Self {
        Self {
            document: self.document.clone(),
            snapshot: self.snapshot.clone().with_status(status),
        }
    }
}

/// Parses and validates the snapshot compiled into the binary.
///
/// A corrupt bundled file is a build failure caught by a test, never a runtime
/// panic: this returns a `DblError` and the caller still has the cache.
pub fn load_bundled_snapshot() -> Result<CatalogState> {
    let document: CatalogDocument = serde_json::from_str(crate::catalog::BUNDLED_CATALOG_JSON)
        .map_err(|err| crate::DblError::Storage {
            reason: format!("bundled catalog is not valid JSON: {err}"),
        })?;
    validate_document(&document)?;
    let snapshot = document.into_snapshot(SystemTime::now(), CatalogStatus::Stale)?;
    Ok(CatalogState::new(document, snapshot))
}

/// The bundled snapshot as a domain value.
pub fn bundled_snapshot() -> Result<CatalogSnapshot> {
    Ok(load_bundled_snapshot()?.snapshot)
}

/// Loads the best snapshot available without touching the network.
///
/// Order: a valid, unexpired cache; a valid but expired cache; the bundled
/// snapshot. Any problem with the cache is silent (D-19).
pub fn load_or_bundled(paths: &DblPaths, now: SystemTime, ttl: Duration) -> Result<CatalogState> {
    if let Some(cached) = read_cache(&catalog_cache_path(paths)) {
        let status = if cached.is_expired_at(now, ttl) {
            CatalogStatus::Stale
        } else {
            CatalogStatus::Fresh
        };
        let snapshot = cached.document.into_snapshot(cached.fetched_at(), status)?;
        return Ok(CatalogState::new(cached.document, snapshot));
    }
    load_bundled_snapshot()
}

/// Refreshes the catalog, silently.
///
/// This never reports an HTTP or parse failure to the caller: a timeout, a
/// malformed payload or an unwritable cache only downgrades the status and the
/// previous snapshot stays in use (F-CAT-01 AC3, D-19). An error is returned
/// only for a programmatic mistake (a `force` refresh of state that cannot be
/// re-projected), which cannot happen for a validated snapshot.
pub async fn refresh<F: CatalogFetcher>(
    state: &CatalogState,
    paths: &DblPaths,
    fetcher: &F,
    force: bool,
    now: SystemTime,
    ttl: Duration,
) -> Result<CatalogState> {
    if !state.needs_refresh(now, ttl, force) {
        return Ok(state.clone());
    }

    let url = state.document.source_url.clone();
    let etag = state.snapshot.etag().map(str::to_owned);

    match fetcher.fetch(&url, etag.as_deref()).await {
        Ok(RefreshOutcome::NotModified) => {
            let cached = CachedDocument::new(state.document.clone(), now, etag);
            record(cache_write(paths, &cached), "refresh time");
            Ok(state.with_status(CatalogStatus::Fresh))
        }
        Ok(RefreshOutcome::Modified { etag, body }) => {
            let document = match serde_json::from_str::<CatalogDocument>(&body) {
                Ok(document) if validate_document(&document).is_ok() => document,
                Ok(_) => {
                    tracing::debug!("catalog refresh returned a payload that failed validation");
                    return Ok(degrade(state));
                }
                Err(err) => {
                    tracing::debug!(%err, "catalog refresh returned malformed JSON");
                    return Ok(degrade(state));
                }
            };
            let cached = CachedDocument::new(document.clone(), now, etag);
            record(cache_write(paths, &cached), "catalog cache");
            let mut snapshot = document.into_snapshot(now, CatalogStatus::Fresh)?;
            if let Some(etag) = &cached.etag {
                snapshot = snapshot.with_etag(etag.clone());
            }
            Ok(CatalogState::new(document, snapshot))
        }
        Err(err) => {
            tracing::debug!(%err, "catalog refresh failed");
            Ok(degrade(state))
        }
    }
}

fn cache_write(paths: &DblPaths, cached: &CachedDocument) -> Result<()> {
    write_cache(&catalog_cache_path(paths), cached)
}

/// A cache we cannot write is a diagnostic, not a failed refresh: the fetched
/// document is still usable for this session.
fn record(result: Result<()>, what: &str) {
    if let Err(err) = result {
        tracing::debug!(%err, "catalog could not write the {what}");
    }
}

fn degrade(state: &CatalogState) -> CatalogState {
    match state.status() {
        CatalogStatus::Fresh => state.with_status(CatalogStatus::Stale),
        CatalogStatus::Stale => state.with_status(CatalogStatus::Offline),
        CatalogStatus::Offline => state.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::dto::tests::sample_document;
    use std::sync::Mutex;
    use tempfile::tempdir;

    enum FakeResponse {
        Ok(RefreshOutcome),
        Network,
    }

    struct FakeFetcher {
        response: FakeResponse,
        calls: Mutex<Vec<Option<String>>>,
    }

    impl FakeFetcher {
        fn new(response: Result<RefreshOutcome>) -> Self {
            let response = match response {
                Ok(outcome) => FakeResponse::Ok(outcome),
                Err(_) => FakeResponse::Network,
            };
            Self {
                response,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Option<String>> {
            self.calls.lock().expect("lock").clone()
        }
    }

    impl CatalogFetcher for FakeFetcher {
        fn fetch(
            &self,
            _url: &str,
            etag: Option<&str>,
        ) -> impl std::future::Future<Output = Result<RefreshOutcome>> + Send {
            self.calls
                .lock()
                .expect("lock")
                .push(etag.map(str::to_owned));
            let response = match &self.response {
                FakeResponse::Ok(outcome) => Ok(outcome.clone()),
                FakeResponse::Network => Err(crate::DblError::Network {
                    reason: "timeout".to_owned(),
                }),
            };
            async move { response }
        }
    }

    fn state(status: CatalogStatus) -> CatalogState {
        let document = sample_document();
        let fetched_at = match status {
            CatalogStatus::Fresh => SystemTime::now(),
            _ => SystemTime::UNIX_EPOCH,
        };
        let snapshot = document.into_snapshot(fetched_at, status).expect("valid");
        CatalogState::new(document, snapshot)
    }

    #[test]
    fn ttl_comes_from_the_shared_constants() {
        assert_eq!(catalog_ttl(), Duration::from_secs(24 * 60 * 60));
        assert_eq!(CATALOG_REFRESH_TIMEOUT_SECS, 15);
    }

    #[tokio::test]
    async fn bundled_snapshot_is_valid() {
        let bundled = load_bundled_snapshot().expect("bundled catalog must validate");
        assert!(!bundled.snapshot().artifacts().is_empty());
        assert!(!bundled.snapshot().versions().is_empty());
        assert_eq!(bundled.status(), CatalogStatus::Stale);
    }

    #[tokio::test]
    async fn loads_bundled_when_the_cache_is_missing() {
        let dir = tempdir().expect("tempdir");
        let paths = DblPaths::with_root(dir.path());
        let state = load_or_bundled(&paths, SystemTime::now(), Duration::from_secs(86_400))
            .expect("bundled fallback");
        assert_eq!(state.status(), CatalogStatus::Stale);
    }

    #[tokio::test]
    async fn fresh_and_expired_cache_differ_only_in_status() {
        let dir = tempdir().expect("tempdir");
        let paths = DblPaths::with_root(dir.path());
        let now = SystemTime::now();
        let ttl = Duration::from_secs(86_400);

        let cached = CachedDocument::new(sample_document(), now - Duration::from_secs(3_600), None);
        write_cache(&catalog_cache_path(&paths), &cached).expect("write");
        let state = load_or_bundled(&paths, now, ttl).expect("cache");
        assert_eq!(state.status(), CatalogStatus::Fresh);
        assert_eq!(state.document(), &sample_document());

        let cached =
            CachedDocument::new(sample_document(), now - Duration::from_secs(90_000), None);
        write_cache(&catalog_cache_path(&paths), &cached).expect("write");
        let state = load_or_bundled(&paths, now, ttl).expect("cache");
        assert_eq!(state.status(), CatalogStatus::Stale);
    }

    #[tokio::test]
    async fn refresh_sends_the_stored_etag() {
        let dir = tempdir().expect("tempdir");
        let paths = DblPaths::with_root(dir.path());
        let fetcher = FakeFetcher::new(Ok(RefreshOutcome::NotModified));
        let state = state(CatalogStatus::Stale).with_status(CatalogStatus::Fresh);
        let _ = refresh(
            &state,
            &paths,
            &fetcher,
            true,
            SystemTime::now(),
            Duration::from_secs(86_400),
        )
        .await;
        assert_eq!(fetcher.calls(), vec![None]);
    }

    #[tokio::test]
    async fn not_modified_becomes_fresh_and_persists() {
        let dir = tempdir().expect("tempdir");
        let paths = DblPaths::with_root(dir.path());
        let fetcher = FakeFetcher::new(Ok(RefreshOutcome::NotModified));
        let now = SystemTime::now();
        let refreshed = refresh(
            &state(CatalogStatus::Stale),
            &paths,
            &fetcher,
            true,
            now,
            Duration::from_secs(86_400),
        )
        .await
        .expect("refresh");
        assert_eq!(refreshed.status(), CatalogStatus::Fresh);
        let cached = read_cache(&catalog_cache_path(&paths)).expect("cache written");
        // The cache stores whole seconds, so the round trip can land within the
        // same second in either direction.
        let cached_at = cached.fetched_at();
        let diff = if cached_at >= now {
            cached_at.duration_since(now).expect("ok")
        } else {
            now.duration_since(cached_at).expect("ok")
        };
        assert!(diff < Duration::from_secs(1), "{diff:?}");
    }

    #[tokio::test]
    async fn modified_payload_replaces_the_snapshot() {
        let dir = tempdir().expect("tempdir");
        let paths = DblPaths::with_root(dir.path());
        let body = serde_json::to_string(&sample_document()).expect("encode");
        let fetcher = FakeFetcher::new(Ok(RefreshOutcome::Modified {
            etag: Some("\"v2\"".to_owned()),
            body,
        }));
        let refreshed = refresh(
            &state(CatalogStatus::Stale),
            &paths,
            &fetcher,
            true,
            SystemTime::now(),
            Duration::from_secs(86_400),
        )
        .await
        .expect("refresh");
        assert_eq!(refreshed.status(), CatalogStatus::Fresh);
        assert_eq!(refreshed.snapshot().etag(), Some("\"v2\""));
    }

    #[tokio::test]
    async fn failed_refresh_degrades_silently() {
        let dir = tempdir().expect("tempdir");
        let paths = DblPaths::with_root(dir.path());
        let fetcher = FakeFetcher::new(Err(crate::DblError::Network {
            reason: "timeout".to_owned(),
        }));
        let ttl = Duration::from_secs(86_400);

        let once = refresh(
            &state(CatalogStatus::Stale),
            &paths,
            &fetcher,
            true,
            SystemTime::now(),
            ttl,
        )
        .await
        .expect("silent");
        assert_eq!(once.status(), CatalogStatus::Offline);
        assert_eq!(once.snapshot().artifacts().len(), 1);

        let fresh = state(CatalogStatus::Fresh);
        let degraded = refresh(&fresh, &paths, &fetcher, true, SystemTime::now(), ttl)
            .await
            .expect("silent");
        assert_eq!(degraded.status(), CatalogStatus::Stale);
    }

    #[tokio::test]
    async fn invalid_payload_keeps_the_previous_snapshot() {
        let dir = tempdir().expect("tempdir");
        let paths = DblPaths::with_root(dir.path());
        let fetcher = FakeFetcher::new(Ok(RefreshOutcome::Modified {
            etag: None,
            body: "{\"schema_version\":1}".to_owned(),
        }));
        let before = state(CatalogStatus::Stale);
        let after = refresh(
            &before,
            &paths,
            &fetcher,
            true,
            SystemTime::now(),
            Duration::from_secs(86_400),
        )
        .await
        .expect("silent");
        assert_eq!(after.document(), before.document());
        assert_eq!(after.status(), CatalogStatus::Offline);
    }

    #[tokio::test]
    async fn refresh_is_skipped_while_fresh() {
        let dir = tempdir().expect("tempdir");
        let paths = DblPaths::with_root(dir.path());
        let fetcher = FakeFetcher::new(Ok(RefreshOutcome::NotModified));
        let fresh = state(CatalogStatus::Fresh);
        let result = refresh(
            &fresh,
            &paths,
            &fetcher,
            false,
            SystemTime::now(),
            Duration::from_secs(86_400),
        )
        .await
        .expect("skipped");
        assert_eq!(result.status(), CatalogStatus::Fresh);
        assert!(fetcher.calls().is_empty());
    }
}
