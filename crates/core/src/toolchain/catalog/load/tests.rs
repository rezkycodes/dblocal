//! Catalog bundling, cache and refresh tests.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use tempfile::tempdir;

use crate::catalog::{CatalogFetcher, RefreshOutcome};
use crate::toolchain::catalog::{
    BUNDLED_CATALOG_JSON, CatalogStatus, ToolId, bundled_or_empty, catalog_ttl, load_bundled,
    load_or_bundled, read_cache, refresh, write_cache,
};
use crate::toolchain::fsops::ToolchainPaths;

const DOC: &str = r#"{
  "schema": 1,
  "generated_at": "2026-11-27T12:00:00Z",
  "source_url": "https://example.test/toolchains-v1.json",
  "tools": [{
    "id": "node",
    "versions": [{
      "version": "24.11.0", "series": "24", "lts": true,
      "artifacts": [{
        "os": "linux", "arch": "x86_64", "profile": null, "format": "tar.gz",
        "url": "https://e.t/node.tar.gz",
        "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "size_bytes": 1, "hash_origin": "upstream", "source": "nodejs.org"
      }]
    }]
  }]
}"#;

struct FakeFetcher {
    response: Result<RefreshOutcome, ()>,
    calls: Mutex<Vec<Option<String>>>,
}

impl FakeFetcher {
    fn new(response: Result<RefreshOutcome, ()>) -> Self {
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
    ) -> impl Future<Output = crate::Result<RefreshOutcome>> + Send {
        self.calls
            .lock()
            .expect("lock")
            .push(etag.map(str::to_owned));
        let response = match &self.response {
            Ok(outcome) => Ok(outcome.clone()),
            Err(()) => Err(crate::DblError::Network {
                reason: "offline".to_owned(),
            }),
        };
        async move { response }
    }
}

fn state() -> crate::toolchain::catalog::CatalogState {
    let (catalog, skips) = crate::toolchain::catalog::parse_catalog(DOC).expect("parse");
    crate::toolchain::catalog::CatalogState::new(
        catalog,
        DOC,
        CatalogStatus::Stale,
        None,
        SystemTime::UNIX_EPOCH,
        skips,
    )
}

#[test]
fn bundled_snapshot_parses() {
    // The fallback document (empty tools) is valid schema 1 and must parse.
    let bundled = load_bundled().expect("bundled parses");
    assert_eq!(bundled.status(), CatalogStatus::Stale);
    // The fallback has no tools; a real catalog would have some. Either way the
    // bundled snapshot must not be an error.
    assert!(BUNDLED_CATALOG_JSON.contains("\"schema\""));
}

#[test]
fn bundled_or_empty_never_errors() {
    let state = bundled_or_empty();
    // Freshness is either Stale (valid empty snapshot) or Offline (unusable); both
    // are acceptable and neither panics.
    assert!(matches!(
        state.status(),
        CatalogStatus::Stale | CatalogStatus::Offline
    ));
}

#[test]
fn cache_round_trips() {
    let dir = tempdir().expect("tmp");
    let paths = ToolchainPaths::with_root(dir.path());
    let now = SystemTime::now();
    let state = crate::toolchain::catalog::CatalogState::new(
        state().catalog().clone(),
        DOC,
        CatalogStatus::Fresh,
        Some("\"etag\"".to_owned()),
        now,
        Default::default(),
    );
    write_cache(&paths, &state).expect("write");
    let read = read_cache(&paths).expect("read");
    assert_eq!(read.status(), CatalogStatus::Fresh);
    assert_eq!(
        read.catalog().tool(ToolId::Node).unwrap().versions()[0].version(),
        "24.11.0"
    );
    assert_eq!(read.etag(), Some("\"etag\""));
}

#[test]
fn missing_or_corrupt_cache_falls_back_to_bundled() {
    let dir = tempdir().expect("tmp");
    let paths = ToolchainPaths::with_root(dir.path());
    assert!(read_cache(&paths).is_none());
    let loaded = load_or_bundled(&paths);
    assert!(matches!(
        loaded.status(),
        CatalogStatus::Stale | CatalogStatus::Offline
    ));

    std::fs::create_dir_all(paths.catalog_cache().parent().unwrap()).expect("mkdir");
    std::fs::write(paths.catalog_cache(), b"{ not json").expect("write");
    assert!(read_cache(&paths).is_none());
}

#[tokio::test]
async fn refresh_not_modified_becomes_fresh_and_caches() {
    let dir = tempdir().expect("tmp");
    let paths = ToolchainPaths::with_root(dir.path());
    let fetcher = FakeFetcher::new(Ok(RefreshOutcome::NotModified));
    let now = SystemTime::now();
    let refreshed = refresh(&state(), &paths, &fetcher, true, now).await;
    assert_eq!(refreshed.status(), CatalogStatus::Fresh);
    assert_eq!(fetcher.calls(), vec![None]);
    assert!(paths.catalog_cache().exists());
}

#[tokio::test]
async fn refresh_modified_replaces_the_document() {
    let dir = tempdir().expect("tmp");
    let paths = ToolchainPaths::with_root(dir.path());
    let fetcher = FakeFetcher::new(Ok(RefreshOutcome::Modified {
        etag: Some("\"v2\"".to_owned()),
        body: DOC.to_owned(),
    }));
    let refreshed = refresh(&state(), &paths, &fetcher, true, SystemTime::now()).await;
    assert_eq!(refreshed.status(), CatalogStatus::Fresh);
    assert_eq!(refreshed.etag(), Some("\"v2\""));
}

#[tokio::test]
async fn failed_refresh_degrades_silently() {
    let dir = tempdir().expect("tmp");
    let paths = ToolchainPaths::with_root(dir.path());
    let fetcher = FakeFetcher::new(Err(()));
    let stale = state();
    let after = refresh(&stale, &paths, &fetcher, true, SystemTime::now()).await;
    assert_eq!(after.status(), CatalogStatus::Offline);
    // The document is unchanged.
    assert_eq!(after.catalog(), stale.catalog());
}

#[tokio::test]
async fn invalid_body_keeps_the_previous_snapshot() {
    let dir = tempdir().expect("tmp");
    let paths = ToolchainPaths::with_root(dir.path());
    let fetcher = FakeFetcher::new(Ok(RefreshOutcome::Modified {
        etag: None,
        body: r#"{"schema": 999, "tools": []}"#.to_owned(),
    }));
    let before = state();
    let after = refresh(&before, &paths, &fetcher, true, SystemTime::now()).await;
    assert_eq!(after.status(), CatalogStatus::Offline);
    assert_eq!(after.catalog(), before.catalog());
}

#[tokio::test]
async fn refresh_is_skipped_while_fresh() {
    let dir = tempdir().expect("tmp");
    let paths = ToolchainPaths::with_root(dir.path());
    let fetcher = FakeFetcher::new(Ok(RefreshOutcome::NotModified));
    let fresh = crate::toolchain::catalog::CatalogState::new(
        state().catalog().clone(),
        DOC,
        CatalogStatus::Fresh,
        None,
        SystemTime::now(),
        Default::default(),
    );
    let result = refresh(&fresh, &paths, &fetcher, false, SystemTime::now()).await;
    assert_eq!(result.status(), CatalogStatus::Fresh);
    assert!(fetcher.calls().is_empty());
}

#[test]
fn ttl_is_24_hours() {
    assert_eq!(catalog_ttl(), Duration::from_secs(24 * 60 * 60));
}
