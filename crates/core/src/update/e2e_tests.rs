//! End-to-end offline tests for self-update using a local fake release server
//! and fake binaries in temporary prefixes. Never touches `~/.local` or any
//! running database instance.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

#[cfg(unix)]
use flate2::Compression;
#[cfg(unix)]
use flate2::write::GzEncoder;
use semver::Version;
#[cfg(unix)]
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::download::CancellationToken;
use crate::paths::DblPaths;
use crate::platform::{Arch, Os};
#[cfg(unix)]
use crate::service::ServiceEvent;
use crate::service::{DblService, LocalService, ServiceOptions};

#[cfg(unix)]
fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(unix)]
fn fake_binary_bytes(version: &str, ok: bool) -> Vec<u8> {
    let code = if ok { 0 } else { 1 };
    format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"dblocal {version}\"; exit {code}; fi\nexit 0\n"
    )
    .into_bytes()
}

#[cfg(unix)]
fn write_fake_exe(path: &Path, version: &str, ok: bool) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, fake_binary_bytes(version, ok)).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
fn build_linux_tarball(binary_bytes: &[u8], include_desktop: bool) -> Vec<u8> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        let mut header = tar::Header::new_gnu();
        header.set_size(binary_bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "dblocal", binary_bytes)
            .unwrap();

        if include_desktop {
            let desktop = b"[Desktop Entry]\nName=DBLocal\nExec=/usr/bin/dblocal\nTryExec=/usr/bin/dblocal\nIcon=dblocal\n";
            let mut h = tar::Header::new_gnu();
            h.set_size(desktop.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, DESKTOP_ENTRY_REL, &desktop[..])
                .unwrap();

            let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
            let mut h = tar::Header::new_gnu();
            h.set_size(svg.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, SVG_ICON_REL, &svg[..]).unwrap();

            let png = b"\x89PNG\r\n\x1a\nfake";
            let mut h = tar::Header::new_gnu();
            h.set_size(png.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(
                &mut h,
                "share/icons/hicolor/64x64/apps/dblocal.png",
                &png[..],
            )
            .unwrap();
        }
        tar.finish().unwrap();
    }
    gz.finish().unwrap()
}

fn spawn_fake_server(
    routes: HashMap<String, (u16, Vec<u8>)>,
    hits: Arc<AtomicUsize>,
) -> (String, tokio::task::JoinHandle<()>) {
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std_listener.set_nonblocking(true).unwrap();
    let addr = std_listener.local_addr().unwrap();
    let listener = TcpListener::from_std(std_listener).unwrap();
    let routes = Arc::new(routes);
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let routes = Arc::clone(&routes);
            let hits = Arc::clone(&hits);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let Ok(n) = socket.read(&mut buf).await else {
                    return;
                };
                hits.fetch_add(1, Ordering::SeqCst);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/");
                let (status, body) = routes
                    .get(path)
                    .cloned()
                    .unwrap_or_else(|| (404, b"not found".to_vec()));
                let head = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    (format!("http://{addr}"), handle)
}

#[test]
fn install_kind_detection_covers_linux_and_windows_paths() {
    // Windows per-user MSI (self-updatable) vs per-machine MSI (refused)
    let per_user = detect_for_os(
        Path::new("C:\\Users\\alice\\AppData\\Local\\DBLocal\\dblocal.exe"),
        Os::Windows,
    );
    assert!(matches!(per_user, InstallKind::WindowsMsi { .. }));
    assert!(per_user.can_self_update());
    assert_eq!(per_user.expected_asset_kind(), Some(UpdateAssetKind::Msi));

    let per_machine = detect_for_os(
        Path::new("C:\\Program Files\\DBLocal\\dblocal.exe"),
        Os::Windows,
    );
    assert!(matches!(per_machine, InstallKind::WindowsMsi { .. }));
    assert!(!per_machine.can_self_update());
    assert!(
        per_machine
            .refusal_reason()
            .unwrap()
            .contains("administrator")
    );

    // Windows portable zip
    let win_zip = detect_for_os(
        Path::new("C:\\Users\\alice\\Tools\\DBLocal\\dblocal.exe"),
        Os::Windows,
    );
    assert!(matches!(win_zip, InstallKind::WindowsZip { .. }));
    assert!(win_zip.can_self_update());
    assert_eq!(win_zip.expected_asset_kind(), Some(UpdateAssetKind::Zip));

    // Windows dev build
    let win_dev = detect_for_os(
        Path::new("C:\\repo\\target\\debug\\dblocal.exe"),
        Os::Windows,
    );
    assert!(matches!(win_dev, InstallKind::Unknown { .. }));
    assert!(!win_dev.can_self_update());

    // macOS deferred
    let mac = detect_for_os(
        Path::new("/Applications/DBLocal.app/Contents/MacOS/dblocal"),
        Os::Macos,
    );
    assert!(matches!(mac, InstallKind::MacApp { .. }));
    assert!(!mac.can_self_update());
    assert!(mac.refusal_reason().unwrap().contains("macOS"));

    // Linux dev build under shared CARGO_TARGET_DIR
    let linux_dev = detect_for_os(
        Path::new("/home/u/.cache/dblocal-target-gui/debug/dblocal"),
        Os::Linux,
    );
    assert!(matches!(linux_dev, InstallKind::Unknown { .. }));
    assert!(!linux_dev.can_self_update());
}

#[tokio::test]
async fn check_selects_asset_matching_install_kind_and_ignores_prereleases() {
    let json = serde_json::json!([
        {
            "tag_name": "v0.3.0-rc.1",
            "draft": false,
            "prerelease": true,
            "html_url": "https://github.com/rezkycodes/dblocal/releases/tag/v0.3.0-rc.1",
            "body": "rc notes",
            "assets": [
                {"name": "dblocal-v0.3.0-rc.1-linux-x86_64.tar.gz", "browser_download_url": "https://github.com/rezkycodes/dblocal/releases/download/v0.3.0-rc.1/dblocal-v0.3.0-rc.1-linux-x86_64.tar.gz", "size": 100},
                {"name": "checksums.txt", "browser_download_url": "https://github.com/rezkycodes/dblocal/releases/download/v0.3.0-rc.1/checksums.txt", "size": 80}
            ]
        },
        {
            "tag_name": "v0.2.0",
            "draft": false,
            "prerelease": false,
            "html_url": "https://github.com/rezkycodes/dblocal/releases/tag/v0.2.0",
            "body": "stable notes",
            "assets": [
                {"name": "dblocal-v0.2.0-linux-x86_64.AppImage", "browser_download_url": "https://github.com/rezkycodes/dblocal/releases/download/v0.2.0/dblocal-v0.2.0-linux-x86_64.AppImage", "size": 200},
                {"name": "dblocal-v0.2.0-linux-x86_64.tar.gz", "browser_download_url": "https://github.com/rezkycodes/dblocal/releases/download/v0.2.0/dblocal-v0.2.0-linux-x86_64.tar.gz", "size": 150},
                {"name": "dblocal_0.2.0_amd64.deb", "browser_download_url": "https://github.com/rezkycodes/dblocal/releases/download/v0.2.0/dblocal_0.2.0_amd64.deb", "size": 160},
                {"name": "dblocal-0.2.0-1.x86_64.rpm", "browser_download_url": "https://github.com/rezkycodes/dblocal/releases/download/v0.2.0/dblocal-0.2.0-1.x86_64.rpm", "size": 170},
                {"name": "checksums.txt", "browser_download_url": "https://github.com/rezkycodes/dblocal/releases/download/v0.2.0/checksums.txt", "size": 80}
            ]
        }
    ])
    .to_string();

    let hits = Arc::new(AtomicUsize::new(0));
    let mut routes = HashMap::new();
    routes.insert(
        "/repos/rezkycodes/dblocal/releases?per_page=20".to_owned(),
        (200, json.into_bytes()),
    );
    let (base, server) = spawn_fake_server(routes, hits);
    let fetcher = ReqwestUpdateFetcher::loopback_for_tests().unwrap();

    // PortableTarball -> .tar.gz (NOT AppImage!)
    let opts = UpdateCheckOptions::for_repo("rezkycodes/dblocal")
        .with_platform(Os::Linux, Arch::X86_64)
        .with_install_kind(InstallKind::PortableTarball {
            exe_path: "/home/u/.local/bin/dblocal".into(),
        })
        .with_api_base_url(&base);
    let res = check(&Version::parse("0.1.3").unwrap(), &fetcher, &opts)
        .await
        .unwrap();
    let avail = res.available().expect("v0.2.0 available");
    assert_eq!(avail.version(), &Version::parse("0.2.0").unwrap());
    assert_eq!(avail.asset().kind(), UpdateAssetKind::TarGz);
    assert_eq!(
        avail.release_url(),
        "https://github.com/rezkycodes/dblocal/releases/tag/v0.2.0"
    );

    // AppImage -> .AppImage
    let opts = opts.with_install_kind(InstallKind::AppImage {
        appimage_path: "/home/u/DBLocal.AppImage".into(),
    });
    let res = check(&Version::parse("0.1.3").unwrap(), &fetcher, &opts)
        .await
        .unwrap();
    assert_eq!(
        res.available().unwrap().asset().kind(),
        UpdateAssetKind::AppImage
    );

    // Dpkg -> .deb matching asset name for hint
    let opts = opts.with_install_kind(InstallKind::DistPackage {
        exe_path: "/usr/bin/dblocal".into(),
        hint: PackageManagerHint::Dpkg,
    });
    let res = check(&Version::parse("0.1.3").unwrap(), &fetcher, &opts)
        .await
        .unwrap();
    let deb = res.available().unwrap();
    assert_eq!(deb.asset().kind(), UpdateAssetKind::Deb);
    assert_eq!(deb.asset().name(), "dblocal_0.2.0_amd64.deb");

    // Rpm -> .rpm matching asset name for hint
    let opts = opts.with_install_kind(InstallKind::DistPackage {
        exe_path: "/usr/bin/dblocal".into(),
        hint: PackageManagerHint::Rpm,
    });
    let res = check(&Version::parse("0.1.3").unwrap(), &fetcher, &opts)
        .await
        .unwrap();
    let rpm = res.available().unwrap();
    assert_eq!(rpm.asset().kind(), UpdateAssetKind::Rpm);
    assert_eq!(rpm.asset().name(), "dblocal-0.2.0-1.x86_64.rpm");

    // Up to date when already on 0.2.0
    let res = check(&Version::parse("0.2.0").unwrap(), &fetcher, &opts)
        .await
        .unwrap();
    assert!(matches!(res, UpdateCheck::UpToDate { .. }));

    server.abort();
}

