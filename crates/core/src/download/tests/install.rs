//! Disk-space preflight, the install entry points, and end-to-end installation.

use std::path::Path;
use std::sync::Arc;

use tempfile::tempdir;

use crate::model::{ArchiveFormat, Artifact, EngineKey};
use crate::paths::DblPaths;
use crate::platform::{Arch, Os};

use super::super::fetch::UrlPolicy;
use super::super::space::{DISK_SPACE_MULTIPLIER, FreeSpaceFn, check_disk_space};
use super::super::verify::sha256_bytes;
use super::super::{InstallOptions, Progress, SourceSpec};
use super::{ServeConfig, TarEntry, make_tar_gz, start_server};

fn probe_zero() -> FreeSpaceFn {
    Arc::new(|_| Ok(0))
}

fn https_artifact() -> Artifact {
    Artifact::new(
        "postgres-18.7.0-linux-x86_64",
        EngineKey::Postgresql,
        "18.7.0",
        Os::Linux,
        Arch::X86_64,
        "https://example.test/postgres.tar.gz",
        "a".repeat(64),
        1024,
        ArchiveFormat::TarGz,
    )
    .expect("valid artifact")
    .with_strip_prefix("postgresql-18.7.0-x86_64-unknown-linux-gnu")
}

#[test]
fn disk_space_requires_three_times_and_states_amounts() {
    assert_eq!(DISK_SPACE_MULTIPLIER, 3);
    let dir = tempdir().expect("tmp");
    let probe: FreeSpaceFn = Arc::new(|_| Ok(100));
    let err = check_disk_space(dir.path(), 100, Some(&probe)).expect_err("insufficient");
    let msg = err.to_string();
    assert!(msg.contains("insufficient disk space"), "{msg}");
    assert!(msg.contains("300"), "must state required bytes: {msg}");
    assert!(msg.contains("100"), "must state available bytes: {msg}");
    // Human-readable forms are present too, so the UI can show them as-is.
    assert!(msg.contains("KiB") || msg.contains(" B"), "{msg}");
}

#[test]
fn disk_space_allows_when_ample() {
    let dir = tempdir().expect("tmp");
    let probe: FreeSpaceFn = Arc::new(|_| Ok(1024));
    let ok = check_disk_space(dir.path(), 100, Some(&probe)).expect("ample");
    assert_eq!(ok.required, 300);
    assert_eq!(ok.available, 1024);
}

#[test]
fn disk_space_uses_real_probe_when_none_is_injected() {
    // Exercises `available_space_for_path` against the live filesystem; every
    // platform we ship on reports some free space for the temp dir.
    let dir = tempdir().expect("tmp");
    let result = check_disk_space(dir.path(), 1, None).expect("real probe");
    assert!(result.available > 0);
    assert_eq!(result.required, 3);
}

#[test]
fn downloads_dir_and_part_path_are_deterministic() {
    let paths = DblPaths::with_root(Path::new("/tmp/dblocal-root"));
    assert_eq!(
        super::super::fetch::downloads_dir(&paths),
        Path::new("/tmp/dblocal-root").join(super::super::fetch::DOWNLOADS_DIR_NAME)
    );
    let artifact = https_artifact();
    assert_eq!(
        super::super::fetch::part_path_for_label(Path::new("/tmp/dl"), artifact.id()),
        Path::new("/tmp/dl/postgres-18.7.0-linux-x86_64.part")
    );
    assert_eq!(
        super::super::fetch::etag_sidecar_path(Path::new("/tmp/dl/x.part")),
        Path::new("/tmp/dl/x.part.etag")
    );
}

#[test]
fn source_spec_mirrors_the_artifact() {
    let artifact = https_artifact();
    let spec = SourceSpec::from_artifact(&artifact);
    assert_eq!(spec.url, artifact.url());
    assert_eq!(spec.label, artifact.id());
    assert_eq!(spec.sha256, artifact.sha256());
    assert_eq!(spec.size_bytes, artifact.size_bytes());
    assert_eq!(spec.format, artifact.format());
    assert_eq!(spec.strip_prefix, artifact.strip_prefix());
}

