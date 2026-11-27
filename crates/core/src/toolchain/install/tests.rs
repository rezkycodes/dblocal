//! Install/remove tests against a loopback fixture server and generated archives.
//!
//! Nothing here touches the network or a real engine: every archive is built in
//! memory, served from `127.0.0.1:0`, and the install runs in a temp home.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::download::UrlPolicy;
use crate::platform::{Arch, Os};
use crate::toolchain::catalog::{Artifact, Format, HashOrigin, ArtifactOs, ToolId};
use crate::toolchain::fsops::ToolchainPaths;
use crate::toolchain::install::{
    InstallKind, InstallRequest, RemoveOptions, expected_binaries, install, node_root_dir, remove,
    strip_prefix_for_test,
};
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Fixture server
// ---------------------------------------------------------------------------

struct Server {
    addr: std::net::SocketAddr,
    hits: Arc<AtomicUsize>,
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

/// Serve `body` at every path; a request for `/truncate` closes mid-body once.
async fn start(body: Vec<u8>, truncate_first: bool) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            let body = body.clone();
            // Truncate every response so retries cannot mask the failure.
            let do_truncate = truncate_first;
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                if do_truncate {
                    let cut = body.len() / 2;
                    let _ = socket.write_all(&body[..cut]).await;
                } else {
                    let _ = socket.write_all(&body).await;
                }
                let _ = socket.flush().await;
            });
        }
    });
    Server { addr, hits }
}

// ---------------------------------------------------------------------------
// Archive builders
// ---------------------------------------------------------------------------

enum Entry {
    Dir(String),
    File(String, Vec<u8>, u32),
    Symlink(String, String),
    /// A regular file whose header name is written verbatim, so a `..`/absolute
    /// path the tar builder would refuse can still be produced.
    RawFile(String, Vec<u8>),
}

/// `Entry` constructors that take borrowed data, so call sites stay terse.
fn ed_dir(path: impl Into<String>) -> Entry {
    Entry::Dir(path.into())
}
fn ed_file(path: impl Into<String>, data: &[u8], mode: u32) -> Entry {
    Entry::File(path.into(), data.to_vec(), mode)
}
fn ed_symlink(path: impl Into<String>, target: impl Into<String>) -> Entry {
    Entry::Symlink(path.into(), target.into())
}
fn ed_escape(path: impl Into<String>, data: &[u8]) -> Entry {
    Entry::RawFile(path.into(), data.to_vec())
}

fn tar_gz(entries: Vec<Entry>) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let enc = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
        let mut ar = tar::Builder::new(enc);
        for entry in entries {
            match entry {
                Entry::Dir(path) => {
                    let mut h = tar::Header::new_gnu();
                    h.set_entry_type(tar::EntryType::Directory);
                    h.set_mode(0o755);
                    h.set_size(0);
                    ar.append_data(&mut h, &path, std::io::empty()).expect("dir");
                }
                Entry::File(path, data, mode) => {
                    let mut h = tar::Header::new_gnu();
                    h.set_entry_type(tar::EntryType::Regular);
                    h.set_mode(mode);
                    h.set_size(data.len() as u64);
                    ar.append_data(&mut h, &path, &data[..]).expect("file");
                }
                Entry::Symlink(path, target) => {
                    let mut h = tar::Header::new_gnu();
                    h.set_entry_type(tar::EntryType::Symlink);
                    h.set_size(0);
                    ar.append_link(&mut h, &path, &target).expect("symlink");
                }
                Entry::RawFile(path, data) => {
                    // A header that keeps an unsafe name the builder would refuse.
                    let mut h = tar::Header::new_gnu();
                    h.set_entry_type(tar::EntryType::Regular);
                    h.set_mode(0o644);
                    h.set_size(data.len() as u64);
                    {
                        let gnu = h.as_gnu_mut().expect("gnu header");
                        let bytes = path.as_bytes();
                        let n = bytes.len().min(gnu.name.len());
                        gnu.name[..n].copy_from_slice(&bytes[..n]);
                    }
                    h.set_cksum();
                    // `append` takes the header as-is; it does not re-validate the
                    // name, unlike `append_data`.
                    ar.append(&h, &data[..]).expect("escape");
                }
            }
        }
        let enc = ar.into_inner().expect("tar finish");
        enc.finish().expect("gz finish");
    }
    out
}

