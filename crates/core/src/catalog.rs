//! Engine catalog: bundled snapshot, cache, refresh, and host-filtered views.

mod cache;
mod dto;
mod fetch;
mod load;
mod select;
mod validate;
mod view;

/// Bundled catalog JSON snapshot, compiled into the binary.
///
/// Lives in `engines-v3.json` (`schema_version: 3`) so released v0.1.3/v0.1.4
/// clients keep reading `engines-v2.json` (`schema_version: 2`) without seeing
/// `redis` or `deb` entries that their validator rejects, and older v0.1.x
/// clients keep reading `engines.json` (`schema_version: 1`).
pub const BUNDLED_CATALOG_JSON: &str = include_str!("../../../catalog/engines-v3.json");

pub use cache::catalog_cache_path;
pub use dto::{
    CATALOG_SCHEMA_VERSION, CatalogArtifact, CatalogDocument, CatalogSkipCounts, CatalogVersion,
};
pub use fetch::{CatalogFetcher, HttpCatalogFetcher, RefreshOutcome};
pub use load::{
    CATALOG_CACHE_FILE, CATALOG_REFRESH_TIMEOUT_SECS, CatalogState, bundled_snapshot, catalog_ttl,
    load_bundled_snapshot, load_or_bundled, refresh,
};
pub use select::select_artifact;
pub use validate::{validate_document, validate_document_tolerant};
pub use view::{
    InstalledVersions, MAX_PATCHES_PER_LINE, MajorLineGroup, UNAVAILABLE_ON_PLATFORM,
    VersionFilter, banner_for, build_catalog_view, filter_versions, major_line_groups,
};
