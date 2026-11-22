//! MySQL and MariaDB source-adapter tests (W4-E). Split from `tests.rs` to keep
//! each file under the size limit.

use super::mariadb::mariadb_from_api;
use super::tests::{MapSource, read_fixture};
use super::*;

#[test]
fn mariadb_reads_sha256_from_the_rest_api() {
    // The fixture mirrors the real API: the server tarball's `size` is null and
    // its URL is `http://`. `mariadb_release_from_api` must resolve the size
    // with a HEAD and upgrade the URL, or nothing is installable (finding: the
    // old code refused every real release on the missing size).
    let release: MariadbRelease =
        serde_json::from_str(&read_fixture("mariadb-release.json")).expect("mariadb");
    let api_url = "https://api.example.test/mariadb/11.8.10/";
    let linux_head = 413_068_258;
    let source = MapSource::new()
        .with_text(api_url, read_fixture("mariadb-release.json"))
        .with_size(
            "https://example.test/mariadb/mariadb-11.8.10-linux-systemd-x86_64.tar.gz",
            "identity",
            linux_head,
        );
    let resolved = mariadb_release_from_api(&source, api_url, "11.8.10").expect("release");
    let (versions, artifacts) = mariadb_from_api(&resolved, "11.8.10").expect("mariadb");
    assert_eq!(versions.len(), 1);
    assert_eq!(artifacts.len(), 2, "one Linux and one Windows server build");

    let linux = artifacts
        .iter()
        .find(|a| a.os == "linux")
        .expect("linux artifact");
    assert_eq!(linux.size_bytes, linux_head);
    assert!(
        linux.url.starts_with("https://"),
        "the http URL must be upgraded: {}",
        linux.url
    );
    assert_eq!(
        linux.strip_prefix.as_deref(),
        Some("mariadb-11.8.10-linux-systemd-x86_64"),
        "the strip prefix must be the archive's real root"
    );
    let windows = artifacts
        .iter()
        .find(|a| a.os == "windows")
        .expect("windows artifact");
    assert_eq!(
        windows.strip_prefix.as_deref(),
        Some("mariadb-11.8.10-winx64"),
        "the Windows zip root must not keep its extension"
    );
    // Plugins and debug-symbol archives never become the installable build.
    assert!(
        artifacts.iter().all(|a| !a.url.contains("plugin")
            && !a.url.contains("debugsymbols")
            && !a.url.contains("/mariadb-11.8.10.tar.gz")),
        "non-server files must be skipped: {:?}",
        artifacts.iter().map(|a| &a.url).collect::<Vec<_>>()
    );
    // The release date comes from the API, not from the version id.
    assert_eq!(versions[0].release_date.as_deref(), Some("2026-11-17"));

    // A release whose Linux build has a size already needs no HEAD.
    let mut given = release;
    for file in &mut given.files {
        if file.file_name.contains("linux") {
            file.size = Some(424242);
        }
    }
    let (_, artifacts) = mariadb_from_api(&given, "11.8.10").expect("mariadb with size");
    assert!(artifacts.iter().any(|a| a.size_bytes == 424242));
}

#[test]
fn mariadb_rejects_missing_checksum_or_size() {
    let mut release: MariadbRelease =
        serde_json::from_str(&read_fixture("mariadb-release.json")).expect("mariadb");
    release
        .files
        .retain(|file| file.file_name.contains("linux-systemd"));
    release.files[0].checksum.sha256sum = None;
    assert!(mariadb_from_api(&release, "11.8.10").is_err());

    let mut release: MariadbRelease =
        serde_json::from_str(&read_fixture("mariadb-release.json")).expect("mariadb");
    release
        .files
        .retain(|file| file.file_name.contains("linux-systemd"));
    release.files[0].size = None;
    assert!(
        mariadb_from_api(&release, "11.8.10").is_err(),
        "a null size must still be refused when nothing resolved it"
    );
}

