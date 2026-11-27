//! Loading the bundled toolchain catalog, its cache, and refreshing it.
//!
//! This mirrors the engine catalog (`crate::catalog`): the document is embedded at
//! build time, a refreshed copy is cached atomically under `<root>/cache`, and a
//! refresh that fails degrades silently to the last good snapshot. The cache keeps
//! the raw JSON, so a cached document is re-parsed by the same tolerant parser on
//! every load; nothing trusts a cached copy more than a fresh one.

use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::catalog::{CatalogFetcher, RefreshOutcome};
use crate::toolchain::catalog::{SkipCounts, ToolchainCatalog, parse_catalog};
use crate::toolchain::consts;
use crate::toolchain::fsops::ToolchainPaths;
use crate::DblError;

/// The catalog embedded at build time.
///
/// `crates/core/build.rs` writes this file into `OUT_DIR`: the generated
/// `catalog/toolchains-v1.json` when it exists, an empty schema-1 document
/// otherwise. Release builds that have the generated catalog therefore bundle it
/// exactly as the engine catalog is bundled.
pub const BUNDLED_CATALOG_JSON: &str =
    include_str!(concat!(env!("OUT_DIR"), "/toolchains-v1.json"));

/// Freshness of the snapshot in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogStatus {
    /// Loaded from a cache refreshed within the TTL.
    Fresh,
    /// Bundled, or a cache older than the TTL.
    Stale,
    /// A refresh failed (or the bundled file is unusable); the last snapshot
    /// stands.
    Offline,
}

/// A parsed catalog plus its freshness, fetch metadata and raw text.
#[derive(Debug, Clone)]
pub struct CatalogState {
    catalog: ToolchainCatalog,
    raw: String,
    status: CatalogStatus,
    etag: Option<String>,
    generated_at: SystemTime,
    skips: SkipCounts,
}

impl CatalogState {
    /// Wraps a parsed catalog with its metadata.
    pub fn new(
        catalog: ToolchainCatalog,
        raw: impl Into<String>,
        status: CatalogStatus,
        etag: Option<String>,
        generated_at: SystemTime,
        skips: SkipCounts,
    ) -> Self {
        Self {
            catalog,
            raw: raw.into(),
            status,
            etag,
            generated_at,
            skips,
        }
    }

    /// The catalog itself.
    pub fn catalog(&self) -> &ToolchainCatalog {
        &self.catalog
    }

    /// The raw JSON the catalog was parsed from.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Its freshness.
    pub const fn status(&self) -> CatalogStatus {
        self.status
    }

    /// The ETag to send on the next refresh.
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    /// When the document was generated or fetched.
    pub const fn generated_at(&self) -> SystemTime {
        self.generated_at
    }

    /// Entries the tolerant parser skipped when this state was built.
    pub const fn skips(&self) -> SkipCounts {
        self.skips
    }
}

fn build_state(raw: &str, status: CatalogStatus, etag: Option<String>, at: SystemTime) -> crate::Result<CatalogState> {
    let (catalog, skips) = parse_catalog(raw)?;
    Ok(CatalogState::new(catalog, raw, status, etag, at, skips))
}

/// Parses the embedded snapshot.
pub fn load_bundled() -> crate::Result<CatalogState> {
    build_state(
        BUNDLED_CATALOG_JSON,
        CatalogStatus::Stale,
        None,
        SystemTime::UNIX_EPOCH,
    )
}

