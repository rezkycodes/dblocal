use std::path::Path;
use std::time::SystemTime;

use dblocal_core::catalog::{CatalogArtifact, CatalogDocument, CatalogVersion, validate_document};

#[cfg(test)]
use super::sources::find_tracked;
use super::sources::{
    self, CATALOG_SOURCE_URL, CATALOG_V2_SOURCE_URL, HttpSource, MysqlBuild, Source,
    TrackedVersion, WAVE_ONE_VARIANTS, assemble, mariadb_api_url, mysql_builds_for,
};
use super::validate::{COMPAT_V2_SCHEMA_VERSION, validate_any_schema};
use dblocal_core::model::EngineKey;

/// Which engines to fetch from upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSelection {
    /// PostgreSQL versions to include, with their picker metadata.
    pub postgresql: Vec<TrackedVersion>,
    /// Valkey versions to include.
    pub valkey: Vec<TrackedVersion>,
    /// Redis versions to include (resolved from `packages.redis.io/deb`).
    pub redis: Vec<TrackedVersion>,
    /// Whether to resolve the latest stable RustFS release `>= 1.0.6`.
    pub rustfs: bool,
    /// MariaDB releases to include, as `(api_url, tracked)`.
    pub mariadb: Vec<(String, TrackedVersion)>,
    /// MySQL builds to download and hash, as `(build, tracked)`.
    ///
    /// Upstream publishes no SHA-256 for MySQL, so the builder downloads each
    /// archive once and records the computed hash.
    pub mysql: Vec<(MysqlBuild, TrackedVersion)>,
}

impl BuildSelection {
    /// The full tracked catalog: every engine's [`sources::tracked`] table.
    pub fn wave_one() -> Self {
        let postgresql = sources::tracked_for_engine(EngineKey::Postgresql).to_vec();
        let valkey = sources::tracked_for_engine(EngineKey::Valkey).to_vec();
        let redis = sources::tracked_for_engine(EngineKey::Redis).to_vec();
        let mariadb = sources::tracked_for_engine(EngineKey::Mariadb)
            .iter()
            .map(|tracked| (mariadb_api_url(tracked.version), *tracked))
            .collect();
        let mysql = sources::tracked_for_engine(EngineKey::Mysql)
            .iter()
            .flat_map(|tracked| {
                mysql_builds_for(tracked)
                    .into_iter()
                    .map(move |build| (build, *tracked))
            })
            .collect();
        Self {
            postgresql,
            valkey,
            redis,
            rustfs: true,
            mariadb,
            mysql,
        }
    }

    /// A narrower selection: the named tags only, for offline tests and patches.
    ///
    /// Unknown tags are a build error, so a typo never silently drops a version.
    /// Only the tests narrow a selection today; the production build always
    /// takes the whole tracked table.
    #[cfg(test)]
    pub fn for_tags(
        postgresql: &[&str],
        valkey: &[&str],
        mariadb: &[&str],
        mysql: &[&str],
    ) -> Result<Self, String> {
        let pick = |engine: EngineKey, tags: &[&str]| -> Result<Vec<TrackedVersion>, String> {
            let table = sources::tracked_for_engine(engine);
            tags.iter()
                .map(|tag| {
                    find_tracked(table, tag).copied().ok_or_else(|| {
                        format!("{} is not a tracked version for {}", tag, engine.as_str())
                    })
                })
                .collect()
        };
        let mariadb_tracked = pick(EngineKey::Mariadb, mariadb)?;
        let mysql_tracked = pick(EngineKey::Mysql, mysql)?;
        Ok(Self {
            postgresql: pick(EngineKey::Postgresql, postgresql)?,
            valkey: pick(EngineKey::Valkey, valkey)?,
            redis: Vec::new(),
            rustfs: false,
            mariadb: mariadb_tracked
                .into_iter()
                .map(|tracked| (mariadb_api_url(tracked.version), tracked))
                .collect(),
            mysql: mysql_tracked
                .into_iter()
                .flat_map(|tracked| {
                    mysql_builds_for(&tracked)
                        .into_iter()
                        .map(move |build| (build, tracked))
                })
                .collect(),
        })
    }
}

/// Builds the bundled catalog and writes it atomically (plus an optional
/// derived schema-2 file for released `v0.1.3`/`v0.1.4` clients).
///
/// `generated_at` is honest without daily churn: it is today's UTC date only
/// when the rest of the document actually changed, otherwise the existing
/// file's stamp is kept (finding B4).
pub fn run_with_compat(output: &Path, compat_v2_output: Option<&Path>) -> Result<String, String> {
    build_and_write_with_compat(
        &http_source()?,
        &BuildSelection::wave_one(),
        output,
        compat_v2_output,
        SystemTime::now(),
    )
}

