use std::path::Path;
use std::time::SystemTime;

use dblocal_core::catalog::{CatalogArtifact, CatalogDocument, CatalogVersion, validate_document};

use super::sources::{
    self, CATALOG_SOURCE_URL, HttpSource, MysqlBuild, Source, WAVE_ONE_POSTGRESQL, WAVE_ONE_VALKEY,
    WAVE_ONE_VARIANTS, assemble,
};

/// Which engines to fetch from upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSelection {
    /// PostgreSQL tags to include.
    pub postgresql: Vec<String>,
    /// Valkey tag to include.
    pub valkey: Option<String>,
    /// MariaDB REST API releases to include, as `(api_url, version)`.
    pub mariadb: Vec<(String, String)>,
    /// MySQL builds to download and hash, as `(build, version)`.
    ///
    /// Upstream publishes no SHA-256 for MySQL, so the builder downloads each
    /// archive once and records the computed hash. Wave 1 leaves this empty.
    pub mysql: Vec<(MysqlBuild, String)>,
}

impl BuildSelection {
    /// The scope published in Wave 1: PostgreSQL and Valkey only.
    pub fn wave_one() -> Self {
        Self {
            postgresql: WAVE_ONE_POSTGRESQL
                .iter()
                .map(|tag| (*tag).to_owned())
                .collect(),
            valkey: Some(WAVE_ONE_VALKEY.to_owned()),
            mariadb: Vec::new(),
            mysql: Vec::new(),
        }
    }
}

/// Builds the bundled catalog and writes it atomically.
///
/// `generated_at` is honest without daily churn: it is today's UTC date only
/// when the rest of the document actually changed, otherwise the existing
/// file's stamp is kept (finding B4).
pub fn run(output: &Path) -> Result<String, String> {
    run_at(output, SystemTime::now())
}

/// [`run`] with an injectable clock, so the date behaviour is testable.
pub fn run_at(output: &Path, now: SystemTime) -> Result<String, String> {
    build_and_write(&http_source()?, &BuildSelection::wave_one(), output, now)
}

/// Builds the production HTTP source, reading `GITHUB_TOKEN` if present.
pub fn http_source() -> Result<HttpSource, String> {
    let token = std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|value| !value.is_empty());
    HttpSource::new(token).map_err(|err| err.to_string())
}

/// Builds, stamps and writes, keeping an existing `generated_at` when nothing
/// else changed.
pub fn build_and_write(
    source: &dyn Source,
    selection: &BuildSelection,
    output: &Path,
    now: SystemTime,
) -> Result<String, String> {
    let mut document = build(source, selection, None)?;
    let today = format!("{}T00:00:00Z", utc_date(now));
    document.generated_at = Some(match read_stamp_if_unchanged(output, &document) {
        Some(existing) => existing,
        None => today,
    });
    validate_document(&document)
        .map_err(|err| format!("generated catalog failed validation: {err}"))?;
    write_atomic(output, &document)
        .map_err(|err| format!("could not write {}: {err}", output.display()))?;
    Ok(render_summary(&document))
}

/// Returns the existing file's stamp when only the stamp and fetch time differ.
fn read_stamp_if_unchanged(output: &Path, fresh: &CatalogDocument) -> Option<String> {
    let text = std::fs::read_to_string(output).ok()?;
    let existing: CatalogDocument = serde_json::from_str(&text).ok()?;
    let stamp = existing.generated_at.clone()?;
    // Compare everything except `generated_at`.
    let same_versions = existing.versions == fresh.versions;
    let same_artifacts = existing.artifacts == fresh.artifacts;
    let same_meta =
        existing.schema_version == fresh.schema_version && existing.source_url == fresh.source_url;
    (same_versions && same_artifacts && same_meta).then_some(stamp)
}

