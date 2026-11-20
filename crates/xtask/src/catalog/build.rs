use std::path::Path;

use dblocal_core::catalog::{CatalogArtifact, CatalogDocument, CatalogVersion, validate_document};

use super::sources::{
    self, CATALOG_SOURCE_URL, HttpSource, MysqlBuild, Source, WAVE_ONE_POSTGRESQL, WAVE_ONE_VALKEY,
    assemble,
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
pub fn run(output: &Path) -> Result<String, String> {
    let token = std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|value| !value.is_empty());
    let source = HttpSource::new(token).map_err(|err| err.to_string())?;
    write_built(
        &source,
        &BuildSelection::wave_one(),
        Some("2026-11-20T00:00:00Z".to_owned()),
        output,
    )
}

/// Fetches, validates and writes the catalog through an injected source.
///
/// Split from [`run`] so the whole write pipeline can be exercised offline with
/// recorded fixtures while `run` only wires up the real HTTP client.
pub fn write_built(
    source: &dyn Source,
    selection: &BuildSelection,
    generated_at: Option<String>,
    output: &Path,
) -> Result<String, String> {
    let document = build(source, selection, generated_at)?;
    validate_document(&document)
        .map_err(|err| format!("generated catalog failed validation: {err}"))?;
    write_atomic(output, &document)
        .map_err(|err| format!("could not write {}: {err}", output.display()))?;
    Ok(render_summary(&document))
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
        let (v, a) =
            sources::valkey_from_sidecars(version, source).map_err(|err| err.to_string())?;
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

    struct FixtureSource {
        text: BTreeMap<String, String>,
        bytes: BTreeMap<String, Vec<u8>>,
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
            let vk_sidecars: BTreeMap<String, String> = serde_json::from_str(
                &std::fs::read_to_string(format!("{base}/valkey-sidecars.json"))
                    .expect("valkey sidecars"),
            )
            .expect("parse sidecars");

            let mut text = BTreeMap::new();
            text.insert(
                "https://api.github.com/repos/theseus-rs/postgresql-binaries/releases?per_page=100"
                    .to_owned(),
                releases,
            );
            for (path, content) in pg_sidecars {
                text.insert(format!("https://example.test{path}"), content);
            }
            for (path, content) in vk_sidecars {
                let url = format!(
                    "https://download.valkey.io/releases{}",
                    path.strip_prefix("/valkey").expect("prefix")
                );
                text.insert(url, content);
            }
            Self {
                text,
                bytes: BTreeMap::new(),
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
        fn get_text(&self, url: &str) -> Result<String, sources::SourceError> {
            self.text
                .get(url)
                .cloned()
                .ok_or_else(|| sources::SourceError::new(format!("fixture has no text for {url}")))
        }

        fn get_bytes(&self, url: &str) -> Result<Vec<u8>, sources::SourceError> {
            self.bytes
                .get(url)
                .cloned()
                .ok_or_else(|| sources::SourceError::new(format!("fixture has no bytes for {url}")))
        }
    }

    #[test]
    fn write_built_writes_a_valid_file_with_a_trailing_newline() {
        let source = FixtureSource::wave_one();
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("nested/catalog/engines.json");
        let summary = write_built(
            &source,
            &BuildSelection::wave_one(),
            Some("2026-11-20T00:00:00Z".to_owned()),
            &output,
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
        assert_eq!(document.artifacts.len(), 22);

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
            "valkey\t9.2.0\t4\t",
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
mod local_server_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    struct EphemeralSource {
        base: String,
    }

    impl Source for EphemeralSource {
        fn get_text(&self, url: &str) -> Result<String, sources::SourceError> {
            let bytes = self.get_bytes(url)?;
            String::from_utf8(bytes)
                .map_err(|err| sources::SourceError::new(format!("{url} is not UTF-8: {err}")))
        }

        fn get_bytes(&self, url: &str) -> Result<Vec<u8>, sources::SourceError> {
            let mapped = if url.starts_with(
                "https://api.github.com/repos/theseus-rs/postgresql-binaries/releases?per_page=100",
            ) {
                format!("{}/postgres-releases.json", self.base)
            } else if let Some(path) = url.strip_prefix("https://example.test/pg/") {
                format!("{}/pg/{path}", self.base)
            } else if let Some(path) = url.strip_prefix("https://download.valkey.io/releases/") {
                format!("{}/valkey/{path}", self.base)
            } else {
                return Err(sources::SourceError::new(format!(
                    "no local mapping for {url}"
                )));
            };

            let client = reqwest::blocking::Client::new();
            let response = client
                .get(&mapped)
                .send()
                .map_err(|err| sources::SourceError::new(format!("GET {mapped} failed: {err}")))?;
            if !response.status().is_success() {
                return Err(sources::SourceError::new(format!(
                    "GET {mapped} returned HTTP {}",
                    response.status().as_u16()
                )));
            }
            response.bytes().map(|bytes| bytes.to_vec()).map_err(|err| {
                sources::SourceError::new(format!("GET {mapped} body failed: {err}"))
            })
        }
    }

    fn serve_fixtures() -> String {
        let base = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
        let releases = std::fs::read(format!("{base}/postgres-releases.json")).expect("releases");
        let pg_sidecars: std::collections::BTreeMap<String, String> = serde_json::from_str(
            &std::fs::read_to_string(format!("{base}/postgres-sidecars.json"))
                .expect("pg sidecars"),
        )
        .expect("parse pg sidecars");
        let vk_sidecars: std::collections::BTreeMap<String, String> = serde_json::from_str(
            &std::fs::read_to_string(format!("{base}/valkey-sidecars.json")).expect("vk sidecars"),
        )
        .expect("parse vk sidecars");

        let mut routes: std::collections::BTreeMap<String, Vec<u8>> =
            std::collections::BTreeMap::new();
        routes.insert("/postgres-releases.json".to_owned(), releases);
        for (path, body) in pg_sidecars {
            routes.insert(path, body.into_bytes());
        }
        for (path, body) in vk_sidecars {
            routes.insert(path, body.into_bytes());
        }

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let read = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let (status, body) = routes
                    .get(path)
                    .map(|body| ("200 OK", body.clone()))
                    .unwrap_or_else(|| ("404 Not Found", b"missing".to_vec()));
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    #[test]
    fn build_reads_recorded_fixtures_from_a_local_server_on_port_0() {
        let base = serve_fixtures();
        let source = EphemeralSource { base };
        let selection = BuildSelection {
            postgresql: vec!["18.7.0".to_owned()],
            valkey: Some("9.2.0".to_owned()),
            mariadb: Vec::new(),
            mysql: Vec::new(),
        };
        let document = build(&source, &selection, Some("2026-11-20T00:00:00Z".to_owned()))
            .expect("build from local fixtures");
        dblocal_core::catalog::validate_document(&document).expect("valid");
        assert_eq!(document.versions.len(), 2);
        assert_eq!(document.artifacts.len(), 10);
    }
}
