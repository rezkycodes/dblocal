use super::helpers::{hex, is_sha256_hex, parse_md5_sidecar, parse_sha256_sidecar};
use super::mariadb::{MariadbChecksum, MariadbFile};

use super::*;
use std::collections::BTreeMap;
use std::io::{Read, Write};

struct MapSource {
    text: BTreeMap<String, String>,
    bytes: BTreeMap<String, Vec<u8>>,
}

impl MapSource {
    fn new() -> Self {
        Self {
            text: BTreeMap::new(),
            bytes: BTreeMap::new(),
        }
    }

    fn with_text(mut self, url: &str, body: impl Into<String>) -> Self {
        self.text.insert(url.to_owned(), body.into());
        self
    }

    fn with_bytes(mut self, url: &str, body: Vec<u8>) -> Self {
        self.bytes.insert(url.to_owned(), body);
        self
    }
}

impl Source for MapSource {
    fn get_text(&self, url: &str) -> Result<String, SourceError> {
        self.text
            .get(url)
            .cloned()
            .ok_or_else(|| SourceError::new(format!("no text fixture for {url}")))
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, SourceError> {
        self.bytes
            .get(url)
            .cloned()
            .ok_or_else(|| SourceError::new(format!("no bytes fixture for {url}")))
    }
}

fn fixtures() -> String {
    format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"))
}

fn read_fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/{name}", fixtures())).expect("fixture")
}