fn zip(entries: Vec<Entry>) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for entry in entries {
        use zip::write::SimpleFileOptions;
        match entry {
            Entry::Dir(path) => {
                w.add_directory(&path, SimpleFileOptions::default()).expect("dir");
            }
            Entry::File(path, data, mode) => {
                let opts = SimpleFileOptions::default().unix_permissions(mode);
                w.start_file(&path, opts).expect("file");
                w.write_all(&data).expect("write");
            }
            Entry::Symlink(path, target) => {
                w.add_symlink(&path, &target, SimpleFileOptions::default()).expect("symlink");
            }
            Entry::RawFile(path, data) => {
                // zip accepts a `..` name, modelling a zip-slip.
                let opts = SimpleFileOptions::default();
                w.start_file(&path, opts).expect("escape");
                w.write_all(&data).expect("write");
            }
        }
    }
    w.finish().expect("zip finish").into_inner()
}

fn sha_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn artifact_for(format: Format, url: String, sha: String, size: u64) -> Artifact {
    Artifact::new(
        ArtifactOs::Linux,
        Arch::X86_64,
        None,
        format,
        "https://example.test/placeholder",
        sha,
        size,
        "test",
        HashOrigin::Pinned,
        Vec::new(),
    )
    .expect("artifact")
    .with_url_for_test(url)
}

fn loopback() -> UrlPolicy {
    UrlPolicy::allow_loopback_http_for_tests()
}

fn home() -> (TempDir, ToolchainPaths) {
    let dir = tempdir().expect("tmp");
    let paths = ToolchainPaths::with_root(dir.path());
    (dir, paths)
}

use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn expected_binaries_come_from_product_code() {
    assert_eq!(expected_binaries(ToolId::Php, Os::Linux), vec!["bin/php"]);
    assert_eq!(expected_binaries(ToolId::Php, Os::Windows), vec!["bin/php.exe"]);
    assert_eq!(
        expected_binaries(ToolId::Node, Os::Linux),
        vec!["bin/node", "bin/npm", "bin/npx"]
    );
    assert_eq!(expected_binaries(ToolId::Composer, Os::Linux), vec!["composer.phar"]);
}

#[test]
fn node_wrapper_is_stripped_and_pinned_to_product_code() {
    assert_eq!(node_root_dir("24.11.0"), "node-v24.11.0-linux-x64");
    assert_eq!(
        strip_prefix_for_test(ToolId::Node, Format::TarGz, "24.11.0", Os::Linux),
        Some("node-v24.11.0-linux-x64".to_owned())
    );
    assert_eq!(strip_prefix_for_test(ToolId::Php, Format::TarGz, "8.4.15", Os::Linux), None);
}

#[tokio::test]
async fn installs_a_valid_tarball() {
    let wrapper = node_root_dir("24.11.0");
    let data = tar_gz(vec![
        ed_dir(format!("{wrapper}/bin")),
        ed_file(format!("{wrapper}/bin/node"), b"#!/bin/sh\n", 0o755),
        ed_file(format!("{wrapper}/bin/npm"), b"#!/bin/sh\n", 0o755),
        ed_file(format!("{wrapper}/bin/npx"), b"#!/bin/sh\n", 0o755),
    ]);
    let server = start(data.clone(), false).await;
    let (_dir, paths) = home();
    let artifact = artifact_for(Format::TarGz, server.url("/node.tar.gz"), sha_hex(&data), data.len() as u64);

    let mut request = InstallRequest::new(ToolId::Node, &artifact, "24.11.0", None);
    request.policy = Some(loopback());
    let outcome = install(&paths, Os::Linux, Arch::X86_64, request).await.expect("install");

    assert_eq!(outcome.kind, InstallKind::Fresh);
    assert!(outcome.dir.join("bin/node").is_file());
    assert!(paths.install_dir(ToolId::Node, "24.11.0", None).join("dblocal-install.json").is_file());
}

