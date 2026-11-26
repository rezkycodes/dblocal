//! On-disk cache for a refreshed catalog document.
//!
//! Writes are atomic: the new document is written next to the target and then
//! renamed over it, so a crash mid-write can never leave a half file behind.
//! A cache file that fails validation is ignored and never replaces a good one.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::catalog::dto::{CATALOG_SCHEMA_VERSION, CatalogDocument};
use crate::catalog::validate::validate_document_tolerant;
use crate::error::Result;
use crate::paths::DblPaths;

/// Sub-directory of the DBLocal home that holds refreshed data.
pub const DIR_CACHE: &str = "cache";
/// File name of the cached catalog document.
pub const CATALOG_CACHE_FILE: &str = "catalog.json";

/// A validated catalog document plus the metadata of the fetch that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedDocument {
    /// Payload schema version, repeated here so a stale cache is obvious.
    pub schema_version: u32,
    /// Unix seconds when the document was fetched.
    pub fetched_at: u64,
    /// HTTP ETag of the response, sent back as `If-None-Match`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// The document itself.
    pub document: CatalogDocument,
}

impl CachedDocument {
    /// Wraps a document with its fetch metadata.
    pub fn new(document: CatalogDocument, fetched_at: SystemTime, etag: Option<String>) -> Self {
        Self {
            schema_version: document.schema_version,
            fetched_at: epoch_secs(fetched_at),
            etag,
            document,
        }
    }

    /// Whether this cache is old enough to refresh.
    pub fn is_expired_at(&self, now: SystemTime, ttl: Duration) -> bool {
        let now_secs = epoch_secs(now);
        // A clock that moved backwards must not pin a snapshot forever
        // (PRD F-CAT edge case: treat negative ages as expired).
        match now_secs.checked_sub(self.fetched_at) {
            Some(age) => age > ttl.as_secs(),
            None => true,
        }
    }

    /// Timestamp as a `SystemTime`.
    pub fn fetched_at(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(self.fetched_at)
    }
}

/// Cache file location under a DBLocal home.
pub fn catalog_cache_path(paths: &DblPaths) -> PathBuf {
    paths.root().join(DIR_CACHE).join(CATALOG_CACHE_FILE)
}

/// Reads and validates the cache.
///
/// A missing, unreadable or invalid file yields `None`: the caller falls back to
/// the bundled snapshot, which is always safe (D-19).
pub fn read_cache(path: &Path) -> Option<CachedDocument> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::debug!(path = %path.display(), %err, "catalog cache unreadable");
            return None;
        }
    };
    let cached: CachedDocument = match serde_json::from_slice(&bytes) {
        Ok(cached) => cached,
        Err(err) => {
            tracing::debug!(path = %path.display(), %err, "catalog cache is not valid JSON");
            return None;
        }
    };
    if cached.schema_version != CATALOG_SCHEMA_VERSION
        || validate_document_tolerant(&cached.document).is_err()
    {
        tracing::debug!(path = %path.display(), "catalog cache failed validation");
        return None;
    }
    Some(cached)
}

/// Writes the cache atomically: temporary sibling, then rename.
pub fn write_cache(path: &Path, cached: &CachedDocument) -> Result<()> {
    let parent = path.parent().ok_or_else(|| crate::DblError::Storage {
        reason: format!("catalog cache path has no parent: {}", path.display()),
    })?;
    std::fs::create_dir_all(parent)?;

    let bytes = serde_json::to_vec_pretty(cached).map_err(|err| crate::DblError::Storage {
        reason: format!("could not encode catalog cache: {err}"),
    })?;

    let temp = parent.join(format!(
        ".{}.{}.tmp",
        CATALOG_CACHE_FILE,
        std::process::id()
    ));
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);

    match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&temp);
            Err(err.into())
        }
    }
}

fn epoch_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|age| age.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::dto::tests::sample_document;
    use tempfile::tempdir;

    fn cached() -> CachedDocument {
        CachedDocument::new(
            sample_document(),
            UNIX_EPOCH + Duration::from_secs(1_000),
            Some("\"v1\"".to_owned()),
        )
    }

    #[test]
    fn path_lives_under_the_home() {
        let dir = tempdir().expect("tempdir");
        let paths = DblPaths::with_root(dir.path());
        let path = catalog_cache_path(&paths);
        assert!(path.starts_with(dir.path()));
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some(CATALOG_CACHE_FILE)
        );
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join(CATALOG_CACHE_FILE);
        let original = cached();
        write_cache(&path, &original).expect("write");
        let read = read_cache(&path).expect("read");
        assert_eq!(read, original);
        assert_eq!(read.fetched_at(), UNIX_EPOCH + Duration::from_secs(1_000));
    }

    #[test]
    fn missing_and_corrupt_caches_are_none() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join(CATALOG_CACHE_FILE);
        assert!(read_cache(&path).is_none());

        std::fs::write(&path, b"{ not json").expect("write");
        assert!(read_cache(&path).is_none());

        let mut broken = cached();
        broken.document.artifacts[0].sha256 = "nope".to_owned();
        std::fs::write(&path, serde_json::to_vec(&broken).expect("encode")).expect("write");
        assert!(read_cache(&path).is_none());

        let mut wrong_schema = cached();
        wrong_schema.schema_version = CATALOG_SCHEMA_VERSION + 1;
        std::fs::write(&path, serde_json::to_vec(&wrong_schema).expect("encode")).expect("write");
        assert!(read_cache(&path).is_none());

        // Upgrading from v0.1.x leaves a schema_version 1 cache on disk; v2
        // rejects it silently so the user immediately sees the bundled v2
        // catalog instead of waiting for the old TTL to expire.
        let mut old_schema = cached();
        old_schema.schema_version = 1;
        std::fs::write(&path, serde_json::to_vec(&old_schema).expect("encode")).expect("write");
        assert!(read_cache(&path).is_none());
    }

    #[test]
    fn write_leaves_no_temp_file_behind() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join(CATALOG_CACHE_FILE);
        write_cache(&path, &cached()).expect("write");
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .expect("read_dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![CATALOG_CACHE_FILE.to_owned()]);
    }

    #[test]
    fn ttl_and_clock_skew() {
        let cached = cached();
        let ttl = Duration::from_secs(86_400);
        assert!(!cached.is_expired_at(UNIX_EPOCH + Duration::from_secs(1_000), ttl));
        assert!(cached.is_expired_at(UNIX_EPOCH + Duration::from_secs(1_000 + 86_401), ttl));
        // Time before the fetch timestamp is treated as expired.
        assert!(cached.is_expired_at(UNIX_EPOCH, ttl));
    }
}
