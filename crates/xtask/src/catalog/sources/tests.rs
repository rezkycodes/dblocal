use super::helpers::{hex, is_sha256_hex, parse_md5_sidecar, parse_sha256_sidecar};
use super::mariadb::{
    MariadbChecksum, MariadbFile, MariadbRelease, mariadb_https_url, mariadb_release_from_api,
};

use super::*;
use std::collections::BTreeMap;
use std::io::{Read, Write};

pub(super) struct MapSource {
    text: BTreeMap<String, String>,
    text_by_encoding: BTreeMap<(String, String), String>,
    bytes: BTreeMap<String, Vec<u8>>,
    sizes: BTreeMap<(String, String), u64>,
}

impl MapSource {
    pub(super) fn new() -> Self {
        Self {
            text: BTreeMap::new(),
            text_by_encoding: BTreeMap::new(),
            bytes: BTreeMap::new(),
            sizes: BTreeMap::new(),
        }
    }

    pub(super) fn with_text(mut self, url: &str, body: impl Into<String>) -> Self {
        self.text.insert(url.to_owned(), body.into());
        self
    }

    pub(super) fn with_text_enc(
        mut self,
        url: &str,
        accept_encoding: &str,
        body: impl Into<String>,
    ) -> Self {
        self.text_by_encoding
            .insert((url.to_owned(), accept_encoding.to_owned()), body.into());
        self
    }

    pub(super) fn with_bytes(mut self, url: &str, body: Vec<u8>) -> Self {
        self.bytes.insert(url.to_owned(), body);
        self
    }

    pub(super) fn with_size(mut self, url: &str, accept_encoding: &str, size: u64) -> Self {
        self.sizes
            .insert((url.to_owned(), accept_encoding.to_owned()), size);
        self
    }
}

impl Source for MapSource {
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, SourceError> {
        self.bytes
            .get(url)
            .cloned()
            .ok_or_else(|| SourceError::new(format!("no bytes fixture for {url}")))
    }

    fn get_text_with(&self, url: &str, accept_encoding: &str) -> Result<String, SourceError> {
        if let Some(body) = self
            .text_by_encoding
            .get(&(url.to_owned(), accept_encoding.to_owned()))
        {
            return Ok(body.clone());
        }
        self.text
            .get(url)
            .cloned()
            .ok_or_else(|| SourceError::new(format!("no text fixture for {url}")))
    }

    fn head_size(&self, url: &str, accept_encoding: &str) -> Result<u64, SourceError> {
        self.sizes
            .get(&(url.to_owned(), accept_encoding.to_owned()))
            .copied()
            .ok_or_else(|| {
                SourceError::new(format!("no HEAD fixture for {url} [{accept_encoding}]"))
            })
    }
}

pub(super) fn fixtures() -> String {
    format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"))
}

/// A bare tracked version for the fixture tests (no EOL, no status).
pub(super) fn tv(version: &'static str, line: &'static str) -> TrackedVersion {
    TrackedVersion {
        version,
        line,
        eol: false,
        status: None,
    }
}

pub(super) fn read_fixture(name: &str) -> String {
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
    // theseus-rs Windows assets publish Windows CertUtil output, not a sidecar
    // line: a header, then the bare hash. The hash must still be found.
    let certutil = "SHA256 hash of postgresql-18.6.0-x86_64-pc-windows-msvc.zip:\r\n\
                    dd4bcd50b38bf5b9f8a6e0d6736bd4412a79a066639cb855479d4d58b1dd74ac\r\n\
                    CertUtil: -hashfile command completed successfully.\r\n";
    assert_eq!(
        parse_sha256_sidecar(certutil, "postgresql-18.6.0-x86_64-pc-windows-msvc.zip")
            .expect("certutil"),
        "dd4bcd50b38bf5b9f8a6e0d6736bd4412a79a066639cb855479d4d58b1dd74ac"
    );
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
    let wanted = [tv("18.7.0", "18"), tv("17.12.0", "17"), tv("16.16.0", "16")];
    let (versions, artifacts) =
        postgresql_from_releases(&releases, &wanted, &source).expect("postgres");
    assert_eq!(versions.len(), 3);
    assert_eq!(artifacts.len(), 18);
    for artifact in &artifacts {
        assert!(artifact.url.starts_with("https://"));
        assert_eq!(artifact.sha256.len(), 64);
        assert!(artifact.size_bytes > 0);
    }
}