#[tokio::test]
async fn install_strips_the_node_wrapper_directory() {
    let wrapper = node_root_dir("24.11.0");
    let data = tar_gz(vec![
        ed_dir(&format!("{wrapper}/bin")),
        ed_file(&format!("{wrapper}/bin/node"), b"x", 0o755),
        ed_file(&format!("{wrapper}/bin/npm"), b"x", 0o755),
        ed_file(&format!("{wrapper}/bin/npx"), b"x", 0o755),
    ]);
    let server = start(data.clone(), false).await;
    let (_dir, paths) = home();
    let artifact = artifact_for(Format::TarGz, server.url("/n.tgz"), sha_hex(&data), data.len() as u64);
    let mut request = InstallRequest::new(ToolId::Node, &artifact, "24.11.0", None);
    request.policy = Some(loopback());
    let outcome = install(&paths, Os::Linux, Arch::X86_64, request).await.expect("install");
    assert!(outcome.dir.join("bin/node").is_file(), "wrapper must be stripped");
    assert!(!outcome.dir.join(&wrapper).exists());
}

#[tokio::test]
async fn install_is_idempotent_and_writes_nothing_the_second_time() {
    let data = tar_gz(vec![
        ed_dir("bin"),
        ed_file("bin/php", b"x", 0o755),
    ]);
    let server = start(data.clone(), false).await;
    let (_dir, paths) = home();
    let artifact = artifact_for(Format::TarGz, server.url("/php.tgz"), sha_hex(&data), data.len() as u64);

    let mut first = InstallRequest::new(ToolId::Php, &artifact, "8.4.15", Some("common"));
    first.policy = Some(loopback());
    install(&paths, Os::Linux, Arch::X86_64, first).await.expect("first");
    let hits = server.hits();

    let mut second = InstallRequest::new(ToolId::Php, &artifact, "8.4.15", Some("common"));
    second.policy = Some(loopback());
    let outcome = install(&paths, Os::Linux, Arch::X86_64, second).await.expect("second");
    assert_eq!(outcome.kind, InstallKind::AlreadyInstalled);
    assert_eq!(server.hits(), hits, "no second download");
}

#[tokio::test]
async fn a_wrong_hash_leaves_no_install() {
    let data = tar_gz(vec![ed_dir("bin"), ed_file("bin/php", b"x", 0o755)]);
    let server = start(data.clone(), false).await;
    let (_dir, paths) = home();
    let artifact = artifact_for(
        Format::TarGz,
        server.url("/x.tgz"),
        "0".repeat(64),
        data.len() as u64,
    );
    let mut request = InstallRequest::new(ToolId::Php, &artifact, "8.4.15", Some("common"));
    request.policy = Some(loopback());
    let err = install(&paths, Os::Linux, Arch::X86_64, request).await.expect_err("bad hash");
    assert!(matches!(err, crate::DblError::ChecksumMismatch { .. }), "{err:?}");
    assert!(!paths.install_dir(ToolId::Php, "8.4.15", Some("common")).exists());
    assert_no_debris(&paths, "8.4.15-common");
}

#[tokio::test]
async fn a_tar_slip_is_rejected_and_leaves_no_install() {
    let data = tar_gz(vec![
        ed_dir("bin"),
        ed_file("bin/php", b"x", 0o755),
        ed_escape("../escape.txt", b"evil"),
    ]);
    // A `..` in a tar entry is rejected by the extractor.
    let server = start(data.clone(), false).await;
    let (_dir, paths) = home();
    let artifact = artifact_for(Format::TarGz, server.url("/s.tgz"), sha_hex(&data), data.len() as u64);
    let mut request = InstallRequest::new(ToolId::Php, &artifact, "8.4.15", Some("common"));
    request.policy = Some(loopback());
    let err = install(&paths, Os::Linux, Arch::X86_64, request).await.expect_err("tar slip");
    assert!(matches!(err, crate::DblError::UnsafeArchive { .. }), "{err:?}");
    assert!(!paths.install_dir(ToolId::Php, "8.4.15", Some("common")).exists());
    assert_no_debris(&paths, "8.4.15-common");
    // Nothing escaped into the home root either.
    assert!(!paths.root().join("escape.txt").exists());
}

