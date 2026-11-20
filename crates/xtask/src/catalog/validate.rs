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
            "/../../catalog/engines.json"
        ));
        let summary = run(path).expect("bundled catalog must validate");
        assert!(summary.starts_with("catalog valid: schema 1, "));
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