/// Formats `time` as a UTC `YYYY-MM-DD` date.
///
/// Small civil-date conversion; avoids pulling a date crate into a build tool.
pub fn utc_date(time: SystemTime) -> String {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|age| age.as_secs())
        .unwrap_or_default();
    let days = (secs / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to `(y, m, d)`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

/// Assembles a catalog for the chosen engines from the given source.
///
/// Every artifact is verified before it is recorded: PostgreSQL and Valkey from
/// their `.sha256` sidecars, MariaDB from the REST API's `checksum.sha256sum`,
/// MySQL by downloading once and cross-checking the `.md5` sidecar.
pub fn build(
    source: &dyn Source,
    selection: &BuildSelection,
    generated_at: Option<String>,
) -> Result<CatalogDocument, String> {
    let mut versions: Vec<CatalogVersion> = Vec::new();
    let mut artifacts: Vec<CatalogArtifact> = Vec::new();

    if !selection.postgresql.is_empty() {
        let releases_text = source
            .get_text(
                "https://api.github.com/repos/theseus-rs/postgresql-binaries/releases?per_page=100",
            )
            .map_err(|err| err.to_string())?;
        let releases: Vec<sources::GithubRelease> = serde_json::from_str(&releases_text)
            .map_err(|err| format!("PostgreSQL releases JSON: {err}"))?;
        let wanted: Vec<&str> = selection.postgresql.iter().map(String::as_str).collect();
        let (v, a) = sources::postgresql_from_releases(&releases, &wanted, source)
            .map_err(|err| err.to_string())?;
        versions.extend(v);
        artifacts.extend(a);
    }

    if let Some(version) = &selection.valkey {
        let variants: Vec<&str> = WAVE_ONE_VARIANTS.to_vec();
        let (v, a) = sources::valkey_from_sidecars(version, &variants, source)
            .map_err(|err| err.to_string())?;
        versions.extend(v);
        artifacts.extend(a);
    }

    for (api_url, version) in &selection.mariadb {
        let text = source.get_text(api_url).map_err(|err| err.to_string())?;
        let release: sources::MariadbRelease =
            serde_json::from_str(&text).map_err(|err| format!("MariaDB release JSON: {err}"))?;
        let (v, a) = sources::mariadb_from_api(&release, version).map_err(|err| err.to_string())?;
        versions.extend(v);
        artifacts.extend(a);
    }

    for (spec, version) in &selection.mysql {
        let artifact =
            sources::mysql_artifact(spec, version, source).map_err(|err| err.to_string())?;
        if !versions
            .iter()
            .any(|entry| entry.engine == "mysql" && entry.version == *version)
        {
            versions.push(CatalogVersion {
                engine: "mysql".to_owned(),
                version: version.clone(),
                major_line: version.split('.').next().unwrap_or(version).to_owned(),
                release_date: None,
                eol: false,
            });
        }
        artifacts.push(artifact);
    }

    let mut document = assemble(versions, artifacts, generated_at);
    document.source_url = CATALOG_SOURCE_URL.to_owned();
    Ok(document)
}

/// Renders the summary table the W1-B report must show.
pub fn render_summary(document: &CatalogDocument) -> String {
    let mut out = String::new();
    out.push_str("engine\tversion\ttargets\ttotal_bytes\n");
    for version in &document.versions {
        let matching: Vec<&CatalogArtifact> = document
            .artifacts
            .iter()
            .filter(|artifact| {
                artifact.engine == version.engine && artifact.version == version.version
            })
            .collect();
        let total: u64 = matching.iter().map(|artifact| artifact.size_bytes).sum();
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            version.engine,
            version.version,
            matching.len(),
            total
        ));
    }
    out
}