fn load_valkey_source() -> MapSource {
    let sidecars: BTreeMap<String, BTreeMap<String, String>> =
        serde_json::from_str(&read_fixture("valkey-sidecars.json")).expect("sidecars");
    let heads: BTreeMap<String, BTreeMap<String, u64>> =
        serde_json::from_str(&read_fixture("valkey-heads.json")).expect("heads");
    let mut source = MapSource::new();
    for (path, encs) in sidecars {
        let url = format!(
            "https://download.valkey.io/releases{}",
            path.strip_prefix("/valkey").expect("prefix")
        );
        for (enc, body) in encs {
            source = source.with_text_enc(&url, &enc, body);
        }
    }
    for (path, encs) in heads {
        let url = format!(
            "https://download.valkey.io/releases{}",
            path.strip_prefix("/valkey").expect("prefix")
        );
        for (enc, size) in encs {
            source = source.with_size(&url, &enc, size);
        }
    }
    source
}

#[test]
fn valkey_maps_arm64_reads_size_from_head_and_records_glibc_floors() {
    // Failing-first for B2: this used to take hard-coded sizes and one sidecar
    // fetch, so it could not read or cross-check the real upstream size.
    let source = load_valkey_source();
    let (versions, artifacts) =
        valkey_from_sidecars(&tv("9.2.0", "9.2"), &WAVE_ONE_VARIANTS, &source).expect("valkey");
    assert_eq!(versions.len(), 1);
    assert_eq!(artifacts.len(), 2);
    for artifact in &artifacts {
        assert!(artifact.min_glibc.is_some(), "{}", artifact.id);
        assert!(artifact.variant.is_some(), "{}", artifact.id);
        assert!(artifact.size_bytes > 0, "{}", artifact.id);
    }
    assert!(
        artifacts
            .iter()
            .any(|artifact| artifact.arch == "aarch64" && artifact.url.contains("arm64"))
    );
    let jammy_x86 = artifacts
        .iter()
        .find(|artifact| artifact.variant.as_deref() == Some("jammy") && artifact.arch == "x86_64")
        .expect("jammy x86_64");
    assert_eq!(jammy_x86.size_bytes, 46_759_925);
}

#[test]
fn valkey_wave_one_selection_is_jammy_only() {
    let source = load_valkey_source();
    let (_, artifacts) =
        valkey_from_sidecars(&tv("9.2.0", "9.2"), &WAVE_ONE_VARIANTS, &source).expect("valkey");
    assert_eq!(artifacts.len(), 2);
    assert!(
        artifacts
            .iter()
            .all(|artifact| artifact.variant.as_deref() == Some("jammy"))
    );
}

#[test]
fn valkey_fails_when_identity_and_gzip_objects_differ() {
    // B2 failing-first: noble-x86_64 disagrees across encodings in the recorded
    // upstream fixture, so the builder must refuse to pin it.
    let source = load_valkey_source();
    let err = valkey_from_sidecars(&tv("9.2.0", "9.2"), &["noble"], &source)
        .expect_err("encoding mismatch must fail");
    assert!(
        err.to_string()
            .contains("inconsistent across Accept-Encoding")
    );
    assert!(err.to_string().contains("valkey-9.2.0-linux-x86_64-noble"));
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
            line: None,
            status: None,
            license: None,
        },
        CatalogVersion {
            engine: "postgresql".to_owned(),
            version: "16.16.0".to_owned(),
            major_line: "16".to_owned(),
            release_date: None,
            eol: false,
            line: None,
            status: None,
            license: None,
        },
        CatalogVersion {
            engine: "postgresql".to_owned(),
            version: "18.7.0".to_owned(),
            major_line: "18".to_owned(),
            release_date: None,
            eol: false,
            line: None,
            status: None,
            license: None,
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
    // MariaDB publishes no arm64 Linux tarball (W0-B, and the REST API shows
    // none for 11.4/11.8), so the target must not be accepted.
    assert!(super::mariadb::mariadb_target("Linux", "arm64", "mariadb.tar.gz").is_none());
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
    let err =
        postgresql_from_releases(&releases, &[tv("99.0.0", "99")], &source).expect_err("missing");
    assert!(err.to_string().contains("no PostgreSQL release"));

    // A release missing an expected asset.
    let mut broken = releases.clone();
    if let Some(release) = broken.iter_mut().find(|r| r.tag_name == "18.7.0") {
        release
            .assets
            .retain(|asset| !asset.name.contains("x86_64-unknown-linux-gnu.tar.gz"));
    }
    assert!(postgresql_from_releases(&broken, &[tv("18.7.0", "18")], &source).is_err());
}

