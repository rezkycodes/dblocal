use std::path::Path;

use dblocal_core::catalog::{CATALOG_SCHEMA_VERSION, CatalogDocument, validate_document};

/// Schema version supported by released `v0.1.3`/`v0.1.4` clients.
pub const COMPAT_V2_SCHEMA_VERSION: u32 = 2;

/// Validates a catalog document according to its declared `schema_version`.
///
/// - Schema 3: full v3 rules via [`validate_document`].
/// - Schema 2: v2 compatibility rules (no `redis` engine, no `deb` format, no
///   `license` field) plus every shared invariant from [`validate_document`].
/// - Any other schema version is rejected.
pub fn validate_any_schema(document: &CatalogDocument) -> Result<(), String> {
    match document.schema_version {
        CATALOG_SCHEMA_VERSION => validate_document(document).map_err(|err| err.to_string()),
        COMPAT_V2_SCHEMA_VERSION => validate_compat_v2(document),
        other => Err(format!(
            "unsupported schema version {other}, expected {COMPAT_V2_SCHEMA_VERSION} or {CATALOG_SCHEMA_VERSION}"
        )),
    }
}

fn validate_compat_v2(document: &CatalogDocument) -> Result<(), String> {
    for version in &document.versions {
        if version.engine == "redis" {
            return Err(format!(
                "schema 2 catalog must not contain redis version {}",
                version.version
            ));
        }
        if version.license.is_some() {
            return Err(format!(
                "schema 2 catalog must not contain license field on {} {}",
                version.engine, version.version
            ));
        }
    }
    for artifact in &document.artifacts {
        if artifact.engine == "redis" {
            return Err(format!(
                "schema 2 catalog must not contain redis artifact {}",
                artifact.id
            ));
        }
        if artifact.format == "deb" {
            return Err(format!(
                "schema 2 catalog must not contain deb artifact {}",
                artifact.id
            ));
        }
    }
    let mut upgraded = document.clone();
    upgraded.schema_version = CATALOG_SCHEMA_VERSION;
    validate_document(&upgraded).map_err(|err| err.to_string())
}

/// Validates a catalog file, fully offline.
pub fn run(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("could not read {}: {err}", path.display()))?;
    let document: CatalogDocument = serde_json::from_str(&text)
        .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
    validate_any_schema(&document).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(format!(
        "catalog valid: schema {}, {} versions, {} artifacts, source {}",
        document.schema_version,
        document.versions.len(),
        document.artifacts.len(),
        document.source_url
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_the_bundled_catalog() {
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../catalog/engines-v3.json"
        ));
        let summary = run(path).expect("bundled v3 catalog must validate");
        assert!(summary.starts_with("catalog valid: schema 3, "));

        let v2_path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../catalog/engines-v2.json"
        ));
        let v2_summary = run(v2_path).expect("compat v2 catalog must validate");
        assert!(v2_summary.starts_with("catalog valid: schema 2, "));
    }

    #[test]
    fn schema_2_validation_rejects_redis_deb_and_license() {
        let v2_path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../catalog/engines-v2.json"
        ));
        let base: CatalogDocument =
            serde_json::from_str(&std::fs::read_to_string(v2_path).expect("read v2"))
                .expect("parse v2");

        let mut with_license = base.clone();
        with_license.versions[0].license = Some("MIT".to_owned());
        let err = validate_any_schema(&with_license).expect_err("license rejected in v2");
        assert!(err.contains("license"), "{err}");

        let mut with_deb = base.clone();
        with_deb.artifacts[0].format = "deb".to_owned();
        let err = validate_any_schema(&with_deb).expect_err("deb rejected in v2");
        assert!(err.contains("deb"), "{err}");

        let mut with_redis = base;
        with_redis
            .versions
            .push(dblocal_core::catalog::CatalogVersion {
                engine: "redis".to_owned(),
                version: "7.4.12".to_owned(),
                major_line: "7".to_owned(),
                release_date: None,
                eol: false,
                line: Some("7.4".to_owned()),
                status: None,
                license: None,
            });
        let err = validate_any_schema(&with_redis).expect_err("redis rejected in v2");
        assert!(err.contains("redis"), "{err}");
    }

    #[test]
    fn frozen_v1_and_v2_catalogs_never_contain_redis_or_deb() {
        // Released v0.1.3/v0.1.4 clients reject unknown engine keys ("redis")
        // and unknown archive formats ("deb") across the entire file. Guard
        // both frozen catalog files so a future edit can never break old users.
        for (rel, expected_schema) in [
            ("../../catalog/engines.json", 1u32),
            ("../../catalog/engines-v2.json", 2u32),
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
            let text = std::fs::read_to_string(&path).expect("read frozen catalog");
            let doc: CatalogDocument = serde_json::from_str(&text).expect("parse frozen catalog");
            assert_eq!(doc.schema_version, expected_schema, "{rel}");
            assert!(
                !doc.versions.iter().any(|v| v.engine == "redis"),
                "{rel} must not contain redis versions"
            );
            assert!(
                !doc.artifacts
                    .iter()
                    .any(|a| a.engine == "redis" || a.format == "deb"),
                "{rel} must not contain redis or deb artifacts"
            );
        }
    }

    #[test]
    fn rejects_a_missing_or_malformed_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(run(&dir.path().join("missing.json")).is_err());

        let path = dir.path().join("bad.json");
        std::fs::write(&path, b"{ not json").expect("write");
        assert!(run(&path).is_err());
    }

    #[test]
    fn rejects_a_file_that_fails_validation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad.json");
        let document = serde_json::json!({
            "schema_version": 1,
            "source_url": "http://insecure.test/catalog.json",
            "versions": [],
            "artifacts": [],
        });
        std::fs::write(&path, document.to_string()).expect("write");
        assert!(run(&path).is_err());
    }
}