fn write_atomic(path: &Path, document: &CatalogDocument) -> Result<(), std::io::Error> {
    let mut text = serde_json::to_string_pretty(document)
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temp, text.as_bytes())?;
    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::time::Duration;

    struct FixtureSource {
        text: BTreeMap<String, String>,
        text_by_encoding: BTreeMap<(String, String), String>,
        bytes: BTreeMap<String, Vec<u8>>,
        sizes: BTreeMap<(String, String), u64>,
    }

    impl FixtureSource {
        fn wave_one() -> Self {
            let base = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
            let releases = std::fs::read_to_string(format!("{base}/postgres-releases.json"))
                .expect("postgres releases fixture");
            let pg_sidecars: BTreeMap<String, String> = serde_json::from_str(
                &std::fs::read_to_string(format!("{base}/postgres-sidecars.json"))
                    .expect("postgres sidecars"),
            )
            .expect("parse sidecars");
            let vk_sidecars: BTreeMap<String, BTreeMap<String, String>> = serde_json::from_str(
                &std::fs::read_to_string(format!("{base}/valkey-sidecars.json"))
                    .expect("valkey sidecars"),
            )
            .expect("parse sidecars");

            let vk_heads: BTreeMap<String, BTreeMap<String, u64>> = serde_json::from_str(
                &std::fs::read_to_string(format!("{base}/valkey-heads.json"))
                    .expect("valkey heads"),
            )
            .expect("parse heads");

            let mut text = BTreeMap::new();
            let mut text_by_encoding = BTreeMap::new();
            let mut sizes = BTreeMap::new();
            text.insert(
                "https://api.github.com/repos/theseus-rs/postgresql-binaries/releases?per_page=100"
                    .to_owned(),
                releases,
            );
            for (path, content) in pg_sidecars {
                text.insert(format!("https://example.test{path}"), content);
            }
            for (path, encs) in vk_sidecars {
                let url = format!(
                    "https://download.valkey.io/releases{}",
                    path.strip_prefix("/valkey").expect("prefix")
                );
                for (enc, body) in encs {
                    text_by_encoding.insert((url.clone(), enc), body);
                }
            }
            for (path, encs) in vk_heads {
                let url = format!(
                    "https://download.valkey.io/releases{}",
                    path.strip_prefix("/valkey").expect("prefix")
                );
                for (enc, size) in encs {
                    sizes.insert((url.clone(), enc), size);
                }
            }
            Self {
                text,
                text_by_encoding,
                bytes: BTreeMap::new(),
                sizes,
            }
        }

        fn with_fixture(mut self, name: &str, url: &str) -> Self {
            let base = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
            let body = std::fs::read_to_string(format!("{base}/{name}")).expect("fixture");
            self.text.insert(url.to_owned(), body);
            self
        }

        fn with_binary(mut self, name: &str, url: &str) -> Self {
            let base = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
            let body = std::fs::read(format!("{base}/{name}")).expect("fixture");
            self.bytes.insert(url.to_owned(), body);
            self
        }
    }

    impl Source for FixtureSource {
        fn get_bytes(&self, url: &str) -> Result<Vec<u8>, sources::SourceError> {
            self.bytes
                .get(url)
                .cloned()
                .ok_or_else(|| sources::SourceError::new(format!("fixture has no bytes for {url}")))
        }

        fn get_text_with(
            &self,
            url: &str,
            accept_encoding: &str,
        ) -> Result<String, sources::SourceError> {
            if let Some(body) = self
                .text_by_encoding
                .get(&(url.to_owned(), accept_encoding.to_owned()))
            {
                return Ok(body.clone());
            }
            self.text
                .get(url)
                .cloned()
                .ok_or_else(|| sources::SourceError::new(format!("fixture has no text for {url}")))
        }

        fn head_size(&self, url: &str, accept_encoding: &str) -> Result<u64, sources::SourceError> {
            self.sizes
                .get(&(url.to_owned(), accept_encoding.to_owned()))
                .copied()
                .ok_or_else(|| {
                    sources::SourceError::new(format!("fixture has no HEAD size for {url}"))
                })
        }
    }

    #[test]
    fn build_and_write_writes_a_valid_file_with_a_trailing_newline() {
        let source = FixtureSource::wave_one();
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("nested/catalog/engines.json");
        let summary = build_and_write(
            &source,
            &BuildSelection::wave_one(),
            &output,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_132_800),
        )
        .expect("write");
        assert!(summary.contains("postgresql\t18.7.0\t6\t"));

        let text = std::fs::read_to_string(&output).expect("read back");
        assert!(text.ends_with('\n'), "missing trailing newline");
        let document: CatalogDocument = serde_json::from_str(&text).expect("parse");
        dblocal_core::catalog::validate_document(&document).expect("valid");

        // No temporary sibling is left behind.
        let siblings: Vec<String> = std::fs::read_dir(output.parent().expect("parent"))
            .expect("read_dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(siblings, vec!["engines.json".to_owned()]);
    }

    #[test]
    fn generated_at_is_kept_when_the_document_is_unchanged() {
        // Failing-first for B4: run() used to hard-code a date, so every future
        // build claimed the same day and a no-op refresh churned the file.
        let source = FixtureSource::wave_one();
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("catalog/engines.json");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000); // 2027-01-15

        // First write: no existing file, so today's UTC date is used.
        build_and_write(&source, &BuildSelection::wave_one(), &output, now).expect("first");
        let first: CatalogDocument =
            serde_json::from_str(&std::fs::read_to_string(&output).expect("read")).expect("parse");
        assert_eq!(first.generated_at.as_deref(), Some("2027-01-15T00:00:00Z"));

        // Same upstream data, later day: keep the original stamp, no churn.
        let later = now + Duration::from_secs(7 * 24 * 60 * 60);
        build_and_write(&source, &BuildSelection::wave_one(), &output, later).expect("second");
        let second: CatalogDocument =
            serde_json::from_str(&std::fs::read_to_string(&output).expect("read")).expect("parse");
        assert_eq!(second.generated_at.as_deref(), Some("2027-01-15T00:00:00Z"));
        assert_eq!(first.artifacts, second.artifacts);
    }

    #[test]
    fn generated_at_moves_forward_when_the_document_changes() {
        let source = FixtureSource::wave_one();
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("catalog/engines.json");
        let first_now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000); // 2027-01-15
        build_and_write(&source, &BuildSelection::wave_one(), &output, first_now).expect("first");

        // A different selection changes the document, so the stamp must move.
        let narrower = BuildSelection {
            postgresql: vec!["18.7.0".to_owned()],
            valkey: None,
            mariadb: Vec::new(),
            mysql: Vec::new(),
        };
        let later = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + 86_400);
        build_and_write(&source, &narrower, &output, later).expect("second");
        let document: CatalogDocument =
            serde_json::from_str(&std::fs::read_to_string(&output).expect("read")).expect("parse");
        assert_eq!(
            document.generated_at.as_deref(),
            Some("2027-01-16T00:00:00Z")
        );
        assert_eq!(document.versions.len(), 1);
    }

    #[test]
    fn utc_date_conversion_is_exact() {
        assert_eq!(utc_date(SystemTime::UNIX_EPOCH), "1970-01-01");
        // 2026-11-20T00:00:00Z
        let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_132_800);
        assert_eq!(utc_date(stamp), "2026-11-20");
        // 2028-02-29 is a leap day.
        let leap = SystemTime::UNIX_EPOCH + Duration::from_secs(1_835_395_200);
        assert_eq!(utc_date(leap), "2028-02-29");
    }

    #[test]
    fn wave_one_build_is_deterministic_and_valid() {
        let source = FixtureSource::wave_one();
        let document = build(
            &source,
            &BuildSelection::wave_one(),
            Some("2026-11-20T00:00:00Z".to_owned()),
        )
        .expect("build wave one");
        dblocal_core::catalog::validate_document(&document).expect("valid");
        assert_eq!(document.versions.len(), 4);
        // Wave 1 publishes Valkey jammy only: 18 PostgreSQL + 2 Valkey.
        assert_eq!(document.artifacts.len(), 20);

        let json = serde_json::to_string_pretty(&document).expect("encode");
        let again = build(
            &source,
            &BuildSelection::wave_one(),
            Some("2026-11-20T00:00:00Z".to_owned()),
        )
        .expect("build again");
        assert_eq!(json, serde_json::to_string_pretty(&again).expect("encode"));

        let value: Value = serde_json::from_str(&json).expect("parse");
        assert!(
            value["artifacts"]
                .as_array()
                .expect("array")
                .iter()
                .all(|a| a["url"].as_str().expect("url").starts_with("https://"))
        );
    }

    #[test]
    fn summary_lists_every_published_version() {
        let source = FixtureSource::wave_one();
        let document = build(
            &source,
            &BuildSelection::wave_one(),
            Some("2026-11-20T00:00:00Z".to_owned()),
        )
        .expect("build");
        let summary = render_summary(&document);
        for needle in [
            "postgresql\t18.7.0\t6\t",
            "postgresql\t17.12.0\t6\t",
            "postgresql\t16.16.0\t6\t",
            "valkey\t9.2.0\t2\t",
        ] {
            assert!(summary.contains(needle), "missing {needle} in {summary}");
        }
    }

    #[test]
    fn mariadb_and_mysql_selections_reach_the_assembled_document() {
        let mariadb_url = "https://example.test/mariadb/11.8.10";
        let meta: Value = serde_json::from_str(
            &std::fs::read_to_string(format!(
                "{}/tests/fixtures/mysql-sidecar.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .expect("meta"),
        )
        .expect("parse meta");
        let file_name = meta["file_name"].as_str().expect("file_name").to_owned();
        let archive_url = format!("https://example.test/mysql/{file_name}");
        let md5_url = format!("{archive_url}.md5");

        let mut source = FixtureSource::wave_one()
            .with_fixture("mariadb-release.json", mariadb_url)
            .with_text(&md5_url, meta["md5_text"].as_str().expect("md5"));
        source = source.with_binary("mysql-archive.bin", &archive_url);

        let selection = BuildSelection {
            postgresql: Vec::new(),
            valkey: None,
            mariadb: vec![(mariadb_url.to_owned(), "11.8.10".to_owned())],
            mysql: vec![(
                MysqlBuild {
                    file_name: file_name.clone(),
                    os: dblocal_core::platform::Os::Linux,
                    arch: dblocal_core::platform::Arch::X86_64,
                    format: dblocal_core::model::ArchiveFormat::TarXz,
                    base_url: "https://example.test/mysql".to_owned(),
                },
                "8.4.11".to_owned(),
            )],
        };
        let document = build(&source, &selection, None).expect("build");
        dblocal_core::catalog::validate_document(&document).expect("valid");
        assert!(document.versions.iter().any(|v| v.engine == "mariadb"));
        assert!(document.versions.iter().any(|v| v.engine == "mysql"));
        assert_eq!(
            document
                .artifacts
                .iter()
                .find(|a| a.engine == "mysql")
                .expect("mysql artifact")
                .sha256,
            meta["sha256"].as_str().expect("sha256")
        );
    }

    impl FixtureSource {
        fn with_text(mut self, url: &str, body: impl Into<String>) -> Self {
            self.text.insert(url.to_owned(), body.into());
            self
        }
    }
}

#[cfg(test)]
#[path = "local_server_tests.rs"]
mod local_server_tests;