#[test]
fn postgres_skips_a_missing_windows_arm64_but_not_other_targets() {
    // PostgreSQL 14/15 publish no `windows-aarch64` asset. A build that omits it
    // must still succeed, with one fewer artifact, rather than failing the whole
    // catalog; omitting a Linux asset still fails.
    let mut releases: Vec<GithubRelease> =
        serde_json::from_str(&read_fixture("postgres-releases.json")).expect("releases");
    if let Some(release) = releases.iter_mut().find(|r| r.tag_name == "18.7.0") {
        release
            .assets
            .retain(|asset| !asset.name.contains("aarch64-pc-windows-msvc"));
    }
    let sidecars: BTreeMap<String, String> =
        serde_json::from_str(&read_fixture("postgres-sidecars.json")).expect("sidecars");
    let mut source = MapSource::new();
    for (path, body) in sidecars {
        source = source.with_text(&format!("https://example.test{path}"), body);
    }
    let (_, artifacts) = postgresql_from_releases(&releases, &[tv("18.7.0", "18")], &source)
        .expect("windows arm64 may be absent");
    assert_eq!(artifacts.len(), 5, "one target fewer, the rest kept");
    assert!(
        !artifacts
            .iter()
            .any(|a| a.os == "windows" && a.arch == "aarch64")
    );
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
    assert!(postgresql_from_releases(&releases, &[tv("18.7.0", "18")], &source).is_err());
}