#[cfg(unix)]
#[test]
fn malicious_tarballs_are_rejected_before_touching_target() {
    let dir = tempdir().unwrap();

    // 1. Path traversal (..)
    let bad_path = dir.path().join("traversal.tar.gz");
    {
        let file = std::fs::File::create(&bad_path).unwrap();
        let gz = GzEncoder::new(file, Compression::default());
        let mut tar = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        let data = b"evil";
        h.set_size(data.len() as u64);
        h.set_mode(0o755);
        // Write raw path bytes to bypass tar::Builder's own sanitization
        let path_bytes = b"../evil";
        h.as_gnu_mut().unwrap().name[..path_bytes.len()].copy_from_slice(path_bytes);
        h.set_cksum();
        tar.append(&h, &data[..]).unwrap();
        tar.finish().unwrap();
    }
    let err = extract_linux_tarball(&bad_path, &dir.path().join("out1")).expect_err("traversal");
    assert_eq!(err.code(), CODE_UPDATE_UNSAFE_ARCHIVE);

    // 2. Symlink entry
    let sym_path = dir.path().join("symlink.tar.gz");
    {
        let file = std::fs::File::create(&sym_path).unwrap();
        let gz = GzEncoder::new(file, Compression::default());
        let mut tar = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        h.set_mode(0o777);
        tar.append_link(&mut h, "dblocal", "/bin/sh").unwrap();
        tar.finish().unwrap();
    }
    let err = extract_linux_tarball(&sym_path, &dir.path().join("out2")).expect_err("symlink");
    assert_eq!(err.code(), CODE_UPDATE_UNSAFE_ARCHIVE);

    // 3. Unexpected extra file
    let extra_path = dir.path().join("extra.tar.gz");
    {
        let file = std::fs::File::create(&extra_path).unwrap();
        let gz = GzEncoder::new(file, Compression::default());
        let mut tar = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        let data = b"#!/bin/sh\n";
        h.set_size(data.len() as u64);
        h.set_mode(0o755);
        h.set_cksum();
        tar.append_data(&mut h, "dblocal", &data[..]).unwrap();
        let mut h2 = tar::Header::new_gnu();
        h2.set_size(4);
        h2.set_mode(0o644);
        h2.set_cksum();
        tar.append_data(&mut h2, "evil.sh", &b"boom"[..]).unwrap();
        tar.finish().unwrap();
    }
    let err = extract_linux_tarball(&extra_path, &dir.path().join("out3")).expect_err("unexpected");
    assert_eq!(err.code(), CODE_UPDATE_UNSAFE_ARCHIVE);
}

#[test]
fn concurrent_update_lock_rejects_second_holder() {
    let dir = tempdir().unwrap();
    let lock1 = UpdateLock::acquire(dir.path()).expect("first lock");
    let err = UpdateLock::acquire(dir.path()).expect_err("second lock must fail");
    assert_eq!(err.code(), CODE_UPDATE_IN_PROGRESS);
    drop(lock1);
    let _lock2 = UpdateLock::acquire(dir.path()).expect("lock released");
}

#[tokio::test]
async fn disk_space_preflight_blocks_download_when_low() {
    let dir = tempdir().unwrap();
    let asset = ReleaseAsset::new(
        "dblocal-v0.2.0-linux-x86_64.tar.gz",
        "http://127.0.0.1:9/asset",
        1024,
        UpdateAssetKind::TarGz,
    );
    let sums = ReleaseAsset::new(
        "checksums.txt",
        "http://127.0.0.1:9/checksums.txt",
        100,
        UpdateAssetKind::Checksums,
    );
    let update = AvailableUpdate::new(Version::parse("0.2.0").unwrap(), "v0.2.0", "", asset, sums);
    let opts =
        UpdateDownloadOptions::loopback_for_tests().with_free_space_probe(Arc::new(|_| Ok(100)));
    let cancel = CancellationToken::new();
    let verifier = Sha256ReleaseVerifier;
    let err = download_update_with_progress(&update, dir.path(), &opts, &verifier, &cancel, None)
        .await
        .expect_err("disk space low");
    assert_eq!(err.code(), CODE_UPDATE_DISK_SPACE);
}

#[tokio::test]
async fn verifier_seam_can_reject_manifest_signature() {
    struct RejectingVerifier;
    impl ReleaseVerifier for RejectingVerifier {
        fn verify_manifest(
            &self,
            _checksums_bytes: &[u8],
            _signature_bytes: Option<&[u8]>,
        ) -> UpdateResult<Checksums> {
            Err(UpdateError::VerifyFailed {
                reason: "missing ed25519 signature".to_owned(),
            })
        }
        fn verify_artifact<'a>(
            &'a self,
            path: &'a Path,
            expected_hex: &'a str,
            label: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = UpdateResult<()>> + Send + 'a>>
        {
            Box::pin(verify_sha256_file(path, expected_hex, label))
        }
    }

    let sums_bytes =
        format!("{}  dblocal-v0.2.0-linux-x86_64.tar.gz\n", "a".repeat(64)).into_bytes();
    let hits = Arc::new(AtomicUsize::new(0));
    let mut routes = HashMap::new();
    routes.insert("/checksums.txt".to_owned(), (200, sums_bytes.clone()));
    let (base, server) = spawn_fake_server(routes, hits);

    let asset = ReleaseAsset::new(
        "dblocal-v0.2.0-linux-x86_64.tar.gz",
        format!("{base}/asset"),
        100,
        UpdateAssetKind::TarGz,
    );
    let sums = ReleaseAsset::new(
        "checksums.txt",
        format!("{base}/checksums.txt"),
        sums_bytes.len() as u64,
        UpdateAssetKind::Checksums,
    );
    let update = AvailableUpdate::new(Version::parse("0.2.0").unwrap(), "v0.2.0", "", asset, sums);
    let dir = tempdir().unwrap();
    let err = download_update_with_progress(
        &update,
        dir.path(),
        &UpdateDownloadOptions::loopback_for_tests(),
        &RejectingVerifier,
        &CancellationToken::new(),
        None,
    )
    .await
    .expect_err("signature rejected");
    assert_eq!(err.code(), CODE_UPDATE_VERIFY_FAILED);
    server.abort();
}

