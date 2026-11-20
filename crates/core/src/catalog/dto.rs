//! JSON shape of `catalog/engines.json` and the conversion into domain types.
//!
//! The document is deliberately flat: one list of versions and one list of
//! artifacts. Grouping per engine and major line happens in
//! [`crate::catalog::view`], so a reader never has to walk a nested tree, and
//! the builder only has to sort two arrays.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::DblError;
use crate::error::Result;
use crate::model::{
    ArchiveFormat, Artifact, CatalogSnapshot, CatalogStatus, EngineKey, EngineVersion,
};
use crate::platform::{Arch, Os};

/// Schema version this build understands.
///
/// A payload advertising anything else is rejected: a higher version may carry
/// semantics we do not know, a lower one is no longer produced (F-CAT-01 AC4).
pub const CATALOG_SCHEMA_VERSION: u32 = 1;

/// One released engine version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogVersion {
    /// Engine identifier, for example `postgresql`.
    pub engine: String,
    /// Version tag, for example `18.7.0`.
    pub version: String,
    /// Major line, for example `18`.
    pub major_line: String,
    /// Upstream release date, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    /// Whether upstream no longer supports this version.
    #[serde(default)]
    pub eol: bool,
}

/// One downloadable build for one `(os, arch)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogArtifact {
    /// Stable identifier, unique inside the document.
    pub id: String,
    /// Engine identifier.
    pub engine: String,
    /// Version tag this build belongs to.
    pub version: String,
    /// Target OS.
    pub os: String,
    /// Target architecture.
    pub arch: String,
    /// HTTPS download URL.
    pub url: String,
    /// Lowercase hex SHA-256 of the archive.
    pub sha256: String,
    /// Archive size in bytes.
    pub size_bytes: u64,
    /// Archive format, for example `tar.gz`.
    pub format: String,
    /// Top-level directory to strip while extracting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strip_prefix: Option<String>,
    /// glibc floor the build was linked against, for example `2.38`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_glibc: Option<String>,
    /// Upstream build variant, for example Valkey's `jammy`/`noble`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

/// The whole catalog document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogDocument {
    /// Payload schema version.
    pub schema_version: u32,
    /// When the builder produced the file, RFC 3339 UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
    /// Where the runtime refreshes this document from.
    pub source_url: String,
    /// Every released version, grouped per engine and sorted newest first.
    pub versions: Vec<CatalogVersion>,
    /// Every downloadable build, sorted by engine, version, OS and arch.
    pub artifacts: Vec<CatalogArtifact>,
}

impl CatalogDocument {
    /// Converts the document into a validated [`CatalogSnapshot`].
    ///
    /// Every artifact goes through [`Artifact::new`], so the same invariants the
    /// rest of the app relies on (HTTPS URL, 64-hex SHA-256, non-zero size) are
    /// enforced here rather than at download time.
    pub fn into_snapshot(
        &self,
        fetched_at: SystemTime,
        status: CatalogStatus,
    ) -> Result<CatalogSnapshot> {
        crate::catalog::validate::check_schema_version(self.schema_version)?;
        crate::catalog::validate::check_source_url(&self.source_url)?;

        let mut versions = Vec::with_capacity(self.versions.len());
        for entry in &self.versions {
            let engine = parse_engine(&entry.engine)?;
            let version = EngineVersion::new(
                engine,
                entry.version.clone(),
                entry.release_date.clone(),
                entry.eol,
            )?;
            if version.major_line() != entry.major_line {
                return Err(invalid(
                    "major_line",
                    format!(
                        "{} {} declares major line {} but derives {}",
                        entry.engine,
                        entry.version,
                        entry.major_line,
                        version.major_line()
                    ),
                ));
            }
            versions.push(version);
        }

        let mut artifacts = Vec::with_capacity(self.artifacts.len());
        for entry in &self.artifacts {
            artifacts.push(domain_artifact(entry)?);
        }

        Ok(CatalogSnapshot::new(
            self.id(),
            fetched_at,
            self.source_url.clone(),
            self.schema_version,
            versions,
            artifacts,
            status,
        ))
    }

    /// Stable snapshot identifier derived from the document contents.
    pub fn id(&self) -> String {
        match &self.generated_at {
            Some(stamp) => format!("catalog-{stamp}"),
            None => "catalog".to_owned(),
        }
    }
}