#[test]
fn mariadb_rejects_empty_release_and_malformed_sha() {
    let release = MariadbRelease {
        release_id: "11.8.10".to_owned(),
        date_of_release: "2026-11-17".to_owned(),
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
    assert!(mariadb_from_api(&release, &tv("11.8.10", "11.8")).is_err());

    let release = MariadbRelease {
        release_id: "11.8.10".to_owned(),
        date_of_release: "2026-11-17".to_owned(),
        files: vec![],
    };
    assert!(mariadb_from_api(&release, &tv("11.8.10", "11.8")).is_err());

    let release = MariadbRelease {
        release_id: "11.8.10".to_owned(),
        date_of_release: "2026-11-17".to_owned(),
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
    assert!(mariadb_from_api(&release, &tv("11.8.10", "11.8")).is_err());
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
fn json_errors_convert_to_source_errors() {
    let err: SourceError = serde_json::from_str::<serde_json::Value>("nope")
        .expect_err("invalid")
        .into();
    assert!(err.to_string().contains("invalid JSON from upstream"));
}

#[test]
fn https_enforcement_covers_every_transport_method() {
    let source = HttpSource::new(None).expect("client");
    for url in ["http://example.test/x", "file:///etc/passwd"] {
        assert!(source.get_bytes(url).is_err());
        assert!(source.get_text_with(url, "identity").is_err());
        assert!(source.head_size(url, "identity").is_err());
    }
}

#[test]
fn head_reports_a_non_success_status() {
    let source = HttpSource::for_loopback().expect("client");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        if let Some(mut stream) = listener.incoming().flatten().next() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let err = source
        .head(&format!("http://127.0.0.1:{port}/x"), "identity")
        .expect_err("500 must fail");
    assert!(err.to_string().contains("returned HTTP 500"));
    let _ = handle.join();
}

#[test]
fn github_token_is_attached_only_for_the_api_host() {
    let token = HttpSource::new(Some("secret-token".to_owned())).expect("client");
    let api = token
        .build_request(
            reqwest::Method::GET,
            "https://api.github.com/repos/x/y",
            None,
        )
        .expect("request")
        .build()
        .expect("build");
    assert!(api.headers().contains_key(reqwest::header::AUTHORIZATION));

    let other = token
        .build_request(reqwest::Method::GET, "https://example.test/x", None)
        .expect("request")
        .build()
        .expect("build");
    assert!(!other.headers().contains_key(reqwest::header::AUTHORIZATION));
    assert_eq!(
        other
            .headers()
            .get(reqwest::header::ACCEPT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("identity")
    );
}

#[test]
fn a_redirect_to_plain_http_fails_the_builder_request() {
    let source = HttpSource::for_loopback().expect("client");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        if let Some(mut stream) = listener.incoming().flatten().next() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let err = source
        .request(
            reqwest::Method::GET,
            &format!("http://127.0.0.1:{port}/x"),
            None,
        )
        .expect_err("http redirect must be refused");
    assert!(err.to_string().contains("redirect"), "{err}");
    let _ = handle.join();
}

#[test]
fn redirect_policy_refuses_non_https_and_long_chains() {
    // B1 for xtask: the builder client must never follow a redirect off TLS.
    let https = url::Url::parse("https://example.test/a.tar.gz").expect("url");
    let plain = url::Url::parse("http://example.test/a.tar.gz").expect("url");
    assert!(allow_redirect(0, &plain).is_err());
    assert!(allow_redirect(MAX_REDIRECTS, &https).is_ok());
    assert!(allow_redirect(MAX_REDIRECTS + 1, &https).is_err());
}

#[test]
fn http_source_wraps_non_utf8_bodies() {
    let source = HttpSource::for_loopback().expect("client");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        if let Some(mut stream) = listener.incoming().flatten().next() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n\xff\xfe",
            );
        }
    });
    let bytes = source
        .request(
            reqwest::Method::GET,
            &format!("http://127.0.0.1:{port}/x"),
            None,
        )
        .expect("raw bytes");
    assert!(String::from_utf8(bytes).is_err());
    let _ = handle.join();
}

#[test]
fn http_source_reports_a_missing_content_length_on_head() {
    let source = HttpSource::for_loopback().expect("client");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        if let Some(mut stream) = listener.incoming().flatten().next() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
        }
    });
    let err = source
        .head(&format!("http://127.0.0.1:{port}/x"), "identity")
        .expect_err("missing Content-Length must be an error");
    assert!(err.to_string().contains("no Content-Length"));
    let _ = handle.join();
}

#[test]
fn http_source_head_size_reads_the_declared_length() {
    let source = HttpSource::for_loopback().expect("client");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        if let Some(mut stream) = listener.incoming().flatten().next() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 12345\r\nConnection: close\r\n\r\n",
            );
        }
    });
    assert_eq!(
        source
            .head(&format!("http://127.0.0.1:{port}/x"), "identity")
            .expect("size"),
        12345
    );
    let _ = handle.join();
}

#[test]
fn http_source_rejects_non_https_and_handles_local_responses() {
    let source = HttpSource::new(None).expect("client");
    let err = source
        .get_text("http://example.test/x")
        .expect_err("http must be rejected");
    assert!(err.to_string().contains("non-HTTPS"));

    let source_with_token = HttpSource::for_loopback().expect("client");
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
            .request(reqwest::Method::GET, &format!("{base}/ok"), None)
            .expect("ok"),
        b"ok"
    );
    assert!(
        source_with_token
            .request(reqwest::Method::GET, &format!("{base}/bad"), None)
            .is_err()
    );
    let _ = handle.join();
}

#[path = "tests_mysql_family.rs"]
mod mysql_family;