struct SyncServer {
    base_url: String,
    _rt: tokio::runtime::Runtime,
    handle: tokio::task::JoinHandle<()>,
}

impl SyncServer {
    fn start(routes: HashMap<String, (u16, Vec<u8>)>, hits: Arc<AtomicUsize>) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (base_url, handle) = rt.block_on(async { spawn_fake_server(routes, hits) });
        Self {
            base_url,
            _rt: rt,
            handle,
        }
    }
}

impl Drop for SyncServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[cfg(unix)]
fn sign_ed25519_for_test(payload: &[u8]) -> (Arc<dyn ReleaseVerifier>, Vec<u8>) {
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    let rng = SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
    let sig = pair.sign(payload).as_ref().to_vec();
    let mut pub_key = [0u8; ED25519_PUBLIC_KEY_LEN];
    pub_key.copy_from_slice(pair.public_key().as_ref());
    let verifier: Arc<dyn ReleaseVerifier> =
        Arc::new(Ed25519ReleaseVerifier::from_raw_keys(vec![pub_key]));
    (verifier, sig)
}

#[cfg(unix)]
#[test]
fn end_to_end_service_tarball_update_with_progress_desktop_icons_and_cache() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("dblocal-home");
    let bin_dir = tmp.path().join("prefix/bin");
    let data_home = tmp.path().join("prefix/share");
    let exe_path = bin_dir.join("dblocal");
    write_fake_exe(&exe_path, "0.1.3", true);

    let new_bin = fake_binary_bytes("0.2.0", true);
    let tarball = build_linux_tarball(&new_bin, true);
    let tarball_hash = sha256_hex(&tarball);
    let asset_name = "dblocal-v0.2.0-linux-x86_64.tar.gz";
    let checksums = format!("{tarball_hash}  {asset_name}\n").into_bytes();
    let (verifier, sig) = sign_ed25519_for_test(&checksums);

    let hits = Arc::new(AtomicUsize::new(0));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let (base, server) = rt.block_on(async {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let addr = std_listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");
        let releases = serde_json::json!([{
            "tag_name": "v0.2.0",
            "draft": false,
            "prerelease": false,
            "html_url": "https://github.com/rezkycodes/dblocal/releases/tag/v0.2.0",
            "body": "Release 0.2.0",
            "assets": [
                {"name": asset_name, "browser_download_url": format!("{base_url}/{asset_name}"), "size": tarball.len()},
                {"name": "checksums.txt", "browser_download_url": format!("{base_url}/checksums.txt"), "size": checksums.len()},
                {"name": "checksums.txt.sig", "browser_download_url": format!("{base_url}/checksums.txt.sig"), "size": sig.len()}
            ]
        }])
        .to_string()
        .into_bytes();

        let mut routes = HashMap::new();
        routes.insert(
            "/repos/rezkycodes/dblocal/releases?per_page=20".to_owned(),
            (200, releases),
        );
        routes.insert(format!("/{asset_name}"), (200, tarball));
        routes.insert("/checksums.txt".to_owned(), (200, checksums));
        routes.insert("/checksums.txt.sig".to_owned(), (200, sig));

        let listener = TcpListener::from_std(std_listener).unwrap();
        let routes = Arc::new(routes);
        let hits_clone = Arc::clone(&hits);
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let routes = Arc::clone(&routes);
                let hits = Arc::clone(&hits_clone);
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    hits.fetch_add(1, Ordering::SeqCst);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let (status, body) = routes
                        .get(path)
                        .cloned()
                        .unwrap_or_else(|| (404, b"not found".to_vec()));
                    let head = format!(
                        "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        (base_url, handle)
    });

    let options = ServiceOptions {
        auto_start: Some(false),
        os: Some(Os::Linux),
        arch: Some(Arch::X86_64),
        current_version: Some(Version::parse("0.1.3").unwrap()),
        install_kind: Some(InstallKind::PortableTarball {
            exe_path: exe_path.clone(),
        }),
        update_fetcher: Some(Arc::new(
            ReqwestUpdateFetcher::loopback_for_tests().unwrap(),
        )),
        update_api_base_url: Some(base),
        update_download_options: Some(UpdateDownloadOptions::loopback_for_tests()),
        update_data_home_override: Some(data_home.clone()),
        update_verifier: Some(verifier),
        ..ServiceOptions::default()
    };
    let svc = LocalService::new(DblPaths::with_root(&home), options).expect("service");

    // Auto-check defaults to enabled; first check_update_if_due hits server once.
    assert!(svc.auto_update_check_enabled().unwrap());
    let first = svc.check_update_if_due().unwrap().expect("cached status");
    assert_eq!(first.available.as_ref().unwrap().version, "0.2.0");
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // Second check_update_if_due within 6h uses cache (0 extra HTTP hits!).
    let second = svc.check_update_if_due().unwrap().expect("from cache");
    assert_eq!(second.available.as_ref().unwrap().version, "0.2.0");
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // Turning auto-check off makes check_update_if_due return Ok(None).
    svc.set_auto_update_check(false).unwrap();
    assert!(!svc.auto_update_check_enabled().unwrap());
    assert!(svc.check_update_if_due().unwrap().is_none());
    svc.set_auto_update_check(true).unwrap();

    // Start async update and collect progress events.
    let mut rx = svc.subscribe();
    let op_id = svc.start_update().expect("start_update");

    let mut saw_download = false;
    let mut saw_verify = false;
    let mut saw_install = false;
    let mut saw_ready = false;

    loop {
        match rx.blocking_recv().expect("event") {
            ServiceEvent::UpdateProgress { id, progress } if id == op_id => match progress {
                UpdateProgress::Downloading { .. } => saw_download = true,
                UpdateProgress::Verifying => saw_verify = true,
                UpdateProgress::Installing => saw_install = true,
                UpdateProgress::ReadyToRestart { version, .. } => {
                    assert_eq!(version, "0.2.0");
                    saw_ready = true;
                }
                other => panic!("unexpected progress: {other:?}"),
            },
            ServiceEvent::OperationFinished { id, result } if id == op_id => {
                result.expect("update succeeded");
                break;
            }
            _ => {}
        }
    }
    assert!(saw_download && saw_verify && saw_install && saw_ready);

    // Verify binary was atomically replaced and backup exists.
    let out = std::process::Command::new(&exe_path)
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("0.2.0"),
        "expected 0.2.0"
    );
    assert!(bin_dir.join("dblocal.old").is_file());

    // Verify desktop file was rewritten with absolute binary path and icons were installed.
    let desktop =
        std::fs::read_to_string(data_home.join("applications/dev.dblocal.DBLocal.desktop"))
            .unwrap();
    let canonical_exe = std::fs::canonicalize(&exe_path).unwrap();
    assert!(desktop.contains(&format!("Exec={}", canonical_exe.display())));
    assert!(desktop.contains(&format!("TryExec={}", canonical_exe.display())));
    assert!(
        data_home
            .join("icons/hicolor/scalable/apps/dblocal.svg")
            .is_file()
    );
    assert!(
        data_home
            .join("icons/hicolor/64x64/apps/dblocal.png")
            .is_file()
    );

    server.abort();
}

