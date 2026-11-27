//! Offline unit and integration tests for `cargo xtask toolchains-catalog`.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};

use super::composer;
use super::node;
use super::php_linux;
use super::php_windows;
use super::schema::{self, ToolchainsDocument, validate_document};
use super::smoke::{SmokeRunner, parse_php_modules, safe_extract_tar_gz};
use super::transport::{MemorySource, hex_lower};
use super::{ToolchainsCatalogArgs, generate, run_with, utc_timestamp};

fn fixtures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/toolchains")
}

#[derive(Default)]
struct MockSmokeRunner {
    fail_php_version: Option<String>,
    fail_node_version: Option<String>,
}

impl SmokeRunner for MockSmokeRunner {
    fn verify_php_linux(
        &self,
        archive_path: &Path,
        expected_version: &str,
        keep_binary_at: Option<&Path>,
    ) -> Result<Vec<String>, String> {
        if self.fail_php_version.as_deref() == Some(expected_version) {
            return Err(format!("mock php {expected_version} crashed"));
        }
        if let Some(dest) = keep_binary_at {
            std::fs::write(dest, b"mock-php").map_err(|e| e.to_string())?;
        }
        let is_bulk = archive_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.contains("bulk"));
        if is_bulk {
            Ok(vec![
                "curl".to_owned(),
                "intl".to_owned(),
                "mbstring".to_owned(),
                "mysqli".to_owned(),
                "opcache".to_owned(),
                "openssl".to_owned(),
                "pdo_mysql".to_owned(),
                "sodium".to_owned(),
            ])
        } else {
            Ok(vec![
                "ctype".to_owned(),
                "curl".to_owned(),
                "fileinfo".to_owned(),
                "mbstring".to_owned(),
                "openssl".to_owned(),
                "pdo_mysql".to_owned(),
                "pdo_pgsql".to_owned(),
                "pdo_sqlite".to_owned(),
                "phar".to_owned(),
                "tokenizer".to_owned(),
                "zlib".to_owned(),
            ])
        }
    }

    fn verify_node_linux(
        &self,
        _archive_path: &Path,
        expected_version: &str,
    ) -> Result<(), String> {
        if self.fail_node_version.as_deref() == Some(expected_version) {
            return Err(format!("mock node {expected_version} crashed"));
        }
        Ok(())
    }

    fn verify_composer(
        &self,
        _phar_path: &Path,
        _php_binary: &Path,
        _expected_version: &str,
    ) -> Result<(), String> {
        Ok(())
    }
}

fn build_complete_fixture_source() -> MemorySource {
    let mut src = MemorySource::from_fixture_dir(&fixtures_dir()).expect("load fixture dir");

    // PHP Linux tarballs (fake bytes, hashed dynamically by MemorySource).
    for (profile, ver) in [
        ("common", "8.0.30"),
        ("common", "8.4.23"),
        ("common", "8.5.8"),
        ("bulk", "8.0.30"),
        ("bulk", "8.4.23"),
        ("bulk", "8.5.8"),
    ] {
        let url = format!(
            "https://dl.static-php.dev/static-php-cli/{profile}/php-{ver}-cli-linux-x86_64.tar.gz"
        );
        src.files
            .insert(url, format!("fake-php-{profile}-{ver}").into_bytes());
    }

    // PHP Windows HEAD sizes.
    for (file, size) in [
        ("php-7.4.33-nts-Win32-vc15-x64.zip", 26_132_214),
        ("php-8.0.30-nts-Win32-vs16-x64.zip", 26_766_957),
        ("php-8.4.28-nts-Win32-vs17-x64.zip", 35_384_096),
        ("php-8.6.0-nts-Win32-vs18-x64.zip", 39_660_643),
    ] {
        src.head_sizes.insert(
            format!("https://downloads.php.net/~windows/releases/{file}"),
            size,
        );
    }

    // Node.js tarballs + matching SHASUMS256.txt + HEAD sizes.
    for ver in ["26.13.1", "24.22.3", "22.23.5"] {
        let linux_bytes = format!("fake-node-{ver}-linux").into_bytes();
        let linux_sha = hex_lower(&Sha256::digest(&linux_bytes));
        let win_sha = "b".repeat(64);
        let shasums = format!(
            "{linux_sha}  node-v{ver}-linux-x64.tar.gz\n{win_sha}  node-v{ver}-win-x64.zip\n"
        );
        src.texts.insert(
            format!("https://nodejs.org/dist/v{ver}/SHASUMS256.txt"),
            shasums,
        );
        let linux_url = format!("https://nodejs.org/dist/v{ver}/node-v{ver}-linux-x64.tar.gz");
        let win_url = format!("https://nodejs.org/dist/v{ver}/node-v{ver}-win-x64.zip");
        src.head_sizes
            .insert(linux_url.clone(), linux_bytes.len() as u64);
        src.files.insert(linux_url, linux_bytes);
        src.head_sizes.insert(win_url, 30_000_000);
    }

    // Composer phar + matching .sha256sum.
    let composer_bytes = b"fake-composer-2.11.3-phar".to_vec();
    let composer_sha = hex_lower(&Sha256::digest(&composer_bytes));
    src.texts.insert(
        "https://getcomposer.org/download/2.11.3/composer.phar.sha256sum".to_owned(),
        format!("{composer_sha}  composer.phar\n"),
    );
    src.files.insert(
        "https://getcomposer.org/download/2.11.3/composer.phar".to_owned(),
        composer_bytes,
    );

    src
}