#[test]
fn install_options_builders_and_debug_are_usable() {
    let sink: super::super::ProgressSink = Arc::new(|_| {});
    let options = InstallOptions::default()
        .with_progress(sink)
        .with_policy(UrlPolicy::allow_loopback_http_for_tests())
        .with_cancel(super::super::CancellationToken::new())
        .with_free_space(probe_zero());
    let rendered = format!("{options:?}");
    assert!(rendered.contains("InstallOptions"), "{rendered}");
    assert!(rendered.contains("<sink>"), "{rendered}");
    assert!(rendered.contains("<probe>"), "{rendered}");
}

#[test]
fn progress_alias_is_the_domain_type() {
    let p: Progress = crate::model::DownloadProgress::new(1, 2, 3, None);
    assert_eq!(p.bytes_done, 1);
}

#[tokio::test]
async fn install_runtime_derives_the_runtime_path_and_refuses_on_full_disk() {
    let artifact = https_artifact();
    let dir = tempdir().expect("tmp");
    let paths = DblPaths::with_root(dir.path());
    let options = InstallOptions::default().with_free_space(probe_zero());

    let err = super::super::install_runtime(&artifact, &paths, &["bin/postgres"], options)
        .await
        .expect_err("full disk must refuse");

    assert!(err.to_string().contains("insufficient disk space"), "{err}");
    // The downloads directory is created before the check so the probe has a
    // real path to measure.
    assert!(super::super::fetch::downloads_dir(&paths).is_dir());
    // No runtime directory may exist.
    let runtime_dir = paths
        .runtime_dir(artifact.engine(), artifact.version(), artifact.arch())
        .expect("runtime path");
    assert!(!runtime_dir.exists());
}

#[tokio::test]
async fn install_runtime_to_delegates_and_refuses_on_full_disk() {
    let artifact = https_artifact();
    let dir = tempdir().expect("tmp");
    let downloads = dir.path().join("downloads");
    let final_dir = dir.path().join("runtime");
    let options = InstallOptions::default().with_free_space(probe_zero());

    let err = super::super::install_runtime_to(
        &artifact,
        &downloads,
        &final_dir,
        &["bin/postgres"],
        options,
    )
    .await
    .expect_err("full disk must refuse");

    assert!(err.to_string().contains("insufficient disk space"), "{err}");
    assert!(!final_dir.exists());
}

#[tokio::test]
async fn install_source_rejects_an_unparseable_url() {
    let dir = tempdir().expect("tmp");
    let source = SourceSpec {
        url: "not a url",
        label: "test-artifact",
        sha256: "a".repeat(64).leak(),
        size_bytes: 16,
        format: ArchiveFormat::TarGz,
        strip_prefix: None,
    };
    let err = super::super::install_source_to(
        &source,
        &dir.path().join("downloads"),
        &dir.path().join("runtime"),
        &[],
        InstallOptions::default(),
    )
    .await
    .expect_err("invalid URL");
    assert!(
        matches!(err, crate::DblError::InvalidInput { .. }),
        "{err:?}"
    );
}

fn valkey_tarball() -> Vec<u8> {
    make_tar_gz(vec![
        TarEntry::Dir {
            path: "valkey-9.2.0".to_owned(),
            mode: 0o755,
        },
        TarEntry::Dir {
            path: "valkey-9.2.0/bin".to_owned(),
            mode: 0o755,
        },
        TarEntry::File {
            path: "valkey-9.2.0/bin/valkey-server".to_owned(),
            data: b"binary".to_vec(),
            mode: 0o755,
        },
    ])
}

#[tokio::test]
async fn install_end_to_end_cleans_part_and_sidecar() {
    let data = valkey_tarball();
    let expected = sha256_bytes(&data);
    let server = start_server(ServeConfig::normal(data.clone())).await;
    let url = server.url("/valkey.tar.gz");

    let dir = tempdir().expect("tmp");
    let downloads = dir.path().join("downloads");
    let final_dir = dir.path().join("runtimes/valkey/9.2.0");
    let options = InstallOptions::default().with_policy(UrlPolicy::allow_loopback_http_for_tests());
    let source = SourceSpec {
        url: &url,
        label: "valkey-9.2.0-linux-x86_64",
        sha256: &expected,
        size_bytes: data.len() as u64,
        format: ArchiveFormat::TarGz,
        strip_prefix: Some("valkey-9.2.0"),
    };

    let installed = super::super::install_source_to(
        &source,
        &downloads,
        &final_dir,
        &["bin/valkey-server"],
        options,
    )
    .await
    .expect("install ok");

    assert_eq!(installed, final_dir);
    assert!(final_dir.join("bin/valkey-server").exists());
    let part = super::super::fetch::part_path_for_label(&downloads, source.label);
    assert!(!part.exists());
    assert!(!super::super::fetch::etag_sidecar_path(&part).exists());
}

