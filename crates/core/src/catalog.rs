//! Engine catalog: bundled snapshot, cache, refresh, and host-filtered views.

mod cache;
mod dto;
mod fetch;
mod load;
mod select;
mod validate;
mod view;

/// Bundled catalog JSON snapshot, compiled into the binary.
pub const BUNDLED_CATALOG_JSON: &str = include_str!("../../../catalog/engines.json");

pub use cache::catalog_cache_path;
pub use dto::{CATALOG_SCHEMA_VERSION, CatalogArtifact, CatalogDocument, CatalogVersion};
pub use fetch::{CatalogFetcher, HttpCatalogFetcher, RefreshOutcome};
pub use load::{
    CATALOG_CACHE_FILE, CATALOG_REFRESH_TIMEOUT_SECS, CatalogState, bundled_snapshot, catalog_ttl,
    load_bundled_snapshot, load_or_bundled, refresh,
};
pub use select::select_artifact;
pub use validate::validate_document;
pub use view::{
    InstalledVersions, MajorLineGroup, UNAVAILABLE_ON_PLATFORM, banner_for, build_catalog_view,
    major_line_groups,
};