/// The bundled snapshot, or an empty catalog if the embedded file is unusable.
///
/// Never returns an error: a toolchain operation can always proceed with what it
/// has, and "no versions available" is a better outcome than a crash.
pub fn bundled_or_empty() -> CatalogState {
    match load_bundled() {
        Ok(state) => state,
        Err(err) => {
            tracing::warn!(%err, "bundled toolchain catalog is unusable; continuing with none");
            CatalogState::new(
                ToolchainCatalog::empty(),
                String::new(),
                CatalogStatus::Offline,
                None,
                SystemTime::UNIX_EPOCH,
                SkipCounts::default(),
            )
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct WireCache {
    schema: u32,
    fetched_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    /// The raw catalog JSON, re-parsed on load.
    document: String,
}

/// Reads the cache if present and valid; otherwise `None`.
pub fn read_cache(paths: &ToolchainPaths) -> Option<CatalogState> {
    let path = paths.catalog_cache();
    let bytes = std::fs::read(&path).ok()?;
    let wire: WireCache = serde_json::from_slice(&bytes).ok()?;
    if wire.schema != consts::CATALOG_SCHEMA_VERSION {
        return None;
    }
    let fetched_at = SystemTime::UNIX_EPOCH + Duration::from_secs(wire.fetched_at);
    let status = if is_expired(fetched_at, SystemTime::now(), catalog_ttl()) {
        CatalogStatus::Stale
    } else {
        CatalogStatus::Fresh
    };
    build_state(&wire.document, status, wire.etag, fetched_at).ok()
}

/// Writes the cache atomically (temp file + rename).
pub fn write_cache(paths: &ToolchainPaths, state: &CatalogState) -> crate::Result<()> {
    let path = paths.catalog_cache();
    let fetched_at = state
        .generated_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let wire = WireCache {
        schema: consts::CATALOG_SCHEMA_VERSION,
        fetched_at,
        etag: state.etag.clone(),
        document: state.raw.clone(),
    };
    let mut bytes = serde_json::to_vec(&wire).map_err(|err| DblError::Storage {
        reason: format!("could not encode toolchain catalog cache: {err}"),
    })?;
    bytes.push(b'\n');
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| DblError::Io { source })?;
    }
    crate::fsutil::write_private_file_atomic(&path, &bytes).map_err(|source| DblError::Io { source })
}

/// Loads the best snapshot available without touching the network.
///
/// Order: a valid cache (fresh or stale), then the bundled snapshot.
pub fn load_or_bundled(paths: &ToolchainPaths) -> CatalogState {
    read_cache(paths).unwrap_or_else(bundled_or_empty)
}

fn is_expired(fetched_at: SystemTime, now: SystemTime, ttl: Duration) -> bool {
    match now.duration_since(fetched_at) {
        Ok(age) => age > ttl,
        // A clock that moved backwards must not pin a snapshot forever.
        Err(_) => true,
    }
}

/// The TTL applied to a refreshed snapshot.
pub fn catalog_ttl() -> Duration {
    Duration::from_secs(consts::CATALOG_TTL_HOURS * 60 * 60)
}

/// Refreshes the catalog, silently.
///
/// Returns the state to use; a network, parse or validation failure downgrades the
/// status and keeps the previous document.
pub async fn refresh<F: CatalogFetcher>(
    state: &CatalogState,
    paths: &ToolchainPaths,
    fetcher: &F,
    force: bool,
    now: SystemTime,
) -> CatalogState {
    let needs = force || is_expired(state.generated_at, now, catalog_ttl());
    if !needs {
        return state.clone();
    }
    let url = state.catalog.source_url();
    if !is_https(url) {
        tracing::debug!("toolchain catalog has no HTTPS refresh URL; keeping the current snapshot");
        return degrade(state);
    }
    match fetcher.fetch(url, state.etag()).await {
        Ok(RefreshOutcome::NotModified) => {
            let next = CatalogState::new(
                state.catalog.clone(),
                state.raw.clone(),
                CatalogStatus::Fresh,
                state.etag.clone(),
                now,
                state.skips,
            );
            record(write_cache(paths, &next), "toolchain catalog cache");
            next
        }
        Ok(RefreshOutcome::Modified { etag, body }) => match parse_catalog(&body) {
            Ok((catalog, skips)) => {
                if skips.total() > 0 {
                    tracing::debug!(
                        skipped = skips.total(),
                        "toolchain catalog refresh skipped entries"
                    );
                }
                let next = CatalogState::new(catalog, body, CatalogStatus::Fresh, etag, now, skips);
                record(write_cache(paths, &next), "toolchain catalog cache");
                next
            }
            Err(err) => {
                tracing::debug!(%err, "toolchain catalog refresh returned an unusable payload");
                degrade(state)
            }
        },
        Err(err) => {
            tracing::debug!(%err, "toolchain catalog refresh failed");
            degrade(state)
        }
    }
}

fn is_https(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    scheme == "https" && !rest.is_empty() && !rest.contains(char::is_whitespace)
}

fn record(result: crate::Result<()>, what: &str) {
    if let Err(err) = result {
        tracing::debug!(%err, "could not write the {what}");
    }
}

fn degrade(state: &CatalogState) -> CatalogState {
    let status = match state.status {
        CatalogStatus::Fresh => CatalogStatus::Stale,
        CatalogStatus::Stale | CatalogStatus::Offline => CatalogStatus::Offline,
    };
    CatalogState::new(
        state.catalog.clone(),
        state.raw.clone(),
        status,
        state.etag.clone(),
        state.generated_at,
        state.skips,
    )
}

/// Parses a catalog, also returning the entries the tolerant parser skipped.
pub fn parse_with_skips(text: &str) -> crate::Result<(ToolchainCatalog, SkipCounts)> {
    parse_catalog(text)
}

#[cfg(test)]
mod tests;