#[test]
fn php_linux_discovers_latest_patch_per_series_and_ignores_other_targets() {
    let json =
        std::fs::read_to_string(fixtures_dir().join("static-php-common.json")).expect("fixture");
    let found = php_linux::discover_latest_for_profile("common", &json).expect("discover");
    let versions: Vec<&str> = found.iter().map(|c| c.version.as_str()).collect();
    // 7.4 is excluded on Linux; 8.4.22 is superseded by 8.4.23; aarch64 & fpm ignored.
    assert_eq!(versions, vec!["8.0.30", "8.4.23", "8.5.8"]);
}

#[test]
fn php_windows_parses_nts_x64_only() {
    let json =
        std::fs::read_to_string(fixtures_dir().join("windows-php-releases.json")).expect("fixture");
    let (cands, skips) = php_windows::parse_releases_json(&json).expect("parse");
    assert!(skips.is_empty());
    let pairs: Vec<(&str, &str)> = cands
        .iter()
        .map(|c| (c.series.as_str(), c.version.as_str()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("7.4", "7.4.33"),
            ("8.0", "8.0.30"),
            ("8.4", "8.4.28"),
            ("8.6", "8.6.0"),
        ]
    );
    assert!(cands.iter().all(|c| c.url.contains("-nts-Win32-")));
}

#[test]
fn node_discovers_only_lines_26_24_22_and_parses_lts_and_shasums() {
    let json = std::fs::read_to_string(fixtures_dir().join("node-index.json")).expect("fixture");
    let found = node::discover_latest_releases(&json).expect("discover");
    let triples: Vec<(&str, &str, bool)> = found
        .iter()
        .map(|c| (c.series.as_str(), c.version.as_str(), c.lts))
        .collect();
    assert_eq!(
        triples,
        vec![
            ("22", "22.23.5", true),
            ("24", "24.22.3", true),
            ("26", "26.13.1", true),
        ]
    );

    let shasums_text =
        std::fs::read_to_string(fixtures_dir().join("node-shasums-26.13.1.txt")).expect("shasums");
    let map = node::parse_shasums256(&shasums_text);
    assert_eq!(
        map.get("node-v26.13.1-linux-x64.tar.gz")
            .map(String::as_str),
        Some("518674de2b621728a5e4486f5fb3b7d2900bb3cc4aa97969edaa463db99f9778")
    );
}

#[test]
fn composer_discovers_latest_stable_2x_and_parses_sha256sum() {
    let json =
        std::fs::read_to_string(fixtures_dir().join("composer-versions.json")).expect("fixture");
    let cand = composer::discover_latest_stable_2x(&json).expect("discover");
    assert_eq!(cand.version, "2.11.3");
    assert_eq!(
        cand.phar_url,
        "https://getcomposer.org/download/2.11.3/composer.phar"
    );

    let sha_text =
        std::fs::read_to_string(fixtures_dir().join("composer-2.11.3.sha256sum")).expect("sha");
    let sha = composer::parse_sha256sum(&sha_text).expect("parse sha");
    assert_eq!(
        sha,
        "04f8b003da4e3a2459056591fbfebd02b4dc3e5adeb158616b90cf55a65113ec"
    );
}

