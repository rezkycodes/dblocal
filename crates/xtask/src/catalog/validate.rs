use std::path::Path;

use dblocal_core::catalog::{CatalogDocument, validate_document};

/// Validates a catalog file, fully offline.
pub fn run(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("could not read {}: {err}", path.display()))?;
    let document: CatalogDocument = serde_json::from_str(&text)
        .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
    validate_document(&document).map_err(|err| format!("{}: {err}", path.display()))?;
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
        let summary = run(path).expect("bundled catalog must validate");
        assert!(summary.starts_with("catalog valid: schema 3, "));
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