#[tokio::test]
async fn a_zip_slip_is_rejected() {
    let data = zip(vec![
        ed_dir("bin"),
        ed_file("bin/php.exe", b"x", 0o755),
        ed_escape("../escape.txt", b"evil"),
    ]);
    let server = start(data.clone(), false).await;
    let (_dir, paths) = home();
    let artifact = artifact_for(Format::Zip, server.url("/z.zip"), sha_hex(&data), data.len() as u64);
    let mut request = InstallRequest::new(ToolId::Php, &artifact, "8.4.15", None);
    request.policy = Some(loopback());
    let err = install(&paths, Os::Windows, Arch::X86_64, request).await.expect_err("zip slip");
    assert!(matches!(err, crate::DblError::UnsafeArchive { .. }), "{err:?}");
    assert!(!paths.install_dir(ToolId::Php, "8.4.15", None).exists());
}

#[tokio::test]
async fn a_symlink_escape_is_rejected() {
    let data = tar_gz(vec![
        ed_dir("bin"),
        ed_file("bin/php", b"x", 0o755),
        ed_symlink("bin/evil", "/etc/passwd"),
    ]);
    let server = start(data.clone(), false).await;
    let (_dir, paths) = home();
    let artifact = artifact_for(Format::TarGz, server.url("/l.tgz"), sha_hex(&data), data.len() as u64);
    let mut request = InstallRequest::new(ToolId::Php, &artifact, "8.4.15", Some("common"));
    request.policy = Some(loopback());
    let err = install(&paths, Os::Linux, Arch::X86_64, request).await.expect_err("symlink escape");
    assert!(matches!(err, crate::DblError::UnsafeArchive { .. }), "{err:?}");
    assert_no_debris(&paths, "8.4.15-common");
}

#[tokio::test]
async fn a_truncated_body_is_rejected() {
    let data = tar_gz(vec![ed_dir("bin"), ed_file("bin/php", b"hello world", 0o755)]);
    let server = start(data.clone(), true).await;
    let (_dir, paths) = home();
    let artifact = artifact_for(Format::TarGz, server.url("/t.tgz"), sha_hex(&data), data.len() as u64);
    let mut request = InstallRequest::new(ToolId::Php, &artifact, "8.4.15", Some("common"));
    request.policy = Some(loopback());
    let err = install(&paths, Os::Linux, Arch::X86_64, request).await.expect_err("truncated");
    // Either a network or checksum failure; both leave nothing installed.
    assert!(
        matches!(err, crate::DblError::Network { .. } | crate::DblError::ChecksumMismatch { .. }),
        "{err:?}"
    );
    assert_no_debris(&paths, "8.4.15-common");
}

#[tokio::test]
async fn a_missing_expected_binary_is_rejected() {
    let data = tar_gz(vec![ed_file("README", b"no binary here", 0o644)]);
    let server = start(data.clone(), false).await;
    let (_dir, paths) = home();
    let artifact = artifact_for(Format::TarGz, server.url("/m.tgz"), sha_hex(&data), data.len() as u64);
    let mut request = InstallRequest::new(ToolId::Php, &artifact, "8.4.15", Some("common"));
    request.policy = Some(loopback());
    let err = install(&paths, Os::Linux, Arch::X86_64, request).await.expect_err("missing binary");
    assert!(matches!(err, crate::DblError::InvalidInput { .. }), "{err:?}");
    assert_no_debris(&paths, "8.4.15-common");
}

#[tokio::test]
async fn installs_a_phar_as_a_single_file() {
    let phar = b"<?php // fake composer phar\n";
    let server = start(phar.to_vec(), false).await;
    let (_dir, paths) = home();
    let artifact = artifact_for(
        Format::Phar,
        server.url("/composer.phar"),
        sha_hex(phar),
        phar.len() as u64,
    );
    let mut request = InstallRequest::new(ToolId::Composer, &artifact, "2.8.4", None);
    request.policy = Some(loopback());
    let outcome = install(&paths, Os::Linux, Arch::X86_64, request).await.expect("phar");
    assert!(outcome.dir.join("composer.phar").is_file());
    assert_eq!(
        std::fs::read(outcome.dir.join("composer.phar")).unwrap(),
        phar
    );
}