#[tokio::test]
async fn install_checksum_failure_leaves_nothing_installed() {
    let body = b"payload".to_vec();
    let bad = sha256_bytes(b"other");
    let server = start_server(ServeConfig::normal(body.clone())).await;
    let url = server.url("/v.tar.gz");

    let dir = tempdir().expect("tmp");
    let downloads = dir.path().join("downloads");
    let final_dir = dir.path().join("runtime");
    let options = InstallOptions::default().with_policy(UrlPolicy::allow_loopback_http_for_tests());
    let source = SourceSpec {
        url: &url,
        label: "test-artifact",
        sha256: &bad,
        size_bytes: body.len() as u64,
        format: ArchiveFormat::TarGz,
        strip_prefix: None,
    };

    let err = super::super::install_source_to(&source, &downloads, &final_dir, &["bin/x"], options)
        .await
        .expect_err("checksum failure must abort");
    assert!(
        matches!(err, crate::DblError::ChecksumMismatch { .. }),
        "{err:?}"
    );
    assert!(!final_dir.exists(), "no runtime after checksum failure");
    assert!(!super::super::fetch::part_path_for_label(&downloads, source.label).exists());
}

#[tokio::test]
async fn install_refuses_before_any_request_when_disk_is_full() {
    let body = b"payload".to_vec();
    let server = start_server(ServeConfig::normal(body.clone())).await;
    let expected = sha256_bytes(&body);
    let url = server.url("/v.tar.gz");

    let dir = tempdir().expect("tmp");
    let downloads = dir.path().join("downloads");
    let final_dir = dir.path().join("runtime");
    let options = InstallOptions::default()
        .with_policy(UrlPolicy::allow_loopback_http_for_tests())
        .with_free_space(probe_zero());
    let source = SourceSpec {
        url: &url,
        label: "test-artifact",
        sha256: &expected,
        size_bytes: body.len() as u64,
        format: ArchiveFormat::TarGz,
        strip_prefix: None,
    };

    let err = super::super::install_source_to(&source, &downloads, &final_dir, &["bin/x"], options)
        .await
        .expect_err("disk full must refuse");
    assert!(err.to_string().contains("insufficient disk space"), "{err}");
    assert_eq!(server.hits(), 0, "no request may be issued");
    assert!(!super::super::fetch::part_path_for_label(&downloads, source.label).exists());
}

#[tokio::test]
async fn install_missing_expected_binary_leaves_no_runtime() {
    let data = make_tar_gz(vec![TarEntry::File {
        path: "root/README".to_owned(),
        data: b"hi".to_vec(),
        mode: 0o644,
    }]);
    let expected = sha256_bytes(&data);
    let server = start_server(ServeConfig::normal(data.clone())).await;
    let url = server.url("/v.tar.gz");

    let dir = tempdir().expect("tmp");
    let downloads = dir.path().join("downloads");
    let final_dir = dir.path().join("runtime");
    let options = InstallOptions::default().with_policy(UrlPolicy::allow_loopback_http_for_tests());
    let source = SourceSpec {
        url: &url,
        label: "test-artifact",
        sha256: &expected,
        size_bytes: data.len() as u64,
        format: ArchiveFormat::TarGz,
        strip_prefix: Some("root"),
    };

    let err =
        super::super::install_source_to(&source, &downloads, &final_dir, &["bin/nope"], options)
            .await
            .expect_err("missing binary must fail");
    assert!(
        matches!(err, crate::DblError::InvalidInput { .. }),
        "{err:?}"
    );
    assert!(!final_dir.exists());
}