#[test]
fn parse_php_modules_normalizes_and_sorts() {
    let raw = "\
[PHP Modules]
Core
PDO
pdo_mysql
Phar
Zend OPcache

[Zend Modules]
Zend OPcache
";
    assert_eq!(
        parse_php_modules(raw),
        vec!["core", "opcache", "pdo", "pdo_mysql", "phar"]
    );
}

#[test]
fn full_offline_generation_is_valid_deterministic_and_supports_check_mode() {
    let src = build_complete_fixture_source();
    let smoke = MockSmokeRunner::default();
    let dir = tempfile::tempdir().expect("tempdir");
    let out_path = dir.path().join("toolchains-v1.json");
    let t1 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_700_000);
    let t2 = t1 + Duration::from_secs(3_600);

    let write_args = ToolchainsCatalogArgs {
        output: out_path.clone(),
        check: false,
        offline_fixtures: None,
    };
    let summary = run_with(&write_args, &src, &smoke, t1).expect("write run");
    assert!(summary.contains("skipped: none"));

    let text = std::fs::read_to_string(&out_path).expect("read");
    assert!(text.ends_with('\n'));
    let doc: ToolchainsDocument = serde_json::from_str(&text).expect("parse");
    validate_document(&doc).expect("valid");

    // PHP 8.0.30 merges Linux (bulk + common) and Windows into one version entry.
    let php = doc.tools.iter().find(|t| t.id == "php").expect("php");
    let v80 = php
        .versions
        .iter()
        .find(|v| v.version == "8.0.30")
        .expect("8.0.30");
    assert_eq!(v80.artifacts.len(), 3);

    // `--check` at a later timestamp succeeds despite differing `generated_at`.
    let check_args = ToolchainsCatalogArgs {
        output: out_path.clone(),
        check: true,
        offline_fixtures: None,
    };
    let check_summary = run_with(&check_args, &src, &smoke, t2).expect("check run");
    assert!(check_summary.contains("is up to date"));
    assert_ne!(utc_timestamp(t1), utc_timestamp(t2));
}

#[test]
fn pinned_hash_change_for_unchanged_url_fails_loudly() {
    let src = build_complete_fixture_source();
    let smoke = MockSmokeRunner::default();
    let mut existing_pinned = BTreeMap::new();
    existing_pinned.insert(
        "https://dl.static-php.dev/static-php-cli/common/php-8.4.23-cli-linux-x86_64.tar.gz"
            .to_owned(),
        "0".repeat(64),
    );
    let err = generate(&src, &smoke, SystemTime::UNIX_EPOCH, &existing_pinned)
        .expect_err("must reject changed hash for unchanged URL");
    assert!(err.contains("pinned SHA-256 changed"), "{err}");
}

#[test]
fn partial_smoke_failure_skips_artifact_without_failing_whole_run() {
    let src = build_complete_fixture_source();
    let smoke = MockSmokeRunner {
        fail_php_version: Some("8.5.8".to_owned()),
        fail_node_version: None,
    };
    let report =
        generate(&src, &smoke, SystemTime::UNIX_EPOCH, &BTreeMap::new()).expect("partial ok");
    validate_document(&report.document).expect("valid");
    assert_eq!(report.skipped.len(), 2); // common + bulk for 8.5.8
}