#[tokio::test]
async fn remove_clears_the_active_entry_and_reports_others() {
    let data = tar_gz(vec![ed_dir("bin"), ed_file("bin/php", b"x", 0o755)]);
    let server = start(data.clone(), false).await;
    let (_dir, paths) = home();

    // Install two versions, activate one.
    for version in ["8.4.15", "8.3.2"] {
        let artifact = artifact_for(Format::TarGz, server.url("/p.tgz"), sha_hex(&data), data.len() as u64);
        let mut request = InstallRequest::new(ToolId::Php, &artifact, version, Some("common"));
        request.policy = Some(loopback());
        install(&paths, Os::Linux, Arch::X86_64, request).await.expect("install");
    }
    crate::toolchain::registry::write_registry(
        &paths,
        &crate::toolchain::registry::Registry::empty().with_active(
            crate::toolchain::registry::ActiveVersion::new(ToolId::Php, "8.4.15", Some("common".into())),
        ),
    )
    .expect("registry");

    let outcome = remove(&paths, ToolId::Php, "8.4.15", Some("common"), RemoveOptions::default())
        .await
        .expect("remove");
    assert!(outcome.cleared_active.is_some());
    assert!(outcome.others_remain, "8.3.2 is still installed");
    assert!(!paths.install_dir(ToolId::Php, "8.4.15", Some("common")).exists());
    assert!(paths.install_dir(ToolId::Php, "8.3.2", Some("common")).exists());
    assert!(
        crate::toolchain::registry::read_registry(&paths).get(ToolId::Php).is_none(),
        "active entry cleared"
    );
}

#[tokio::test]
async fn removing_a_missing_version_is_a_not_found() {
    let (_dir, paths) = home();
    let err = remove(&paths, ToolId::Php, "8.4.15", None, RemoveOptions::default())
        .await
        .expect_err("missing");
    assert!(matches!(err, crate::DblError::NotFound { .. }), "{err:?}");
}

#[tokio::test]
async fn removing_a_non_active_version_keeps_the_registry() {
    let (_dir, paths) = home();
    for version in ["24.11.0", "22.1.0"] {
        let wrapper = node_root_dir(version);
        let data = tar_gz(vec![
            ed_dir(format!("{wrapper}/bin")),
            ed_file(format!("{wrapper}/bin/node"), b"x", 0o755),
            ed_file(format!("{wrapper}/bin/npm"), b"x", 0o755),
            ed_file(format!("{wrapper}/bin/npx"), b"x", 0o755),
        ]);
        let server = start(data.clone(), false).await;
        let artifact = artifact_for(Format::TarGz, server.url("/n.tgz"), sha_hex(&data), data.len() as u64);
        let mut request = InstallRequest::new(ToolId::Node, &artifact, version, None);
        request.policy = Some(loopback());
        install(&paths, Os::Linux, Arch::X86_64, request).await.expect("install");
    }
    crate::toolchain::registry::write_registry(
        &paths,
        &crate::toolchain::registry::Registry::empty()
            .with_active(crate::toolchain::registry::ActiveVersion::new(ToolId::Node, "24.11.0", None)),
    )
    .expect("registry");

    let outcome = remove(&paths, ToolId::Node, "22.1.0", None, RemoveOptions::default())
        .await
        .expect("remove non-active");
    assert!(outcome.cleared_active.is_none());
    assert!(crate::toolchain::registry::read_registry(&paths).get(ToolId::Node).is_some());
}

/// Asserts no staging/removing debris was left beside an install leaf.
fn assert_no_debris(paths: &ToolchainPaths, leaf: &str) {
    let tool_dir = paths.toolchains_dir();
    for tool in ToolId::all() {
        let dir = tool_dir.join(tool.as_str());
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            assert!(
                !name.contains(leaf) || !name.starts_with('.'),
                "leftover debris {name} in {}",
                dir.display()
            );
        }
    }
}