/// Engines published exclusively in schema v3 (excluded from derived v2).
const V3_ONLY_ENGINES: &[&str] = &["redis", "rustfs"];

/// Derives a schema-2 compatible catalog from a schema-3 catalog.
///
/// Drops every v3-only engine (`redis`, `rustfs`) version and artifact, drops
/// every `deb` artifact, clears the `license` field so it is omitted during
/// JSON serialization, sets `schema_version = 2`, and restores the v2
/// `source_url`.
pub fn derive_compat_v2(v3: &CatalogDocument) -> CatalogDocument {
    let versions = v3
        .versions
        .iter()
        .filter(|version| !V3_ONLY_ENGINES.contains(&version.engine.as_str()))
        .cloned()
        .map(|mut version| {
            version.license = None;
            version
        })
        .collect();
    let artifacts = v3
        .artifacts
        .iter()
        .filter(|artifact| {
            !V3_ONLY_ENGINES.contains(&artifact.engine.as_str()) && artifact.format != "deb"
        })
        .cloned()
        .collect();
    CatalogDocument {
        schema_version: COMPAT_V2_SCHEMA_VERSION,
        generated_at: None,
        source_url: CATALOG_V2_SOURCE_URL.to_owned(),
        versions,
        artifacts,
    }
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
#[cfg(test)]
pub fn build_and_write(
    source: &dyn Source,
    selection: &BuildSelection,
    output: &Path,
    now: SystemTime,
) -> Result<String, String> {
    build_and_write_with_compat(source, selection, output, None, now)
}

/// Builds once, validates and atomically writes the v3 catalog, and when
/// `compat_v2_output` is set also derives, validates and atomically writes the
/// schema-2 catalog with its own independent `generated_at` stability check.
pub fn build_and_write_with_compat(
    source: &dyn Source,
    selection: &BuildSelection,
    output: &Path,
    compat_v2_output: Option<&Path>,
    now: SystemTime,
) -> Result<String, String> {
    let existing = std::fs::read_to_string(output)
        .ok()
        .and_then(|text| serde_json::from_str::<CatalogDocument>(&text).ok());
    let mut document = build_with_existing(source, selection, None, existing.as_ref())?;
    let today = format!("{}T00:00:00Z", utc_date(now));
    document.generated_at = Some(match read_stamp_if_unchanged(output, &document) {
        Some(existing) => existing,
        None => today.clone(),
    });
    validate_document(&document)
        .map_err(|err| format!("generated catalog failed validation: {err}"))?;
    write_atomic(output, &document)
        .map_err(|err| format!("could not write {}: {err}", output.display()))?;

    if let Some(v2_output) = compat_v2_output {
        let mut v2 = derive_compat_v2(&document);
        v2.generated_at = Some(match read_stamp_if_unchanged(v2_output, &v2) {
            Some(existing) => existing,
            None => today,
        });
        validate_any_schema(&v2)
            .map_err(|err| format!("generated compat v2 catalog failed validation: {err}"))?;
        write_atomic(v2_output, &v2)
            .map_err(|err| format!("could not write {}: {err}", v2_output.display()))?;
    }

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
#[cfg(test)]
pub fn build(
    source: &dyn Source,
    selection: &BuildSelection,
    generated_at: Option<String>,
) -> Result<CatalogDocument, String> {
    build_with_existing(source, selection, generated_at, None)
}

/// Assembles a catalog, optionally reusing verified `(engine, version, url)`
/// entries from `existing` to avoid re-downloading unchanged packages.
pub fn build_with_existing(
    source: &dyn Source,
    selection: &BuildSelection,
    generated_at: Option<String>,
    existing: Option<&CatalogDocument>,
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
        let (v, a) = sources::postgresql_from_releases(&releases, &selection.postgresql, source)
            .map_err(|err| err.to_string())?;
        versions.extend(v);
        artifacts.extend(a);
    }

    let variants: Vec<&str> = WAVE_ONE_VARIANTS.to_vec();
    for tracked in &selection.valkey {
        let (v, a) = sources::valkey_from_sidecars(tracked, &variants, source)
            .map_err(|err| err.to_string())?;
        versions.extend(v);
        artifacts.extend(a);
    }

    for tracked in &selection.redis {
        let (v, a) =
            sources::redis_from_apt(tracked, source, existing).map_err(|err| err.to_string())?;
        versions.extend(v);
        artifacts.extend(a);
    }

    if selection.rustfs {
        let releases_text = source
            .get_text(sources::RUSTFS_RELEASES_URL)
            .map_err(|err| err.to_string())?;
        let releases: Vec<sources::RustfsRelease> = serde_json::from_str(&releases_text)
            .map_err(|err| format!("RustFS releases JSON: {err}"))?;
        let (v, a) = sources::rustfs_from_releases(&releases, source, existing)
            .map_err(|err| err.to_string())?;
        versions.extend(v);
        artifacts.extend(a);
    }

    for (api_url, tracked) in &selection.mariadb {
        let release = sources::mariadb_release_from_api(source, api_url, tracked.version)
            .map_err(|err| err.to_string())?;
        let (v, a) = sources::mariadb_from_api(&release, tracked).map_err(|err| err.to_string())?;
        versions.extend(v);
        artifacts.extend(a);
    }

    for (spec, tracked) in &selection.mysql {
        let version = tracked.version;
        let artifact = match sources::mysql_artifact(spec, version, source) {
            Ok(artifact) => artifact,
            // Oracle drops the macOS x86_64 tarball on some releases; that
            // target is simply not offered for this version.
            Err(_) if sources::mysql_target_optional(spec) => continue,
            Err(err) => return Err(err.to_string()),
        };
        if !versions
            .iter()
            .any(|entry| entry.engine == "mysql" && entry.version == version)
        {
            versions.push(CatalogVersion {
                engine: "mysql".to_owned(),
                version: version.to_owned(),
                major_line: version.split('.').next().unwrap_or(version).to_owned(),
                release_date: None,
                eol: tracked.eol,
                line: Some(tracked.line.to_owned()),
                status: tracked.status.map(|status| status.as_str().to_owned()),
                license: None,
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
    use dblocal_core::model::VersionStatus;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::time::Duration;

    #[test]
    fn for_tags_narrows_to_known_versions_and_rejects_typos() {
        let selection =
            BuildSelection::for_tags(&["18.7.0"], &["9.2.0"], &["11.8.10"], &["8.4.13"])
                .expect("known tags");
        assert_eq!(selection.postgresql.len(), 1);
        assert_eq!(selection.postgresql[0].version, "18.7.0");
        assert_eq!(selection.postgresql[0].line, "18");
        assert_eq!(selection.valkey.len(), 1);
        assert_eq!(selection.mariadb.len(), 1);
        assert_eq!(selection.mariadb[0].1.line, "11.8");
        assert_eq!(
            selection.mysql.len(),
            mysql_builds_for(&selection.mysql[0].1).len()
        );

        let err = BuildSelection::for_tags(&["99.0.0"], &[], &[], &[]).expect_err("unknown");
        assert!(err.contains("99.0.0"), "{err}");
    }

    /// One bare tracked version, for the local-server test in the child module.
    pub(super) fn fixture_tracked(version: &'static str, line: &'static str) -> TrackedVersion {
        TrackedVersion {
            version,
            line,
            eol: false,
            status: None,
        }
    }

    /// The versions the recorded fixtures cover: the old wave-one set, tagged
    /// with picker metadata the way the real tracked table does.
    fn fixture_selection() -> BuildSelection {
        let pg = |version: &'static str, line: &'static str| TrackedVersion {
            version,
            line,
            eol: false,
            status: None,
        };
        let tracked = |version: &'static str, line: &'static str| TrackedVersion {
            version,
            line,
            eol: false,
            status: Some(VersionStatus::Lts),
        };
        let valkey = TrackedVersion {
            version: "9.2.0",
            line: "9.2",
            eol: false,
            status: None,
        };
        let mysql = TrackedVersion {
            version: "8.4.11",
            line: "8.4",
            eol: false,
            status: Some(VersionStatus::Lts),
        };
        BuildSelection {
            postgresql: vec![pg("18.7.0", "18"), pg("17.12.0", "17"), pg("16.16.0", "16")],
            valkey: vec![valkey],
            redis: Vec::new(),
            rustfs: false,
            mariadb: vec![
                (
                    sources::mariadb_api_url("11.8.10"),
                    tracked("11.8.10", "11.8"),
                ),
                (
                    sources::mariadb_api_url("11.4.14"),
                    tracked("11.4.14", "11.4"),
                ),
            ],
            mysql: mysql_builds_for(&mysql)
                .into_iter()
                .map(|build| (build, mysql))
                .collect(),
        }
    }

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

            // MariaDB: the fixture returns a null size for the server tarballs,
            // so the builder's HEAD lookup is served from here. Each published
            // version gets its own release body.
            for (fixture, version) in [
                ("mariadb-release.json", "11.8.10"),
                ("mariadb-release-11.4.json", "11.4.14"),
            ] {
                let body = std::fs::read_to_string(format!("{base}/{fixture}"))
                    .expect("mariadb release fixture");
                text.insert(sources::mariadb_api_url(version), body);
            }
            for name in [
                "mariadb-11.8.10-linux-systemd-x86_64.tar.gz",
                "mariadb-11.8.10-winx64.zip",
                "mariadb-11.4.14-linux-systemd-x86_64.tar.gz",
                "mariadb-11.4.14-winx64.zip",
            ] {
                sizes.insert(
                    (
                        format!("https://example.test/mariadb/{name}"),
                        sources::ENCODING_IDENTITY.to_owned(),
                    ),
                    1024,
                );
            }

            // MySQL: every published build shares the tiny fixture archive; the
            // .md5 sidecar is computed from those exact bytes so the cross-check
            // the builder performs is genuinely exercised.
            let mysql_bytes =
                std::fs::read(format!("{base}/mysql-archive.bin")).expect("mysql archive fixture");
            let mysql_md5 = sources::mysql_md5_hex(&mysql_bytes);
            let mut mysql_bytes_map = BTreeMap::new();
            for (build, _) in fixture_selection().mysql {
                let url = format!("{}/{}", build.base_url, build.file_name);
                text.insert(
                    format!("{url}.md5"),
                    format!("{mysql_md5}  {}\n", build.file_name),
                );
                mysql_bytes_map.insert(url, mysql_bytes.clone());
            }

            Self {
                text,
                text_by_encoding,
                bytes: mysql_bytes_map,
                sizes,
            }
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
            &fixture_selection(),
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
        build_and_write(&source, &fixture_selection(), &output, now).expect("first");
        let first: CatalogDocument =
            serde_json::from_str(&std::fs::read_to_string(&output).expect("read")).expect("parse");
        assert_eq!(first.generated_at.as_deref(), Some("2027-01-15T00:00:00Z"));

        // Same upstream data, later day: keep the original stamp, no churn.
        let later = now + Duration::from_secs(7 * 24 * 60 * 60);
        build_and_write(&source, &fixture_selection(), &output, later).expect("second");
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
        build_and_write(&source, &fixture_selection(), &output, first_now).expect("first");

        // A different selection changes the document, so the stamp must move.
        let narrower = BuildSelection {
            postgresql: vec![TrackedVersion {
                version: "18.7.0",
                line: "18",
                eol: false,
                status: None,
            }],
            valkey: Vec::new(),
            redis: Vec::new(),
            rustfs: false,
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
            &fixture_selection(),
            Some("2026-11-20T00:00:00Z".to_owned()),
        )
        .expect("build wave one");
        dblocal_core::catalog::validate_document(&document).expect("valid");
        // 3 PostgreSQL lines + Valkey + 2 MariaDB + MySQL (one version, four
        // builds across OS/arch).
        assert_eq!(document.versions.len(), 7);
        // 18 PostgreSQL + 2 Valkey + (2 MariaDB versions x 2 targets) + 4 MySQL.
        assert_eq!(document.artifacts.len(), 28);

        let json = serde_json::to_string_pretty(&document).expect("encode");
        let again = build(
            &source,
            &fixture_selection(),
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
            &fixture_selection(),
            Some("2026-11-20T00:00:00Z".to_owned()),
        )
        .expect("build");
        let summary = render_summary(&document);
        for needle in [
            "postgresql\t18.7.0\t6\t",
            "postgresql\t17.12.0\t6\t",
            "postgresql\t16.16.0\t6\t",
            "valkey\t9.2.0\t2\t",
            "mariadb\t11.8.10\t2\t",
            "mariadb\t11.4.14\t2\t",
            "mysql\t8.4.11\t4\t",
        ] {
            assert!(summary.contains(needle), "missing {needle} in {summary}");
        }
    }

    #[test]
    fn mariadb_and_mysql_selections_reach_the_assembled_document() {
        // `wave_one()` already serves the MariaDB fixtures and every MySQL build
        // from `mysql-archive.bin` with a matching `.md5`, so this proves the two
        // adapters reach the assembled document without hand-built URLs.
        let source = FixtureSource::wave_one();
        let tracked = TrackedVersion {
            version: "8.4.11",
            line: "8.4",
            eol: false,
            status: Some(VersionStatus::Lts),
        };
        let mariadb = TrackedVersion {
            version: "11.8.10",
            line: "11.8",
            eol: false,
            status: Some(VersionStatus::Lts),
        };
        let selection = BuildSelection {
            postgresql: Vec::new(),
            valkey: Vec::new(),
            redis: Vec::new(),
            rustfs: false,
            mariadb: vec![(sources::mariadb_api_url("11.8.10"), mariadb)],
            mysql: mysql_builds_for(&tracked)
                .into_iter()
                .map(|build| (build, tracked))
                .collect(),
        };
        let document = build(&source, &selection, None).expect("build");
        dblocal_core::catalog::validate_document(&document).expect("valid");
        assert!(document.versions.iter().any(|v| v.engine == "mariadb"));
        assert!(document.versions.iter().any(|v| v.engine == "mysql"));

        let meta: Value = serde_json::from_str(
            &std::fs::read_to_string(format!(
                "{}/tests/fixtures/mysql-sidecar.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .expect("meta"),
        )
        .expect("parse meta");
        let mysql: Vec<_> = document
            .artifacts
            .iter()
            .filter(|a| a.engine == "mysql")
            .collect();
        assert_eq!(mysql.len(), 4, "one artifact per published build");
        for artifact in &mysql {
            // Each build carries the computed hash of the shared fixture bytes.
            assert_eq!(artifact.sha256, meta["sha256"].as_str().expect("sha256"));
            assert!(
                artifact
                    .strip_prefix
                    .as_deref()
                    .is_some_and(|p| p.starts_with("mysql-8.4.11-")),
                "strip prefix must be the real archive root: {:?}",
                artifact.strip_prefix
            );
        }
    }

    #[test]
    fn deriving_v2_from_committed_v3_equals_committed_v2_ignoring_generated_at() {
        let v3_path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../catalog/engines-v3.json"
        ));
        let v2_path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../catalog/engines-v2.json"
        ));
        let v3_doc: CatalogDocument =
            serde_json::from_str(&std::fs::read_to_string(v3_path).expect("read v3"))
                .expect("parse v3");
        let v2_committed_text = std::fs::read_to_string(v2_path).expect("read v2");
        let v2_committed: CatalogDocument =
            serde_json::from_str(&v2_committed_text).expect("parse v2");

        let mut derived = derive_compat_v2(&v3_doc);
        derived.generated_at = v2_committed.generated_at.clone();

        validate_any_schema(&derived).expect("derived v2 must validate");
        assert_eq!(derived, v2_committed);

        let mut derived_json = serde_json::to_string_pretty(&derived).expect("encode derived");
        derived_json.push('\n');
        assert_eq!(
            derived_json, v2_committed_text,
            "derived v2 JSON serialization must match committed engines-v2.json byte-for-byte"
        );
    }

    #[test]
    fn build_and_write_with_compat_writes_both_and_preserves_stamps_independently() {
        let source = FixtureSource::wave_one();
        let dir = tempfile::tempdir().expect("tempdir");
        let v3_out = dir.path().join("catalog/engines-v3.json");
        let v2_out = dir.path().join("catalog/engines-v2.json");
        let day1 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000); // 2027-01-15
        let day2 = day1 + Duration::from_secs(86_400); // 2027-01-16

        build_and_write_with_compat(&source, &fixture_selection(), &v3_out, Some(&v2_out), day1)
            .expect("first write");
        let v3_first: CatalogDocument =
            serde_json::from_str(&std::fs::read_to_string(&v3_out).expect("read v3")).expect("v3");
        let v2_first: CatalogDocument =
            serde_json::from_str(&std::fs::read_to_string(&v2_out).expect("read v2")).expect("v2");
        assert_eq!(v3_first.schema_version, 3);
        assert_eq!(v2_first.schema_version, 2);
        assert_eq!(v2_first.source_url, CATALOG_V2_SOURCE_URL);
        assert_eq!(
            v3_first.generated_at.as_deref(),
            Some("2027-01-15T00:00:00Z")
        );
        assert_eq!(
            v2_first.generated_at.as_deref(),
            Some("2027-01-15T00:00:00Z")
        );

        // Second run on a later day with unchanged data keeps both stamps.
        build_and_write_with_compat(&source, &fixture_selection(), &v3_out, Some(&v2_out), day2)
            .expect("second write");
        let v3_second: CatalogDocument =
            serde_json::from_str(&std::fs::read_to_string(&v3_out).expect("read v3")).expect("v3");
        let v2_second: CatalogDocument =
            serde_json::from_str(&std::fs::read_to_string(&v2_out).expect("read v2")).expect("v2");
        assert_eq!(v3_second.generated_at, v3_first.generated_at);
        assert_eq!(v2_second.generated_at, v2_first.generated_at);
    }
}

#[cfg(test)]
#[path = "local_server_tests.rs"]
mod local_server_tests;
