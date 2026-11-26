//! Validation shared by the runtime loader, the xtask builder and CI.
//!
//! Validation is deliberately split from parsing: [`validate_document`] takes a
//! parsed document and returns the first problem it finds, so both the runtime
//! (which must never panic on a corrupt bundled file) and the offline
//! `xtask catalog validate` use exactly one implementation.

use std::collections::BTreeSet;
use std::time::SystemTime;

use semver::Version;

use crate::catalog::dto::{
    CATALOG_SCHEMA_VERSION, CatalogArtifact, CatalogDocument, CatalogSkipCounts, invalid,
};
use crate::error::Result;
use crate::model::{ArchiveFormat, CatalogStatus, EngineKey};

/// Checks every invariant a catalog document must hold strictly.
///
/// Unknown engine keys and archive formats fail validation. Used by
/// `xtask catalog validate` and the catalog builder so typos fail CI.
pub fn validate_document(document: &CatalogDocument) -> Result<()> {
    check_schema_version(document.schema_version)?;
    check_source_url(&document.source_url)?;
    check_versions(document, false)?;
    check_artifacts(document, false)?;
    // The last step is the authoritative one: it reuses the same domain
    // constructors the rest of the app trusts.
    document.into_snapshot(SystemTime::UNIX_EPOCH, CatalogStatus::Fresh)?;
    Ok(())
}

/// Validates a schema-3 catalog document for runtime clients, skipping entries
/// with unknown engine keys or unknown archive formats (and artifacts belonging
/// to a skipped version) while keeping malformed entries of known engines as
/// hard errors.
pub fn validate_document_tolerant(document: &CatalogDocument) -> Result<CatalogSkipCounts> {
    check_schema_version(document.schema_version)?;
    check_source_url(&document.source_url)?;
    check_versions(document, true)?;
    check_artifacts(document, true)?;
    let (_, counts) =
        document.into_snapshot_tolerant(SystemTime::UNIX_EPOCH, CatalogStatus::Fresh)?;
    Ok(counts)
}

/// Rejects payloads this build cannot safely interpret.
pub(crate) fn check_schema_version(version: u32) -> Result<()> {
    if version == CATALOG_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(invalid(
            "schema_version",
            format!(
                "unsupported schema version {version}, this build understands {CATALOG_SCHEMA_VERSION}"
            ),
        ))
    }
}

/// The refresh source must be HTTPS (AGENTS.md invariant 6).
pub(crate) fn check_source_url(url: &str) -> Result<()> {
    let https = url.split_once("://").is_some_and(|(scheme, rest)| {
        scheme == "https" && !rest.is_empty() && !rest.contains(char::is_whitespace)
    });
    if https {
        Ok(())
    } else {
        Err(invalid("source_url", "must be an absolute https URL"))
    }
}