#[test]
fn sidecars_are_parsed_and_checked() {
    assert_eq!(
        parse_sha256_sidecar(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  file.tar.gz\n",
            "file.tar.gz"
        )
        .expect("hash"),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert!(parse_sha256_sidecar("not-a-hash  file.tar.gz\n", "file.tar.gz").is_err());
    assert!(
        parse_sha256_sidecar(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  other.tar.gz\n",
            "file.tar.gz"
        )
        .is_err()
    );
}

#[test]
fn md5_sidecars_are_parsed_and_checked() {
    assert_eq!(
        parse_md5_sidecar(
            "d41d8cd98f00b204e9800998ecf8427e  file.tar.xz\n",
            "file.tar.xz"
        )
        .expect("md5"),
        "d41d8cd98f00b204e9800998ecf8427e"
    );
    assert!(parse_md5_sidecar("deadbeef  file.tar.xz\n", "file.tar.xz").is_err());
    assert!(
        parse_md5_sidecar(
            "d41d8cd98f00b204e9800998ecf8427e  other.tar.xz\n",
            "file.tar.xz"
        )
        .is_err()
    );
}

#[test]
fn postgresql_fixture_yields_six_targets_per_version() {
    let releases: Vec<GithubRelease> =
        serde_json::from_str(&read_fixture("postgres-releases.json")).expect("releases");
    let sidecars: BTreeMap<String, String> =
        serde_json::from_str(&read_fixture("postgres-sidecars.json")).expect("sidecars");
    let mut source = MapSource::new();
    for (path, body) in sidecars {
        source = source.with_text(&format!("https://example.test{path}"), body);
    }
    let (versions, artifacts) =
        postgresql_from_releases(&releases, &WAVE_ONE_POSTGRESQL, &source).expect("postgres");
    assert_eq!(versions.len(), 3);
    assert_eq!(artifacts.len(), 18);
    for artifact in &artifacts {
        assert!(artifact.url.starts_with("https://"));
        assert_eq!(artifact.sha256.len(), 64);
        assert!(artifact.size_bytes > 0);
    }
}

#[test]
fn valkey_maps_arm64_and_records_glibc_floors() {
    let sidecars: BTreeMap<String, String> =
        serde_json::from_str(&read_fixture("valkey-sidecars.json")).expect("sidecars");
    let mut source = MapSource::new();
    for (path, body) in sidecars {
        let url = format!(
            "https://download.valkey.io/releases{}",
            path.strip_prefix("/valkey").expect("prefix")
        );
        source = source.with_text(&url, body);
    }
    let (versions, artifacts) = valkey_from_sidecars(WAVE_ONE_VALKEY, &source).expect("valkey");
    assert_eq!(versions.len(), 1);
    assert_eq!(artifacts.len(), 4);
    for artifact in &artifacts {
        assert!(artifact.min_glibc.is_some(), "{}", artifact.id);
        assert!(artifact.variant.is_some(), "{}", artifact.id);
    }
    assert!(
        artifacts
            .iter()
            .any(|artifact| artifact.arch == "aarch64" && artifact.url.contains("arm64"))
    );
}

#[test]
fn mariadb_reads_sha256_from_the_rest_api() {
    let release: MariadbRelease =
        serde_json::from_str(&read_fixture("mariadb-release.json")).expect("mariadb");
    let (versions, artifacts) = mariadb_from_api(&release, "11.8.10").expect("mariadb");
    assert_eq!(versions.len(), 1);
    assert_eq!(artifacts.len(), 2);
    for artifact in &artifacts {
        assert!(artifact.url.starts_with("https://"));
        assert_eq!(artifact.sha256.len(), 64);
        assert!(artifact.size_bytes > 0);
    }
}

#[test]
fn mariadb_rejects_missing_checksum_or_size() {
    let mut release: MariadbRelease =
        serde_json::from_str(&read_fixture("mariadb-release.json")).expect("mariadb");
    release
        .files
        .retain(|file| file.file_name.ends_with(".tar.gz"));
    release.files[0].checksum.sha256sum = None;
    assert!(mariadb_from_api(&release, "11.8.10").is_err());

    let mut release: MariadbRelease =
        serde_json::from_str(&read_fixture("mariadb-release.json")).expect("mariadb");
    release
        .files
        .retain(|file| file.file_name.ends_with(".tar.gz"));
    release.files[0].size = None;
    assert!(mariadb_from_api(&release, "11.8.10").is_err());
}

#[test]
fn mysql_computes_sha256_and_cross_checks_md5() {
    let meta: serde_json::Value =
        serde_json::from_str(&read_fixture("mysql-sidecar.json")).expect("meta");
    let body = std::fs::read(format!("{}/mysql-archive.bin", fixtures())).expect("archive");
    let file_name = meta["file_name"].as_str().expect("file_name");
    let source = MapSource::new()
        .with_text(
            &format!("https://example.test/mysql/{file_name}.md5"),
            meta["md5_text"].as_str().expect("md5"),
        )
        .with_bytes(&format!("https://example.test/mysql/{file_name}"), body);
    let build = MysqlBuild {
        file_name: file_name.to_owned(),
        os: Os::Linux,
        arch: Arch::X86_64,
        format: ArchiveFormat::TarXz,
        base_url: "https://example.test/mysql".to_owned(),
    };
    let artifact = mysql_artifact(&build, "8.4.11", &source).expect("mysql");
    assert_eq!(artifact.sha256, meta["sha256"].as_str().expect("sha256"));
    assert_eq!(artifact.size_bytes, meta["size"].as_u64().expect("size"));
}

#[test]
fn mysql_rejects_a_bad_md5_sidecar() {
    let meta: serde_json::Value =
        serde_json::from_str(&read_fixture("mysql-sidecar.json")).expect("meta");
    let body = std::fs::read(format!("{}/mysql-archive.bin", fixtures())).expect("archive");
    let file_name = meta["file_name"].as_str().expect("file_name");
    let source = MapSource::new()
        .with_text(
            &format!("https://example.test/mysql/{file_name}.md5"),
            "00000000000000000000000000000000  wrong-name.tar.xz\n",
        )
        .with_bytes(&format!("https://example.test/mysql/{file_name}"), body);
    let build = MysqlBuild {
        file_name: file_name.to_owned(),
        os: Os::Linux,
        arch: Arch::X86_64,
        format: ArchiveFormat::TarXz,
        base_url: "https://example.test/mysql".to_owned(),
    };
    assert!(mysql_artifact(&build, "8.4.11", &source).is_err());
}

#[test]
fn mysql_rejects_a_body_that_breaks_md5() {
    let meta: serde_json::Value =
        serde_json::from_str(&read_fixture("mysql-sidecar.json")).expect("meta");
    let file_name = meta["file_name"].as_str().expect("file_name");
    let source = MapSource::new()
        .with_text(
            &format!("https://example.test/mysql/{file_name}.md5"),
            meta["md5_text"].as_str().expect("md5"),
        )
        .with_bytes(
            &format!("https://example.test/mysql/{file_name}"),
            b"truncated".to_vec(),
        );
    let build = MysqlBuild {
        file_name: file_name.to_owned(),
        os: Os::Linux,
        arch: Arch::X86_64,
        format: ArchiveFormat::TarXz,
        base_url: "https://example.test/mysql".to_owned(),
    };
    assert!(mysql_artifact(&build, "8.4.11", &source).is_err());
}

#[test]
fn require_https_rejects_non_tls_urls() {
    assert!(require_https("https://example.test/x").is_ok());
    for bad in ["http://example.test/x", "file:///x", "ftp://x", "x", ""] {
        assert!(require_https(bad).is_err(), "{bad}");
    }
}

#[test]
fn hex_and_format_and_version_rank_helpers() {
    assert_eq!(hex(&[0u8, 1, 255]), "0001ff");
    assert!(is_sha256_hex(&"a".repeat(64)));
    assert!(!is_sha256_hex("AAAA"));
    assert_eq!(
        super::helpers::archive_format("a.tar.gz"),
        Some(ArchiveFormat::TarGz)
    );
    assert_eq!(
        super::helpers::archive_format("a.tar.xz"),
        Some(ArchiveFormat::TarXz)
    );
    assert_eq!(
        super::helpers::archive_format("a.zip"),
        Some(ArchiveFormat::Zip)
    );
    assert_eq!(super::helpers::archive_format("a.rar"), None);
    assert_eq!(super::helpers::version_rank("18.7.0").0, 18);
    assert_eq!(super::helpers::version_rank("bad").3, "bad");
}

#[test]
fn assemble_is_sorted_deterministically() {
    let versions = vec![
        CatalogVersion {
            engine: "valkey".to_owned(),
            version: "9.2.0".to_owned(),
            major_line: "9".to_owned(),
            release_date: None,
            eol: false,
        },
        CatalogVersion {
            engine: "postgresql".to_owned(),
            version: "16.16.0".to_owned(),
            major_line: "16".to_owned(),
            release_date: None,
            eol: false,
        },
        CatalogVersion {
            engine: "postgresql".to_owned(),
            version: "18.7.0".to_owned(),
            major_line: "18".to_owned(),
            release_date: None,
            eol: false,
        },
    ];
    let doc = assemble(
        versions,
        Vec::new(),
        Some("2026-11-20T00:00:00Z".to_owned()),
    );
    assert_eq!(doc.versions[0].engine, "postgresql");
    assert_eq!(doc.versions[0].version, "18.7.0");
    assert_eq!(doc.versions[1].version, "16.16.0");
    assert_eq!(doc.versions[2].engine, "valkey");
}

#[test]
fn sidecar_rejects_empty_and_hashless_input() {
    assert!(parse_sha256_sidecar("", "x.tar.gz").is_err());
    assert!(parse_sha256_sidecar("   \n", "x.tar.gz").is_err());
    assert!(parse_md5_sidecar("", "x.tar.xz").is_err());
    assert!(parse_md5_sidecar("   \n", "x.tar.xz").is_err());
}

#[test]
fn mariadb_target_filters() {
    assert!(super::mariadb::mariadb_target("Linux", "x86_64", "mariadb.tar.gz").is_some());
    assert!(super::mariadb::mariadb_target("Windows", "amd64", "mariadb.zip").is_some());
    assert!(super::mariadb::mariadb_target("OSX", "x86_64", "mariadb.tar.gz").is_none());
    assert!(super::mariadb::mariadb_target("Linux", "ppc64le", "mariadb.tar.gz").is_none());
    assert!(super::mariadb::mariadb_target("Linux", "x86_64", "mariadb.msi").is_none());
    assert!(super::mariadb::mariadb_target("Linux", "arm64", "mariadb.tar.gz").is_some());
}

#[test]
fn postgres_missing_release_or_asset_is_an_error() {
    let releases: Vec<GithubRelease> =
        serde_json::from_str(&read_fixture("postgres-releases.json")).expect("releases");
    let sidecars: BTreeMap<String, String> =
        serde_json::from_str(&read_fixture("postgres-sidecars.json")).expect("sidecars");
    let mut source = MapSource::new();
    for (path, body) in sidecars {
        source = source.with_text(&format!("https://example.test{path}"), body);
    }
    let err = postgresql_from_releases(&releases, &["99.0.0"], &source).expect_err("missing");
    assert!(err.to_string().contains("no PostgreSQL release"));

    // A release missing an expected asset.
    let mut broken = releases.clone();
    if let Some(release) = broken.iter_mut().find(|r| r.tag_name == "18.7.0") {
        release
            .assets
            .retain(|asset| !asset.name.contains("x86_64-unknown-linux-gnu.tar.gz"));
    }
    assert!(postgresql_from_releases(&broken, &["18.7.0"], &source).is_err());
}

#[test]
fn postgres_rejects_asset_without_sidecar() {
    let mut releases: Vec<GithubRelease> =
        serde_json::from_str(&read_fixture("postgres-releases.json")).expect("releases");
    if let Some(release) = releases.iter_mut().find(|r| r.tag_name == "18.7.0") {
        release.assets.retain(|asset| {
            !asset
                .name
                .ends_with("x86_64-unknown-linux-gnu.tar.gz.sha256")
        });
    }
    let sidecars: BTreeMap<String, String> =
        serde_json::from_str(&read_fixture("postgres-sidecars.json")).expect("sidecars");
    let mut source = MapSource::new();
    for (path, body) in sidecars {
        source = source.with_text(&format!("https://example.test{path}"), body);
    }
    assert!(postgresql_from_releases(&releases, &["18.7.0"], &source).is_err());
}

#[test]
fn mariadb_rejects_empty_release_and_malformed_sha() {
    let release = MariadbRelease {
        release_id: "11.8.10".to_owned(),
        files: vec![MariadbFile {
            file_name: "mariadb-11.8.10-linux-systemd-x86_64.tar.gz".to_owned(),
            os: "Linux".to_owned(),
            cpu: "x86_64".to_owned(),
            file_download_url: "https://example.test/mariadb.tar.gz".to_owned(),
            checksum: MariadbChecksum {
                sha256sum: Some("NOTHEX".to_owned()),
            },
            size: Some(100),
        }],
    };
    assert!(mariadb_from_api(&release, "11.8.10").is_err());

    let release = MariadbRelease {
        release_id: "11.8.10".to_owned(),
        files: vec![],
    };
    assert!(mariadb_from_api(&release, "11.8.10").is_err());

    let release = MariadbRelease {
        release_id: "11.8.10".to_owned(),
        files: vec![MariadbFile {
            file_name: "mariadb-11.8.10-linux-systemd-x86_64.tar.gz".to_owned(),
            os: "Linux".to_owned(),
            cpu: "x86_64".to_owned(),
            file_download_url: String::new(),
            checksum: MariadbChecksum {
                sha256sum: Some("a".repeat(64)),
            },
            size: Some(100),
        }],
    };
    assert!(mariadb_from_api(&release, "11.8.10").is_err());
}

#[test]
fn mysql_rejects_empty_download() {
    let meta: serde_json::Value =
        serde_json::from_str(&read_fixture("mysql-sidecar.json")).expect("meta");
    let file_name = meta["file_name"].as_str().expect("file_name");
    let source = MapSource::new()
        .with_text(
            &format!("https://example.test/mysql/{file_name}.md5"),
            meta["md5_text"].as_str().expect("md5"),
        )
        .with_bytes(
            &format!("https://example.test/mysql/{file_name}"),
            Vec::new(),
        );
    let build = MysqlBuild {
        file_name: file_name.to_owned(),
        os: Os::Linux,
        arch: Arch::X86_64,
        format: ArchiveFormat::TarXz,
        base_url: "https://example.test/mysql".to_owned(),
    };
    assert!(mysql_artifact(&build, "8.4.11", &source).is_err());
}

#[test]
fn md5_vector_matches_rfc1321() {
    assert_eq!(
        hex(&super::mysql::md5_compute(b"")),
        "d41d8cd98f00b204e9800998ecf8427e"
    );
    assert_eq!(
        hex(&super::mysql::md5_compute(b"abc")),
        "900150983cd24fb0d6963f7d28e17f72"
    );
}

#[test]
fn http_source_rejects_non_https_and_handles_local_responses() {
    let source = HttpSource::new(None).expect("client");
    let err = source
        .get_text("http://example.test/x")
        .expect_err("http must be rejected");
    assert!(err.to_string().contains("non-HTTPS"));

    let source_with_token = HttpSource::new(Some("secret-token".to_owned())).expect("client");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        let mut served = 0usize;
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let body = if served == 0 { "ok" } else { "nope" };
            let status = if served == 0 {
                "200 OK"
            } else {
                "500 Internal Server Error"
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            served += 1;
            if served == 2 {
                break;
            }
        }
    });
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(
        source_with_token
            .request(&format!("{base}/ok"))
            .expect("ok"),
        b"ok"
    );
    assert!(source_with_token.request(&format!("{base}/bad")).is_err());
    let _ = handle.join();
}