#[cfg(unix)]
#[test]
fn end_to_end_cancellation_during_download_cleans_up() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("dblocal-home");
    let exe_path = tmp.path().join("bin/dblocal");
    write_fake_exe(&exe_path, "0.1.3", true);

    let asset_name = "dblocal-v0.2.0-linux-x86_64.tar.gz";
    let checksums = format!("{}  {asset_name}\n", "a".repeat(64)).into_bytes();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let (base, server) = rt.block_on(async {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let addr = std_listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        let releases = serde_json::json!([{
            "tag_name": "v0.2.0",
            "draft": false,
            "prerelease": false,
            "html_url": "https://github.com/rezkycodes/dblocal/releases/tag/v0.2.0",
            "body": "notes",
            "assets": [
                {"name": asset_name, "browser_download_url": format!("{base}/{asset_name}"), "size": 10000},
                {"name": "checksums.txt", "browser_download_url": format!("{base}/checksums.txt"), "size": checksums.len()}
            ]
        }])
        .to_string()
        .into_bytes();

        let listener = TcpListener::from_std(std_listener).unwrap();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let releases = releases.clone();
                let checksums = checksums.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");
                    if path.contains("releases") {
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            releases.len()
                        );
                        let _ = socket.write_all(head.as_bytes()).await;
                        let _ = socket.write_all(&releases).await;
                    } else if path == "/checksums.txt" {
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            checksums.len()
                        );
                        let _ = socket.write_all(head.as_bytes()).await;
                        let _ = socket.write_all(&checksums).await;
                    } else {
                        // Drip-feed slowly so the client can cancel mid-download.
                        let head =
                            "HTTP/1.1 200 OK\r\nContent-Length: 10000\r\nConnection: close\r\n\r\n";
                        let _ = socket.write_all(head.as_bytes()).await;
                        let _ = socket.write_all(b"partial").await;
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    }
                });
            }
        });
        (base, server)
    });

    let options = ServiceOptions {
        auto_start: Some(false),
        os: Some(Os::Linux),
        arch: Some(Arch::X86_64),
        current_version: Some(Version::parse("0.1.3").unwrap()),
        install_kind: Some(InstallKind::PortableTarball {
            exe_path: exe_path.clone(),
        }),
        update_fetcher: Some(Arc::new(
            ReqwestUpdateFetcher::loopback_for_tests().unwrap(),
        )),
        update_api_base_url: Some(base),
        update_download_options: Some(UpdateDownloadOptions::loopback_for_tests()),
        update_verifier: Some(Arc::new(Sha256ReleaseVerifier)),
        ..ServiceOptions::default()
    };
    let svc = LocalService::new(DblPaths::with_root(&home), options).expect("service");
    let mut rx = svc.subscribe();
    let op_id = svc.start_update().expect("start");

    let mut cancelled = false;
    loop {
        match rx.blocking_recv().expect("event") {
            ServiceEvent::UpdateProgress { id, progress }
                if id == op_id
                    && matches!(progress, UpdateProgress::Downloading { .. })
                    && !cancelled =>
            {
                svc.cancel_operation(&op_id).expect("cancel");
                cancelled = true;
            }
            ServiceEvent::OperationFinished { id, result } if id == op_id => {
                let err = result.expect_err("must be cancelled");
                assert_eq!(err.code, CODE_UPDATE_CANCELLED);
                break;
            }
            _ => {}
        }
    }

    // Staging dir must have no leftover `run-*` partial directories.
    let staging = home.join(crate::consts::DIR_UPDATE_STAGING);
    if staging.exists() {
        let leftovers: Vec<_> = std::fs::read_dir(&staging)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("run-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "partial download left behind: {leftovers:?}"
        );
    }

    server.abort();
}

#[test]
fn background_check_is_quiet_when_offline_or_rate_limited() {
    let tmp = tempdir().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let mut routes = HashMap::new();
    routes.insert(
        "/repos/rezkycodes/dblocal/releases?per_page=20".to_owned(),
        (429, b"API rate limit exceeded".to_vec()),
    );
    let server = SyncServer::start(routes, hits);

    let options = ServiceOptions {
        auto_start: Some(false),
        os: Some(Os::Linux),
        arch: Some(Arch::X86_64),
        current_version: Some(Version::parse("0.1.3").unwrap()),
        install_kind: Some(InstallKind::PortableTarball {
            exe_path: tmp.path().join("dblocal"),
        }),
        update_fetcher: Some(Arc::new(
            ReqwestUpdateFetcher::loopback_for_tests().unwrap(),
        )),
        update_api_base_url: Some(server.base_url.clone()),
        ..ServiceOptions::default()
    };
    let svc = LocalService::new(DblPaths::with_root(tmp.path().join("home")), options).unwrap();
    // Rate limited -> quiet Ok(None) for background check!
    assert!(svc.check_update_if_due().unwrap().is_none());
    // Manual check still surfaces DBL-8003!
    let err = svc
        .check_update()
        .expect_err("manual check reports rate limit");
    assert_eq!(err.code, CODE_UPDATE_RATE_LIMITED);
}

#[cfg(unix)]
#[test]
fn end_to_end_appimage_atomic_replace_and_rollback_on_bad_binary() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    let appimage = tmp.path().join("DBLocal.AppImage");
    write_fake_exe(&appimage, "0.1.3", true);

    // 1. Good AppImage update replaces atomically and keeps .old
    let good_bytes = fake_binary_bytes("0.2.0", true);
    let good_hash = sha256_hex(&good_bytes);
    let asset_name = "dblocal-v0.2.0-linux-x86_64.AppImage";
    let checksums = format!("{good_hash}  {asset_name}\n").into_bytes();
    let (verifier, sig) = sign_ed25519_for_test(&checksums);

    let hits = Arc::new(AtomicUsize::new(0));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let (base, server) = rt.block_on(async {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let addr = std_listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        let releases = serde_json::json!([{
            "tag_name": "v0.2.0",
            "draft": false,
            "prerelease": false,
            "html_url": "https://github.com/rezkycodes/dblocal/releases/tag/v0.2.0",
            "body": "AppImage 0.2.0",
            "assets": [
                {"name": asset_name, "browser_download_url": format!("{base}/{asset_name}"), "size": good_bytes.len()},
                {"name": "checksums.txt", "browser_download_url": format!("{base}/checksums.txt"), "size": checksums.len()},
                {"name": "checksums.txt.sig", "browser_download_url": format!("{base}/checksums.txt.sig"), "size": sig.len()}
            ]
        }])
        .to_string()
        .into_bytes();

        let mut routes = HashMap::new();
        routes.insert(
            "/repos/rezkycodes/dblocal/releases?per_page=20".to_owned(),
            (200, releases),
        );
        routes.insert(format!("/{asset_name}"), (200, good_bytes));
        routes.insert("/checksums.txt".to_owned(), (200, checksums));
        routes.insert("/checksums.txt.sig".to_owned(), (200, sig));
        let listener = TcpListener::from_std(std_listener).unwrap();
        let routes = Arc::new(routes);
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let routes = Arc::clone(&routes);
                let hits = Arc::clone(&hits);
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    hits.fetch_add(1, Ordering::SeqCst);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let (status, body) = routes
                        .get(path)
                        .cloned()
                        .unwrap_or_else(|| (404, b"not found".to_vec()));
                    let head = format!(
                        "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        (base, server)
    });

    let options = ServiceOptions {
        auto_start: Some(false),
        os: Some(Os::Linux),
        arch: Some(Arch::X86_64),
        current_version: Some(Version::parse("0.1.3").unwrap()),
        install_kind: Some(InstallKind::AppImage {
            appimage_path: appimage.clone(),
        }),
        update_fetcher: Some(Arc::new(
            ReqwestUpdateFetcher::loopback_for_tests().unwrap(),
        )),
        update_api_base_url: Some(base),
        update_download_options: Some(UpdateDownloadOptions::loopback_for_tests()),
        update_verifier: Some(verifier),
        ..ServiceOptions::default()
    };
    let svc = LocalService::new(DblPaths::with_root(&home), options).unwrap();
    svc.apply_update().expect("appimage update succeeded");

    let out = std::process::Command::new(&appimage)
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("0.2.0"));
    assert!(tmp.path().join("DBLocal.AppImage.old").is_file());

    // 2. Bad tarball binary fails pre-swap smoke check and leaves target untouched!
    let bad_tarball_path = tmp.path().join("bad.tar.gz");
    let bad_bin = fake_binary_bytes("0.3.0", false);
    std::fs::write(&bad_tarball_path, build_linux_tarball(&bad_bin, false)).unwrap();
    let staged_bad = StagedUpdate::new_for_test(
        Version::parse("0.3.0").unwrap(),
        bad_tarball_path,
        tmp.path().to_path_buf(),
        "dblocal-v0.3.0-linux-x86_64.tar.gz".to_owned(),
        UpdateAssetKind::TarGz,
    );
    let err = apply_staged_update(
        &InstallKind::PortableTarball {
            exe_path: appimage.clone(),
        },
        &staged_bad,
        &appimage,
        None,
    )
    .expect_err("bad candidate binary must fail before swap");
    assert_eq!(err.code(), CODE_UPDATE_APPLY_FAILED);
    let still = std::process::Command::new(&appimage)
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&still.stdout).contains("0.2.0"));

    server.abort();
}