fn domain_artifact(entry: &CatalogArtifact) -> Result<Artifact> {
    let engine = parse_engine(&entry.engine)?;
    let os = entry.os.parse::<Os>()?;
    let arch = entry.arch.parse::<Arch>()?;
    let format = entry.format.parse::<ArchiveFormat>()?;
    let mut artifact = Artifact::new(
        entry.id.clone(),
        engine,
        entry.version.clone(),
        os,
        arch,
        entry.url.clone(),
        entry.sha256.clone(),
        entry.size_bytes,
        format,
    )?;
    if let Some(prefix) = &entry.strip_prefix {
        artifact = artifact.with_strip_prefix(prefix.clone());
    }
    if let Some(glibc) = &entry.min_glibc {
        artifact = artifact.with_min_glibc(glibc.clone());
    }
    if let Some(variant) = &entry.variant {
        artifact = artifact.with_variant(variant.clone());
    }
    Ok(artifact)
}

fn parse_engine(value: &str) -> Result<EngineKey> {
    value.parse::<EngineKey>().map_err(|_| {
        invalid(
            "engine",
            format!("unknown engine {value:?}, expected one of postgresql, mysql, mariadb, valkey"),
        )
    })
}

pub(crate) fn invalid(field: &'static str, reason: impl Into<String>) -> DblError {
    DblError::InvalidInput {
        field,
        reason: reason.into(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sha() -> String {
        "0123456789abcdef".repeat(4)
    }

    pub(crate) fn sample_document() -> CatalogDocument {
        CatalogDocument {
            schema_version: CATALOG_SCHEMA_VERSION,
            generated_at: Some("2026-11-20T00:00:00Z".to_owned()),
            source_url: "https://example.test/catalog/engines.json".to_owned(),
            versions: vec![CatalogVersion {
                engine: "postgresql".to_owned(),
                version: "18.7.0".to_owned(),
                major_line: "18".to_owned(),
                release_date: Some("2026-11-12".to_owned()),
                eol: false,
            }],
            artifacts: vec![CatalogArtifact {
                id: "postgresql-18.7.0-linux-x86_64".to_owned(),
                engine: "postgresql".to_owned(),
                version: "18.7.0".to_owned(),
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
                url: "https://example.test/postgresql-18.7.0.tar.gz".to_owned(),
                sha256: sha(),
                size_bytes: 42,
                format: "tar.gz".to_owned(),
                strip_prefix: Some("postgresql-18.7.0/".to_owned()),
                min_glibc: None,
                variant: None,
            }],
        }
    }

    #[test]
    fn document_round_trips_through_json() {
        let document = sample_document();
        let text = serde_json::to_string(&document).expect("serialize");
        let parsed: CatalogDocument = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(parsed, document);
    }

    #[test]
    fn conversion_builds_domain_types() {
        let snapshot = sample_document()
            .into_snapshot(SystemTime::UNIX_EPOCH, CatalogStatus::Fresh)
            .expect("valid document");
        assert_eq!(snapshot.schema_version(), CATALOG_SCHEMA_VERSION);
        assert_eq!(snapshot.versions().len(), 1);
        assert_eq!(snapshot.artifacts().len(), 1);
        assert_eq!(snapshot.artifacts()[0].variant(), None);
        assert_eq!(
            snapshot.artifacts()[0].strip_prefix(),
            Some("postgresql-18.7.0/")
        );
        assert_eq!(snapshot.id(), "catalog-2026-11-20T00:00:00Z");
    }

    #[test]
    fn conversion_rejects_bad_fields() {
        let mut document = sample_document();
        document.artifacts[0].sha256 = "nope".to_owned();
        assert!(
            document
                .into_snapshot(SystemTime::UNIX_EPOCH, CatalogStatus::Fresh)
                .is_err()
        );

        let mut document = sample_document();
        document.artifacts[0].url = "http://example.test/a.tar.gz".to_owned();
        assert!(
            document
                .into_snapshot(SystemTime::UNIX_EPOCH, CatalogStatus::Fresh)
                .is_err()
        );

        let mut document = sample_document();
        document.versions[0].major_line = "19".to_owned();
        assert!(
            document
                .into_snapshot(SystemTime::UNIX_EPOCH, CatalogStatus::Fresh)
                .is_err()
        );

        let mut document = sample_document();
        document.artifacts[0].engine = "sqlite".to_owned();
        assert!(
            document
                .into_snapshot(SystemTime::UNIX_EPOCH, CatalogStatus::Fresh)
                .is_err()
        );
    }
}