fn check_versions(document: &CatalogDocument, tolerant: bool) -> Result<()> {
    let mut seen = BTreeSet::new();
    let mut by_engine: std::collections::BTreeMap<&str, Vec<Version>> =
        std::collections::BTreeMap::new();
    for entry in &document.versions {
        if tolerant && entry.engine.parse::<EngineKey>().is_err() {
            continue;
        }
        if !seen.insert((entry.engine.as_str(), entry.version.as_str())) {
            return Err(invalid(
                "versions",
                format!("duplicate version {} {}", entry.engine, entry.version),
            ));
        }
        by_engine
            .entry(entry.engine.as_str())
            .or_default()
            .push(parse_semver(&entry.version)?);
    }
    for (engine, tags) in by_engine {
        for window in tags.windows(2) {
            if window[1] > window[0] {
                return Err(invalid(
                    "versions",
                    format!(
                        "{engine} is not ordered newest first: {} appears after {}",
                        window[1], window[0]
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn check_artifacts(document: &CatalogDocument, tolerant: bool) -> Result<()> {
    let mut seen_ids = BTreeSet::new();
    let versions: BTreeSet<(&str, &str)> = document
        .versions
        .iter()
        .filter(|v| !tolerant || v.engine.parse::<EngineKey>().is_ok())
        .map(|v| (v.engine.as_str(), v.version.as_str()))
        .collect();
    let skipped_versions: BTreeSet<(&str, &str)> = if tolerant {
        document
            .versions
            .iter()
            .filter(|v| v.engine.parse::<EngineKey>().is_err())
            .map(|v| (v.engine.as_str(), v.version.as_str()))
            .collect()
    } else {
        BTreeSet::new()
    };

    for artifact in &document.artifacts {
        if tolerant
            && (artifact.engine.parse::<EngineKey>().is_err()
                || skipped_versions.contains(&(artifact.engine.as_str(), artifact.version.as_str()))
                || artifact.format.parse::<ArchiveFormat>().is_err())
        {
            continue;
        }
        check_artifact(artifact, &versions)?;
        if !seen_ids.insert(artifact.id.as_str()) {
            return Err(invalid(
                "artifacts",
                format!("duplicate artifact id {}", artifact.id),
            ));
        }
    }
    Ok(())
}

fn check_artifact(artifact: &CatalogArtifact, versions: &BTreeSet<(&str, &str)>) -> Result<()> {
    if artifact.id.trim().is_empty() {
        return Err(invalid("id", "artifact id must not be empty"));
    }
    if !is_sha256_hex(&artifact.sha256) {
        return Err(invalid(
            "sha256",
            format!(
                "artifact {} must carry 64 lowercase hex characters, got {:?}",
                artifact.id, artifact.sha256
            ),
        ));
    }
    if artifact.size_bytes == 0 {
        return Err(invalid(
            "size_bytes",
            format!("artifact {} must have a non-zero size", artifact.id),
        ));
    }
    let https = artifact
        .url
        .split_once("://")
        .is_some_and(|(scheme, rest)| {
            scheme == "https" && !rest.is_empty() && !rest.contains(char::is_whitespace)
        });
    if !https {
        return Err(invalid(
            "url",
            format!("artifact {} must use an absolute https URL", artifact.id),
        ));
    }
    if !versions.contains(&(artifact.engine.as_str(), artifact.version.as_str())) {
        return Err(invalid(
            "version",
            format!(
                "artifact {} targets unknown version {} {}",
                artifact.id, artifact.engine, artifact.version
            ),
        ));
    }
    if let Some(glibc) = &artifact.min_glibc {
        parse_glibc(glibc).ok_or_else(|| {
            invalid(
                "min_glibc",
                format!("artifact {} has malformed min_glibc {glibc:?}", artifact.id),
            )
        })?;
    }
    Ok(())
}

/// Parses a `MAJOR.MINOR` glibc floor such as `2.38`.
pub fn parse_glibc(value: &str) -> Option<(u32, u32)> {
    let (major, minor) = value.split_once('.')?;
    let major = major.parse::<u32>().ok()?;
    let minor = minor.parse::<u32>().ok()?;
    Some((major, minor))
}

fn parse_semver(tag: &str) -> Result<Version> {
    Version::parse(tag).map_err(|_| {
        invalid(
            "version",
            format!("version {tag:?} is not a semver triple such as 18.7.0"),
        )
    })
}

pub(crate) fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::dto::tests::sample_document;

    #[test]
    fn valid_document_passes() {
        validate_document(&sample_document()).expect("valid");
    }

    #[test]
    fn schema_version_mismatch_is_rejected() {
        for version in [0, CATALOG_SCHEMA_VERSION + 1] {
            let mut document = sample_document();
            document.schema_version = version;
            assert!(validate_document(&document).is_err(), "{version}");
        }
    }

    #[test]
    fn source_must_be_https() {
        for url in ["http://example.test/a.json", "example.test/a.json", ""] {
            let mut document = sample_document();
            document.source_url = url.to_owned();
            assert!(validate_document(&document).is_err(), "{url}");
        }
    }

    #[test]
    fn duplicate_artifact_ids_are_rejected() {
        let mut document = sample_document();
        document.artifacts.push(document.artifacts[0].clone());
        let err = validate_document(&document).expect_err("duplicate id");
        assert!(err.to_string().contains("duplicate artifact id"));
    }

    #[test]
    fn versions_must_be_newest_first() {
        let mut document = sample_document();
        document.versions.push(crate::catalog::dto::CatalogVersion {
            engine: "postgresql".to_owned(),
            version: "19.0.0".to_owned(),
            major_line: "19".to_owned(),
            release_date: None,
            eol: false,
            line: Some("19".to_owned()),
            status: None,
            license: None,
        });
        let err = validate_document(&document).expect_err("out of order");
        assert!(err.to_string().contains("not ordered newest first"));
    }

    #[test]
    fn artifact_must_point_at_a_known_version() {
        let mut document = sample_document();
        document.artifacts[0].version = "99.0.0".to_owned();
        assert!(validate_document(&document).is_err());
    }

    #[test]
    fn artifact_checks_reject_corruption() {
        let cases: [&dyn Fn(&mut CatalogArtifact); 3] = [
            &|a| a.sha256 = "ABC".repeat(21),
            &|a| a.size_bytes = 0,
            &|a| a.url = "http://example.test/x.tar.gz".to_owned(),
        ];
        for mutate in cases {
            let mut document = sample_document();
            mutate(&mut document.artifacts[0]);
            assert!(validate_document(&document).is_err());
        }
    }

    #[test]
    fn glibc_floors_parse() {
        assert_eq!(parse_glibc("2.38"), Some((2, 38)));
        assert_eq!(parse_glibc("2.28"), Some((2, 28)));
        assert_eq!(parse_glibc("2"), None);
        assert_eq!(parse_glibc("x.y"), None);
    }

    #[test]
    fn tolerant_parser_skips_unknown_engine_and_unknown_format_keeping_known_entries() {
        let mut document = sample_document();
        // Add an unknown engine version and two artifacts belonging to it.
        document.versions.push(crate::catalog::dto::CatalogVersion {
            engine: "mongodb".to_owned(),
            version: "8.0.1".to_owned(),
            major_line: "8".to_owned(),
            release_date: None,
            eol: false,
            line: Some("8.0".to_owned()),
            status: None,
            license: Some("SSPLv1".to_owned()),
        });
        document.artifacts.push(CatalogArtifact {
            id: "mongodb-8.0.1-linux-x86_64".to_owned(),
            engine: "mongodb".to_owned(),
            version: "8.0.1".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            url: "https://example.test/mongodb-8.0.1.tgz".to_owned(),
            sha256: crate::catalog::dto::tests::sha(),
            size_bytes: 100,
            format: "tar.gz".to_owned(),
            strip_prefix: None,
            min_glibc: None,
            variant: None,
        });
        // Add an unknown archive format on a known engine.
        document.artifacts.push(CatalogArtifact {
            id: "postgresql-18.7.0-linux-x86_64-rpm".to_owned(),
            engine: "postgresql".to_owned(),
            version: "18.7.0".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            url: "https://example.test/postgresql-18.7.0.rpm".to_owned(),
            sha256: crate::catalog::dto::tests::sha(),
            size_bytes: 200,
            format: "rpm".to_owned(),
            strip_prefix: None,
            min_glibc: None,
            variant: None,
        });

        // Strict validation (used by CI / xtask) must still reject unknown entries.
        assert!(validate_document(&document).is_err());

        // Tolerant validation skips the unknown engine version, its artifact, and the unknown format artifact.
        let counts = validate_document_tolerant(&document).expect("tolerant validation");
        assert_eq!(counts.skipped_versions(), 1);
        assert_eq!(counts.skipped_artifacts(), 2);
        assert_eq!(counts.total(), 3);

        let (snapshot, snap_counts) = document
            .into_snapshot_tolerant(SystemTime::UNIX_EPOCH, CatalogStatus::Fresh)
            .expect("tolerant snapshot");
        assert_eq!(snap_counts, counts);
        assert_eq!(snapshot.versions().len(), 1);
        assert_eq!(snapshot.versions()[0].engine(), EngineKey::Postgresql);
        assert_eq!(snapshot.artifacts().len(), 1);
        assert_eq!(snapshot.artifacts()[0].id(), "postgresql-18.7.0-linux-x86_64");
    }

    #[test]
    fn tolerant_parser_still_rejects_malformed_known_engine_entries() {
        let mut bad_sha = sample_document();
        bad_sha.artifacts[0].sha256 = "not-a-sha".to_owned();
        assert!(validate_document_tolerant(&bad_sha).is_err());

        let mut bad_url = sample_document();
        bad_url.artifacts[0].url = "http://insecure.test/pg.tar.gz".to_owned();
        assert!(validate_document_tolerant(&bad_url).is_err());

        let mut bad_major = sample_document();
        bad_major.versions[0].major_line = "99".to_owned();
        assert!(validate_document_tolerant(&bad_major).is_err());
    }
}