#[test]
fn mariadb_release_from_api_reads_both_envelopes() {
    // Older API: `{"releases": {"11.8.10": {...}}}`. Newer: `release_data`.
    let release: MariadbRelease =
        serde_json::from_str(&read_fixture("mariadb-release.json")).expect("mariadb");
    let mut files = release.files.clone();
    for file in &mut files {
        if file.file_name.contains("linux") {
            file.size = Some(1);
        }
    }
    let release = MariadbRelease {
        release_id: "11.8.10".to_owned(),
        date_of_release: "2026-11-17".to_owned(),
        files,
    };

    for key in ["releases", "release_data"] {
        let body = serde_json::json!({
            key: {"11.8.10": release},
        })
        .to_string();
        let url = "https://api.example.test/x/";
        let source = MapSource::new().with_text(url, body);
        let resolved = mariadb_release_from_api(&source, url, "11.8.10").expect(key);
        assert_eq!(resolved.files.len(), release.files.len(), "{key}");
    }

    let url = "https://api.example.test/none/";
    let source = MapSource::new().with_text(url, read_fixture("mariadb-release.json"));
    let err = mariadb_release_from_api(&source, url, "99.9.9").expect_err("missing version");
    assert!(err.to_string().contains("no release"));
}

#[test]
fn mariadb_https_url_upgrades_and_rejects() {
    assert_eq!(
        mariadb_https_url("http://downloads.mariadb.org/x.tar.gz").expect("upgrade"),
        "https://downloads.mariadb.org/x.tar.gz"
    );
    assert_eq!(
        mariadb_https_url("https://downloads.mariadb.org/x.tar.gz").expect("kept"),
        "https://downloads.mariadb.org/x.tar.gz"
    );
    assert!(mariadb_https_url("ftp://x/y").is_err());
    assert!(mariadb_https_url("").is_err());
}

#[test]
fn mysql_archive_root_matches_every_published_format() {
    for (name, root) in [
        (
            "mysql-8.4.11-linux-glibc2.28-x86_64-minimal.tar.xz",
            "mysql-8.4.11-linux-glibc2.28-x86_64-minimal",
        ),
        (
            "mysql-8.4.11-macos15-arm64.tar.gz",
            "mysql-8.4.11-macos15-arm64",
        ),
        ("mysql-8.4.11-winx64.zip", "mysql-8.4.11-winx64"),
    ] {
        assert_eq!(super::mysql::archive_root(name).expect(name), root);
    }
    assert!(super::mysql::archive_root("mysql-8.4.11.tgz").is_err());
}

#[test]
fn mysql_published_builds_use_the_real_strip_prefix_and_existing_urls() {
    // Failing-first for the builder defect: `strip_prefix` used to be
    // `mysql-{version}-minimal`, which no real archive contains, so `bin/` would
    // land one directory too deep after a genuine download.
    let builds = super::mysql_wave_one_builds();
    assert!(!builds.is_empty());
    for build in &builds {
        let root = super::mysql::archive_root(&build.file_name).expect("root");
        assert!(
            root.starts_with("mysql-8.4.11-"),
            "{}: root {root}",
            build.file_name
        );
        assert_ne!(root, "mysql-8.4.11-minimal");
        assert!(
            build.file_name.contains("linux-glibc2.28-x86_64-minimal")
                || build.file_name.contains("macos15")
                || build.file_name.contains("winx64"),
            "unexpected build {}",
            build.file_name
        );
    }
    // No arm64 Linux minimal tarball exists upstream, so none is claimed.
    assert!(
        !builds
            .iter()
            .any(|b| b.os == Os::Linux && b.arch == Arch::Aarch64),
        "there is no linux arm64 minimal build to point at"
    );
}

#[test]
fn mariadb_has_no_arm64_or_macos_artifact() {
    // The project publishes neither, so the catalog must not invent one.
    assert!(super::mariadb::mariadb_target("Linux", "arm64", "mariadb.tar.gz").is_none());
    assert!(super::mariadb::mariadb_target("OSX", "x86_64", "mariadb.tar.gz").is_none());
    assert!(super::mariadb::mariadb_target("Linux", "x86_64", "mariadb.tar.gz").is_some());
}