#[test]
fn windows_zip_extraction_accepts_valid_and_rejects_unexpected_or_traversal() {
    use std::io::Write as _;
    let dir = tempdir().unwrap();

    // Valid zip containing only dblocal.exe
    let good_zip = dir.path().join("good.zip");
    {
        let f = std::fs::File::create(&good_zip).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        zw.start_file("dblocal.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b"MZ-fake-exe").unwrap();
        zw.finish().unwrap();
    }
    let extracted = extract_windows_zip(&good_zip, &dir.path().join("out_good")).unwrap();
    assert_eq!(std::fs::read(&extracted).unwrap(), b"MZ-fake-exe");

    // Malicious zip with unexpected file
    let extra_zip = dir.path().join("extra.zip");
    {
        let f = std::fs::File::create(&extra_zip).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        zw.start_file("dblocal.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b"MZ").unwrap();
        zw.start_file("evil.dll", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b"bad").unwrap();
        zw.finish().unwrap();
    }
    let err = extract_windows_zip(&extra_zip, &dir.path().join("out_extra"))
        .expect_err("extra dll rejected");
    assert_eq!(err.code(), CODE_UPDATE_UNSAFE_ARCHIVE);
}

#[test]
fn advisory_lock_ignores_empty_or_garbled_lockfile_and_releases_on_drop() {
    let dir = tempdir().unwrap();
    let lock_path = dir.path().join(crate::consts::UPDATE_LOCK_FILE);

    // Pre-create an empty lock file and a garbled lock file; OS advisory lock
    // must still acquire cleanly instead of deadlocking forever.
    std::fs::write(&lock_path, b"").unwrap();
    let guard = UpdateLock::acquire(dir.path()).expect("empty lock file acquired");
    let err = UpdateLock::acquire(dir.path()).expect_err("second acquire blocked");
    assert_eq!(err.code(), CODE_UPDATE_IN_PROGRESS);
    drop(guard);

    std::fs::write(&lock_path, b"not-a-pid-garbled!!!\n").unwrap();
    let _guard2 = UpdateLock::acquire(dir.path()).expect("garbled lock file acquired");
}

#[test]
fn windows_msi_helper_runs_from_staging_copy_not_install_target_and_cleans_up_later() {
    let tmp = tempdir().unwrap();
    let install_dir = tmp.path().join("AppData").join("Local").join("DBLocal");
    std::fs::create_dir_all(&install_dir).unwrap();
    let target_exe = install_dir.join("dblocal.exe");
    std::fs::write(&target_exe, b"MZ-installed-exe").unwrap();

    let staging_root = tmp.path().join("update");
    let run_dir = staging_root.join("run-123-abcdef");
    std::fs::create_dir_all(&run_dir).unwrap();
    let msi_file = run_dir.join("dblocal-update.bin");
    std::fs::write(&msi_file, b"MSI-BYTES").unwrap();

    let staged = StagedUpdate::new_for_test_any(
        Version::parse("0.2.0").unwrap(),
        msi_file.clone(),
        run_dir.clone(),
        "dblocal-0.2.0-windows-x86_64.msi".to_owned(),
        UpdateAssetKind::Msi,
    );
    let action = apply_staged_update(
        &InstallKind::WindowsMsi {
            exe_path: target_exe.clone(),
        },
        &staged,
        &target_exe,
        None,
    )
    .expect("msi apply stages helper");

    match action {
        RestartAction::RunWindowsMsi {
            helper_exe,
            msi_path,
            relaunch_exe,
        } => {
            assert_ne!(
                helper_exe, target_exe,
                "helper must never run from the installed target"
            );
            assert!(
                helper_exe.starts_with(&run_dir),
                "helper must live inside private staging dir: {}",
                helper_exe.display()
            );
            assert!(helper_exe.is_file());
            assert_eq!(msi_path, msi_file);
            assert_eq!(relaunch_exe, target_exe);
        }
        other => panic!("expected RunWindowsMsi, got {other:?}"),
    }

    // Pending staging dir is protected from startup cleanup until completed or aged.
    mark_staging_pending(&run_dir);
    cleanup_stale_staging(&staging_root);
    assert!(
        run_dir.exists(),
        "pending run-* dir must not be deleted by startup cleanup"
    );

    // Once marked completed (after helper finishes), startup cleanup removes it immediately.
    mark_staging_completed(&run_dir);
    cleanup_stale_staging(&staging_root);
    assert!(
        !run_dir.exists(),
        "completed run-* dir must be cleaned up on next startup"
    );
}

#[test]
fn windows_helper_handles_timeout_3010_and_failure_relaunch_with_persisted_error() {
    use std::sync::atomic::AtomicBool;
    let tmp = tempdir().unwrap();
    let target = tmp.path().join("dblocal.exe");
    std::fs::write(&target, b"MZ-old").unwrap();
    let msi = tmp.path().join("update.msi");
    std::fs::write(&msi, b"MSI").unwrap();
    let err_file = tmp.path().join("last-error.txt");

    // 1. Parent timeout -> aborts without running msiexec, persists error.
    let msi_called = AtomicBool::new(false);
    let relaunched = AtomicBool::new(false);
    let args = WindowsHelperArgs {
        pid: 99999,
        msi: Some(msi.clone()),
        staged: None,
        target: target.clone(),
        version: None,
        error_file: Some(err_file.clone()),
    };
    let err = super::install::run_windows_apply_helper_with(
        &args,
        |_| false,
        |_| {
            msi_called.store(true, Ordering::SeqCst);
            Ok(Some(0))
        },
        |_| {
            relaunched.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .expect_err("timeout must abort");
    assert_eq!(err.code(), CODE_UPDATE_APPLY_FAILED);
    assert!(!msi_called.load(Ordering::SeqCst), "must not run msiexec");
    assert!(!relaunched.load(Ordering::SeqCst));
    let persisted = take_last_update_failure(tmp.path()).expect("error persisted");
    assert!(persisted.contains("did not exit"), "{persisted}");

    // 2. Exit 3010 (reboot required) -> treated as success, relaunches app, no error persisted.
    let relaunched = AtomicBool::new(false);
    super::install::run_windows_apply_helper_with(
        &args,
        |_| true,
        |_| Ok(Some(MSI_EXIT_REBOOT_REQUIRED)),
        |_| {
            relaunched.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .expect("3010 is success");
    assert!(relaunched.load(Ordering::SeqCst));
    assert!(take_last_update_failure(tmp.path()).is_none());

    // 3. Exit 1603 (MSI failure) -> persists error AND relaunches previous app!
    let relaunched = AtomicBool::new(false);
    let err = super::install::run_windows_apply_helper_with(
        &args,
        |_| true,
        |_| Ok(Some(1603)),
        |_| {
            relaunched.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .expect_err("1603 is failure");
    assert_eq!(err.code(), CODE_UPDATE_APPLY_FAILED);
    assert!(
        relaunched.load(Ordering::SeqCst),
        "must relaunch previous app on failure"
    );
    let persisted = take_last_update_failure(tmp.path()).expect("failure persisted");
    assert!(persisted.contains("1603"), "{persisted}");
}

#[cfg(unix)]
#[test]
fn linux_tarball_desktop_sync_is_non_fatal_and_respects_ownership() {
    let tmp = tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    let exe_path = bin_dir.join("dblocal");
    write_fake_exe(&exe_path, "0.1.3", true);

    let tarball_path = tmp.path().join("update.tar.gz");
    std::fs::write(
        &tarball_path,
        build_linux_tarball(&fake_binary_bytes("0.2.0", true), true),
    )
    .unwrap();
    let staged = StagedUpdate::new_for_test_any(
        Version::parse("0.2.0").unwrap(),
        tarball_path,
        tmp.path().to_path_buf(),
        "dblocal-v0.2.0-linux-x86_64.tar.gz".to_owned(),
        UpdateAssetKind::TarGz,
    );

    // Case A: existing desktop entry points at a DIFFERENT binary -> must not be hijacked.
    let data_home_other = tmp.path().join("share-other");
    let apps_other = data_home_other.join("applications");
    std::fs::create_dir_all(&apps_other).unwrap();
    let existing_desktop = apps_other.join("dev.dblocal.DBLocal.desktop");
    let original_desktop = "[Desktop Entry]\nName=DBLocal\nExec=/some/other/dblocal\n";
    std::fs::write(&existing_desktop, original_desktop).unwrap();

    apply_staged_update(
        &InstallKind::PortableTarball {
            exe_path: exe_path.clone(),
        },
        &staged,
        &exe_path,
        Some(&data_home_other),
    )
    .expect("update succeeds");
    assert_eq!(
        std::fs::read_to_string(&existing_desktop).unwrap(),
        original_desktop,
        "must not hijack desktop entry owned by another copy"
    );

    // Case B: unwritable/broken data_home -> desktop sync failure is NON-FATAL after binary swap.
    let broken_data_home = tmp.path().join("share-broken");
    std::fs::create_dir_all(&broken_data_home).unwrap();
    // Place a regular file where `applications/` directory should be so mkdir fails.
    std::fs::write(broken_data_home.join("applications"), b"not-a-dir").unwrap();

    write_fake_exe(&exe_path, "0.1.3", true);
    apply_staged_update(
        &InstallKind::PortableTarball {
            exe_path: exe_path.clone(),
        },
        &staged,
        &exe_path,
        Some(&broken_data_home),
    )
    .expect("desktop sync failure must be non-fatal once binary is swapped");
    let out = std::process::Command::new(&exe_path)
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("0.2.0"));
}

#[cfg(unix)]
#[test]
fn ed25519_verifier_verifies_exact_openssl_release_yml_signature_and_fails_closed() {
    let tmp = tempdir().unwrap();
    let priv_pem = tmp.path().join("key.pem");
    let pub_der = tmp.path().join("pub.der");
    let sums_file = tmp.path().join("checksums.txt");
    let sig_file = tmp.path().join("checksums.txt.sig");

    let checksums_body = format!("{}  dblocal-v0.2.0-linux-x86_64.tar.gz\n", "a".repeat(64));
    std::fs::write(&sums_file, &checksums_body).unwrap();

    // Exact OpenSSL commands used by generate-key.sh and release.yml:
    let st = std::process::Command::new("openssl")
        .args(["genpkey", "-algorithm", "ED25519", "-out"])
        .arg(&priv_pem)
        .status()
        .expect("openssl genpkey");
    assert!(st.success());

    let st = std::process::Command::new("openssl")
        .args(["pkey", "-in"])
        .arg(&priv_pem)
        .args(["-pubout", "-outform", "DER", "-out"])
        .arg(&pub_der)
        .status()
        .expect("openssl pkey pubout");
    assert!(st.success());

    let st = std::process::Command::new("openssl")
        .args(["pkeyutl", "-sign", "-inkey"])
        .arg(&priv_pem)
        .args(["-rawin", "-in"])
        .arg(&sums_file)
        .arg("-out")
        .arg(&sig_file)
        .status()
        .expect("openssl pkeyutl -sign");
    assert!(st.success());

    let der_bytes = std::fs::read(&pub_der).unwrap();
    assert!(der_bytes.len() >= ED25519_PUBLIC_KEY_LEN);
    let raw_pub = &der_bytes[der_bytes.len() - ED25519_PUBLIC_KEY_LEN..];
    let mut pub_hex = String::with_capacity(64);
    for b in raw_pub {
        use std::fmt::Write as _;
        let _ = write!(pub_hex, "{b:02x}");
    }
    let sig_bytes = std::fs::read(&sig_file).unwrap();
    assert_eq!(sig_bytes.len(), ED25519_SIGNATURE_LEN);

    // 1. Valid OpenSSL signature verifies in Rust (including alongside a second rotated key).
    let dummy_old_key = "0".repeat(64);
    let verifier =
        Ed25519ReleaseVerifier::from_hex_keys(&[&dummy_old_key, &pub_hex]).expect("hex keys");
    let parsed = verifier
        .verify_manifest(checksums_body.as_bytes(), Some(&sig_bytes))
        .expect("OpenSSL Ed25519 signature must verify");
    assert_eq!(parsed.len(), 1);

    // 2. Fail closed: missing signature.
    let err = verifier
        .verify_manifest(checksums_body.as_bytes(), None)
        .expect_err("missing sig");
    assert_eq!(err.code(), CODE_UPDATE_VERIFY_FAILED);

    // 3. Fail closed: tampered checksums.txt.
    let mut tampered_body = checksums_body.into_bytes();
    tampered_body[0] ^= 0x01;
    let err = verifier
        .verify_manifest(&tampered_body, Some(&sig_bytes))
        .expect_err("tampered body");
    assert_eq!(err.code(), CODE_UPDATE_VERIFY_FAILED);

    // 4. Fail closed: tampered signature.
    let mut tampered_sig = sig_bytes.clone();
    tampered_sig[0] ^= 0xff;
    let err = verifier
        .verify_manifest(
            format!("{}  dblocal-v0.2.0-linux-x86_64.tar.gz\n", "a".repeat(64)).as_bytes(),
            Some(&tampered_sig),
        )
        .expect_err("tampered sig");
    assert_eq!(err.code(), CODE_UPDATE_VERIFY_FAILED);

    // 5. Fail closed: truncated/malformed signature.
    let err = verifier
        .verify_manifest(b"anything", Some(&sig_bytes[..32]))
        .expect_err("short sig");
    assert_eq!(err.code(), CODE_UPDATE_VERIFY_FAILED);

    // 6. Fail closed: empty production public-keys.txt refuses everything.
    let empty_prod = Ed25519ReleaseVerifier::from_raw_keys(Vec::new());
    let err = empty_prod
        .verify_manifest(b"anything", Some(&sig_bytes))
        .expect_err("empty keys must fail closed");
    assert_eq!(err.code(), CODE_UPDATE_VERIFY_FAILED);
}

#[cfg(unix)]
#[test]
fn windows_zip_swap_rollback_restores_original_bytes_on_smoke_failure() {
    use std::sync::atomic::AtomicBool;
    let tmp = tempdir().unwrap();
    let target = tmp.path().join("dblocal.exe");
    let staged = tmp.path().join("run-42-abcd/extracted/dblocal.exe");
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();

    let original_bytes = b"#!/bin/sh\n# original-0.1.3-marker\necho 'dblocal 0.1.3'\n";
    let broken_bytes = b"#!/bin/sh\n# broken-0.9.9-marker\necho 'dblocal 0.9.9'\n";
    std::fs::write(&target, original_bytes).unwrap();
    std::fs::write(&staged, broken_bytes).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let err_file = tmp.path().join(LAST_UPDATE_ERROR_FILE);
    let relaunched = AtomicBool::new(false);
    let args = WindowsHelperArgs {
        pid: 0,
        msi: None,
        staged: Some(staged.clone()),
        target: target.clone(),
        version: Some("0.2.0".to_owned()),
        error_file: Some(err_file.clone()),
    };

    let err = super::install::run_windows_apply_helper_with(
        &args,
        |_| true,
        |_| unreachable!("zip swap must not invoke msiexec"),
        |exe| {
            assert_eq!(exe, target);
            relaunched.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .expect_err("wrong version binary must trigger rollback");
    assert_eq!(err.code(), CODE_UPDATE_APPLY_FAILED);
    assert!(
        relaunched.load(Ordering::SeqCst),
        "must relaunch restored original binary"
    );

    // Crucial regression check: the hard-linked backup was NOT overwritten by the swap,
    // so rollback restored the exact original 0.1.3 bytes!
    let restored = std::fs::read(&target).unwrap();
    assert_eq!(
        restored, original_bytes,
        "rollback must restore exact original bytes, not the broken staged binary"
    );
    let backup = target.with_file_name("dblocal.exe.old");
    assert_eq!(
        std::fs::read(&backup).unwrap(),
        original_bytes,
        "backup must still hold the untouched original bytes"
    );
    let persisted = take_last_update_failure(tmp.path()).expect("failure persisted");
    assert!(persisted.contains("smoke check"), "{persisted}");
}

#[test]
fn public_keys_file_single_source_rejects_malformed_lines() {
    // 1. Committed packaging/release-signing/public-keys.txt must parse cleanly.
    let committed = parse_trusted_public_keys(TRUSTED_PUBLIC_KEYS_RAW)
        .expect("committed packaging/release-signing/public-keys.txt must have no malformed lines");
    assert_eq!(
        committed.len(),
        Ed25519ReleaseVerifier::production().key_count()
    );

    // 2. Comments, blank lines, and multiple 64-char lowercase hex keys parse cleanly.
    let key1 = "a".repeat(64);
    let key2 = "0123456789abcdef".repeat(4);
    let valid_doc = format!("# comment\n\n  {key1}  \n# another\n{key2}\n");
    let parsed = parse_trusted_public_keys(&valid_doc).expect("valid keys");
    assert_eq!(parsed.len(), 2);

    // 3. Any malformed line must fail (never silently skipped).
    for bad in [
        format!("{key1}\nnot-a-hex-key\n"),
        format!("{}\n", "A".repeat(64)), // uppercase rejected to match release.yml
        format!("{}\n", "a".repeat(63)), // too short
        format!("{key1} extra\n"),       // trailing junk
    ] {
        let err = parse_trusted_public_keys(&bad).expect_err("malformed line must fail");
        assert_eq!(err.code(), CODE_UPDATE_VERIFY_FAILED);
    }
}

#[test]
fn startup_cleanup_protects_pending_and_locked_staging_and_reclaims_completed_or_aged() {
    use std::sync::atomic::AtomicBool;
    let tmp = tempdir().unwrap();
    let staging = tmp.path().join("update");
    std::fs::create_dir_all(&staging).unwrap();

    // 1. Fresh unmarked dir is protected (minimum age) so a race mid-download never deletes it.
    let fresh_unmarked = staging.join("run-100-fresh");
    std::fs::create_dir_all(&fresh_unmarked).unwrap();
    cleanup_stale_staging(&staging);
    assert!(
        fresh_unmarked.exists(),
        "fresh unmarked dir must be protected"
    );

    // 2. Pending dir is protected across repeated startup cleanups.
    let pending_dir = staging.join("run-200-pending");
    std::fs::create_dir_all(&pending_dir).unwrap();
    mark_staging_pending(&pending_dir);
    cleanup_stale_staging(&staging);
    assert!(pending_dir.exists(), "pending dir must be protected");

    // 3. While the detached helper is running, it holds UpdateLock and marks completed when done.
    let helper_run = staging.join("run-300-helper");
    std::fs::create_dir_all(&helper_run).unwrap();
    mark_staging_pending(&helper_run);
    let msi = helper_run.join("dblocal-update.bin");
    std::fs::write(&msi, b"MSI").unwrap();
    let target = tmp.path().join("dblocal.exe");
    std::fs::write(&target, b"MZ").unwrap();
    let lock_blocked_during_helper = AtomicBool::new(false);
    let staging_for_closure = staging.clone();
    let args = WindowsHelperArgs {
        pid: 0,
        msi: Some(msi),
        staged: None,
        target,
        version: None,
        error_file: Some(staging.join(LAST_UPDATE_ERROR_FILE)),
    };
    super::install::run_windows_apply_helper_with(
        &args,
        |_| true,
        |_| {
            // While helper runs, another process cannot acquire UpdateLock.
            let err = UpdateLock::acquire(&staging_for_closure).expect_err("helper must hold lock");
            if err.code() == CODE_UPDATE_IN_PROGRESS {
                lock_blocked_during_helper.store(true, Ordering::SeqCst);
            }
            Ok(Some(0))
        },
        |_| Ok(()),
    )
    .expect("helper succeeds");
    assert!(
        lock_blocked_during_helper.load(Ordering::SeqCst),
        "helper must hold UpdateLock during execution"
    );
    assert!(
        helper_run.join(STAGED_COMPLETED_MARKER).is_file(),
        "helper must mark run dir completed on exit"
    );
    assert!(
        !helper_run.join(STAGED_PENDING_MARKER).exists(),
        "pending marker must be cleared on completion"
    );

    // 4. Next startup cleanup removes completed dir immediately while keeping pending dir.
    cleanup_stale_staging(&staging);
    assert!(!helper_run.exists(), "completed run dir must be removed");
    assert!(pending_dir.exists(), "still-pending run dir must remain");

    // 5. Aged dirs (past threshold) are reclaimed.
    super::install::cleanup_stale_staging_with_ages(&staging, Duration::ZERO, Duration::ZERO);
    assert!(!fresh_unmarked.exists(), "aged unmarked dir reclaimed");
    assert!(!pending_dir.exists(), "aged pending dir reclaimed");
}

#[tokio::test]
async fn check_rejects_mismatched_or_versionless_asset_names_including_msi() {
    struct StaticFetcher(String);
    impl UpdateFetcher for StaticFetcher {
        fn get(
            &self,
            _url: String,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = std::result::Result<FetchResponse, String>>
                    + Send
                    + '_,
            >,
        > {
            let body = self.0.as_bytes().to_vec();
            Box::pin(async move { Ok(FetchResponse::new(200, body)) })
        }
    }

    let make_json = |tag: &str, asset_name: &str| {
        serde_json::json!([{
            "tag_name": tag,
            "draft": false,
            "prerelease": false,
            "body": "release",
            "assets": [
                {
                    "name": asset_name,
                    "browser_download_url": format!("https://github.com/a/b/releases/download/{tag}/{asset_name}"),
                    "size": 100
                },
                {
                    "name": "checksums.txt",
                    "browser_download_url": format!("https://github.com/a/b/releases/download/{tag}/checksums.txt"),
                    "size": 10
                }
            ]
        }])
        .to_string()
    };

    let current = Version::parse("0.1.3").unwrap();
    let win_msi_opts = UpdateCheckOptions::for_repo("rezkycodes/dblocal")
        .with_platform(Os::Windows, Arch::X86_64)
        .with_install_kind(InstallKind::WindowsMsi {
            exe_path: PathBuf::from(r"C:\Users\u\AppData\Local\DBLocal\dblocal.exe"),
        });

    // 1. Older MSI re-uploaded under newer tag v0.2.0 -> refused.
    let fetcher = StaticFetcher(make_json("v0.2.0", "dblocal-0.1.2-windows-x86_64.msi"));
    let res = check(&current, &fetcher, &win_msi_opts).await.unwrap();
    assert!(
        matches!(res, UpdateCheck::UpToDate { .. }),
        "older MSI under newer tag must be refused"
    );

    // 2. Versionless MSI under tag v0.2.0 -> refused.
    let fetcher = StaticFetcher(make_json("v0.2.0", "dblocal-windows-x86_64.msi"));
    let res = check(&current, &fetcher, &win_msi_opts).await.unwrap();
    assert!(
        matches!(res, UpdateCheck::UpToDate { .. }),
        "versionless MSI must be refused"
    );

    // 3. Prerelease tarball under stable tag v0.2.0 -> refused.
    let linux_opts = UpdateCheckOptions::for_repo("rezkycodes/dblocal")
        .with_platform(Os::Linux, Arch::X86_64)
        .with_install_kind(InstallKind::PortableTarball {
            exe_path: PathBuf::from("/home/u/.local/bin/dblocal"),
        });
    let fetcher = StaticFetcher(make_json(
        "v0.2.0",
        "dblocal-0.2.0-rc.1-linux-x86_64.tar.gz",
    ));
    let res = check(&current, &fetcher, &linux_opts).await.unwrap();
    assert!(
        matches!(res, UpdateCheck::UpToDate { .. }),
        "prerelease asset under stable tag must be refused"
    );

    // 4. Matching versioned MSI -> accepted.
    let fetcher = StaticFetcher(make_json("v0.2.0", "dblocal-0.2.0-windows-x86_64.msi"));
    let res = check(&current, &fetcher, &win_msi_opts).await.unwrap();
    match res {
        UpdateCheck::Available(u) => {
            assert_eq!(u.version().to_string(), "0.2.0");
            assert_eq!(u.asset().name(), "dblocal-0.2.0-windows-x86_64.msi");
        }
        other => panic!("expected Available for matching MSI, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn generate_key_script_backup_guards_and_fake_gh_upload() {
    use std::os::unix::fs::PermissionsExt;

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let script = repo_root.join("packaging/release-signing/generate-key.sh");
    assert!(script.is_file(), "missing {}", script.display());

    let tmp = tempdir().unwrap();
    let fake_bin = tmp.path().join("bin");
    std::fs::create_dir_all(&fake_bin).unwrap();
    let captured_key = tmp.path().join("captured-secret.pem");
    let captured_args = tmp.path().join("captured-args.txt");

    // Fake `gh` on PATH: records args and stdin without touching GitHub.
    let fake_gh = fake_bin.join("gh");
    std::fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$*\" > '{}'\ncat > '{}'\n",
            captured_args.display(),
            captured_key.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake_gh, std::fs::Permissions::from_mode(0o755)).unwrap();

    let path_env = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // 1. Fails when neither --backup nor --no-backup is passed.
    let out = std::process::Command::new("bash")
        .arg(&script)
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "must require --backup or --no-backup"
    );

    // 2. Refuses backup path inside the git repository.
    let in_repo_backup = repo_root.join("should-never-be-created.enc.pem");
    let out = std::process::Command::new("bash")
        .arg(&script)
        .args(["--backup", in_repo_backup.to_str().unwrap()])
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(!out.status.success(), "must refuse backup inside git repo");
    assert!(!in_repo_backup.exists());

    // 3. Refuses to overwrite an existing backup file.
    let existing_backup = tmp.path().join("existing.enc.pem");
    std::fs::write(&existing_backup, b"existing").unwrap();
    let out = std::process::Command::new("bash")
        .arg(&script)
        .args(["--backup", existing_backup.to_str().unwrap()])
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(!out.status.success(), "must refuse to overwrite backup");
    assert_eq!(std::fs::read(&existing_backup).unwrap(), b"existing");

    // 4. Succeeds with --no-backup using fake gh; never prints private key.
    let out = std::process::Command::new("bash")
        .arg(&script)
        .args(["--no-backup", "--repo", "example/dblocal"])
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("PRIVATE KEY"),
        "must never print private key"
    );
    assert!(
        stdout.contains("packaging/release-signing/public-keys.txt"),
        "must point to single source of truth"
    );
    assert!(
        !stdout.contains("TRUSTED_PUBLIC_KEYS"),
        "must not tell user to edit a second list"
    );
    let uploaded = std::fs::read_to_string(&captured_key).unwrap();
    assert!(uploaded.contains("BEGIN PRIVATE KEY"));
    let gh_args = std::fs::read_to_string(&captured_args).unwrap();
    assert!(gh_args.contains("secret set RELEASE_SIGNING_KEY --repo example/dblocal"));

    // 5. Succeeds with --backup <outside-temp-file> driven through a PTY so OpenSSL
    // prompts interactively (never via env var or CLI arg).
    let backup_file = tmp.path().join("offline-backup.enc.pem");
    let py_driver = r#"
import fcntl, os, pty, re, select, subprocess, sys, termios, time
script, backup_path = sys.argv[1], sys.argv[2]
master, slave = pty.openpty()
def set_ctty():
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
proc = subprocess.Popen(
    ["bash", script, "--backup", backup_path, "--repo", "example/dblocal"],
    stdin=slave, stdout=slave, stderr=slave, close_fds=True, preexec_fn=set_ctty,
)
os.close(slave)
buf = b""
prompts_answered = 0
prompt_re = re.compile(rb"(pass\s*phrase[^:\n]*|password)\s*:", re.IGNORECASE)
deadline = time.time() + 20
while time.time() < deadline:
    r, _, _ = select.select([master], [], [], 0.1)
    if r:
        try:
            chunk = os.read(master, 4096)
        except OSError:
            break
        if not chunk:
            break
        buf += chunk
        count = len(prompt_re.findall(buf))
        while prompts_answered < count:
            os.write(master, b"test-backup-passphrase-123\n")
            prompts_answered += 1
    if proc.poll() is not None:
        while True:
            r2, _, _ = select.select([master], [], [], 0.05)
            if not r2:
                break
            try:
                c = os.read(master, 4096)
                if not c:
                    break
                buf += c
            except OSError:
                break
        break
if proc.poll() is None:
    proc.kill()
os.close(master)
sys.stdout.buffer.write(buf)
sys.exit(proc.wait())
"#;
    let out = std::process::Command::new("python3")
        .args([
            "-c",
            py_driver,
            script.to_str().unwrap(),
            backup_file.to_str().unwrap(),
        ])
        .env("PATH", &path_env)
        .output()
        .unwrap();
    let combined = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "backup run failed: {combined}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !combined.contains("BEGIN PRIVATE KEY"),
        "must never print plaintext key"
    );
    assert!(backup_file.is_file(), "backup file must exist");
    let mode = std::fs::metadata(&backup_file)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "backup file must be mode 0600");
    let backup_text = std::fs::read_to_string(&backup_file).unwrap();
    assert!(
        backup_text.contains("BEGIN ENCRYPTED PRIVATE KEY"),
        "backup must be encrypted PKCS#8"
    );
    // A stolen backup must resist offline guessing: the PBKDF2 iteration count
    // lives in the clear in the PKCS#8 parameters, so it can be read instead of
    // trusted. OpenSSL's old default (2048) is far too low. `asn1parse` prints
    // the INTEGER in uppercase hex, so compare against that form.
    let asn1 = std::process::Command::new("openssl")
        .args(["asn1parse", "-in"])
        .arg(&backup_file)
        .output()
        .unwrap();
    assert!(asn1.status.success(), "backup must parse as PKCS#8");
    let asn1_text = String::from_utf8_lossy(&asn1.stdout);
    // ASN.1 prints the INTEGER signed, so a positive value whose top byte sets
    // the sign bit appears with one leading `0`: 600000 (0x0927C0) is `0927C0`.
    assert!(
        asn1_text.contains(":0927C0"),
        "backup must use a 600000-iteration PBKDF2: {asn1_text}"
    );

    // Confirm backup decrypts to the exact 64-char hex public key printed by the script.
    let printed_hex = combined
        .lines()
        .map(str::trim)
        .find(|line| {
            line.len() == 64 && line.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
        .expect("script must print 64-char lowercase hex public key");
    let dec_der = tmp.path().join("dec.pub.der");
    let st = std::process::Command::new("openssl")
        .args(["pkey", "-in"])
        .arg(&backup_file)
        .args([
            "-passin",
            "pass:test-backup-passphrase-123",
            "-pubout",
            "-outform",
            "DER",
            "-out",
        ])
        .arg(&dec_der)
        .status()
        .unwrap();
    assert!(st.success());
    let der = std::fs::read(&dec_der).unwrap();
    let raw = &der[der.len() - ED25519_PUBLIC_KEY_LEN..];
    let mut derived_hex = String::with_capacity(64);
    for b in raw {
        use std::fmt::Write as _;
        let _ = write!(derived_hex, "{b:02x}");
    }
    assert_eq!(derived_hex, printed_hex);
}