#[test]
fn safe_extract_tar_gz_rejects_tar_slip_escaping_symlink_and_setuid() {
    let dir = tempfile::tempdir().expect("tempdir");

    // 1. Parent traversal `../evil`
    let bad_traversal = dir.path().join("traversal.tar.gz");
    {
        let file = std::fs::File::create(&bad_traversal).expect("create");
        let enc = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(enc);
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o644);
        header.set_cksum();
        // Write raw path bytes bypassing tar::Builder's own check.
        let path_bytes = b"../evil.txt";
        header.as_gnu_mut().expect("gnu").name[..path_bytes.len()].copy_from_slice(path_bytes);
        header.set_cksum();
        builder
            .append(&header, &b"evil"[..])
            .expect("append traversal");
        builder.into_inner().expect("finish").finish().expect("gz");
    }
    let dest1 = dir.path().join("dest1");
    std::fs::create_dir_all(&dest1).expect("mkdir");
    let err = safe_extract_tar_gz(&bad_traversal, &dest1, 1024 * 1024).expect_err("reject ..");
    assert!(err.contains("escaping path"), "{err}");

    // 2. Escaping symlink
    let bad_symlink = dir.path().join("symlink.tar.gz");
    {
        let file = std::fs::File::create(&bad_symlink).expect("create");
        let enc = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(enc);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        builder
            .append_link(&mut header, "bin/escape", "../../etc/passwd")
            .expect("append link");
        builder.into_inner().expect("finish").finish().expect("gz");
    }
    let dest2 = dir.path().join("dest2");
    std::fs::create_dir_all(&dest2).expect("mkdir");
    let err = safe_extract_tar_gz(&bad_symlink, &dest2, 1024 * 1024).expect_err("reject symlink");
    assert!(err.contains("escaping extract root"), "{err}");

    // 3. Setuid bit
    let bad_setuid = dir.path().join("setuid.tar.gz");
    {
        let file = std::fs::File::create(&bad_setuid).expect("create");
        let enc = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(enc);
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(0o4755);
        header.set_cksum();
        builder
            .append_data(&mut header, "bin/suid", &b"hi"[..])
            .expect("append suid");
        builder.into_inner().expect("finish").finish().expect("gz");
    }
    let dest3 = dir.path().join("dest3");
    std::fs::create_dir_all(&dest3).expect("mkdir");
    let err = safe_extract_tar_gz(&bad_setuid, &dest3, 1024 * 1024).expect_err("reject setuid");
    assert!(err.contains("setuid/setgid"), "{err}");

    // 4. Extracted size cap
    let big_archive = dir.path().join("big.tar.gz");
    {
        let file = std::fs::File::create(&big_archive).expect("create");
        let enc = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(enc);
        let payload = vec![b'x'; 2048];
        let mut header = tar::Header::new_gnu();
        header.set_size(payload.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "big.bin", payload.as_slice())
            .expect("append big");
        let mut inner = builder.into_inner().expect("finish");
        inner.flush().expect("flush");
        inner.finish().expect("gz");
    }
    let dest4 = dir.path().join("dest4");
    std::fs::create_dir_all(&dest4).expect("mkdir");
    let err = safe_extract_tar_gz(&big_archive, &dest4, 512).expect_err("reject oversize");
    assert!(err.contains("maximum extracted size"), "{err}");
}

#[test]
fn validate_document_rejects_non_https_and_bad_sha256() {
    let mut doc = schema::assemble(
        vec![schema::ToolEntry {
            id: "composer".to_owned(),
            versions: vec![schema::ToolVersion {
                version: "2.11.3".to_owned(),
                series: "2".to_owned(),
                lts: None,
                artifacts: vec![schema::ToolArtifact {
                    os: "any".to_owned(),
                    arch: "any".to_owned(),
                    profile: None,
                    format: "phar".to_owned(),
                    url: "http://getcomposer.org/download/2.11.3/composer.phar".to_owned(),
                    sha256: "a".repeat(64),
                    size_bytes: 1234,
                    source: "getcomposer.org".to_owned(),
                    hash_origin: "upstream".to_owned(),
                    extensions: None,
                }],
            }],
        }],
        "2026-11-27T00:00:00Z".to_owned(),
    );
    let err = validate_document(&doc).expect_err("non-https rejected");
    assert!(err.contains("non-HTTPS"), "{err}");

    doc.tools[0].versions[0].artifacts[0].url =
        "https://getcomposer.org/download/2.11.3/composer.phar".to_owned();
    doc.tools[0].versions[0].artifacts[0].sha256 = "not-a-hash".to_owned();
    let err = validate_document(&doc).expect_err("bad sha256 rejected");
    assert!(err.contains("invalid sha256"), "{err}");
}

#[test]
fn committed_toolchains_catalog_is_valid_and_deterministic() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../catalog/toolchains-v1.json"
    ));
    let raw = std::fs::read_to_string(path).expect("read committed toolchains-v1.json");
    assert!(raw.ends_with('\n'), "missing trailing newline");
    let doc: ToolchainsDocument = serde_json::from_str(&raw).expect("parse toolchains-v1.json");
    validate_document(&doc).expect("committed toolchains catalog must validate");
    assert_eq!(doc.tools.len(), 3);
}
