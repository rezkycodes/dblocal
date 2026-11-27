//! Reusing an installed runtime for a second instance (`F-RTM-03` AC3).
//!
//! The RustFS archive is the one shipped artifact with no `bin/` directory: its
//! zip holds a single `rustfs` entry at the archive root and the catalog sets no
//! `strip_prefix`. This drives the real download -> verify -> extract pipeline
//! over loopback HTTP with exactly that layout, so the installed-runtime check
//! is exercised against the bytes upstream actually publishes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::download::UrlPolicy;
use crate::model::{
    ArchiveFormat, AuthMode, EngineKey, InstanceName, InstanceSpec, InstanceStatus, Port,
};
use crate::service::DblService;

use super::harness::{Harness, RuntimeSource};

/// The real RustFS archive layout: one regular file named `rustfs` at the root.
fn rustfs_zip() -> (Vec<u8>, String) {
    use zip::write::SimpleFileOptions;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().unix_permissions(0o755);
    writer.start_file("rustfs", options).expect("start entry");
    writer
        .write_all(b"\x7fELF fake rustfs build")
        .expect("write entry");
    let bytes = writer.finish().expect("finish zip").into_inner();
    let hex = sha2::Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    (bytes, hex)
}

/// A loopback server answering every `GET` with `body`, counting the requests.
async fn serve(body: Vec<u8>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let body = body.clone();
            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                let mut head = [0u8; 1024];
                let _ = socket.read(&mut head).await;
                counter.fetch_add(1, Ordering::SeqCst);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    (format!("http://{addr}/rustfs.zip"), hits)
}

fn rustfs_spec(name: &str, port: u16) -> InstanceSpec {
    InstanceSpec::new(
        InstanceName::new(name).expect("name"),
        EngineKey::Rustfs,
        "1.0.7",
        Some(Port::new(port).expect("port")),
        AuthMode::Password,
    )
    .expect("spec")
}

#[test]
fn a_second_rustfs_instance_reuses_the_installed_runtime() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt");
    let (body, sha) = rustfs_zip();
    let size = body.len() as u64;
    let (url, hits) = runtime.block_on(serve(body));

    let source = RuntimeSource {
        id: "rustfs-1.0.7-linux-x86_64".to_owned(),
        url,
        sha256: sha,
        size_bytes: size,
        format: ArchiveFormat::Zip,
        strip_prefix: None,
        engine: EngineKey::Rustfs,
        version: "1.0.7".to_owned(),
        expected_binaries: vec!["rustfs".to_owned()],
    };
    let h = Harness::with_options(EngineKey::Rustfs, |options| {
        options.runtime_sources = vec![source];
        options.download_policy = Some(UrlPolicy::allow_loopback_http_for_tests());
    });

    // First instance installs the runtime through the real pipeline.
    let first = h.create(rustfs_spec("s3-one", super::harness::test_port()));
    let first_stages = super::harness::stages(&first);
    assert!(
        first_stages.contains(&"downloading"),
        "the first instance must install: {first_stages:?}"
    );
    let dir = h
        .paths
        .runtime_dir(EngineKey::Rustfs, "1.0.7", crate::platform::Arch::X86_64)
        .expect("runtime dir");
    assert!(
        dir.join("rustfs").is_file(),
        "the real archive unpacks `rustfs` at the runtime root: {}",
        dir.display()
    );
    assert!(
        !dir.join("bin").exists(),
        "the RustFS archive has no `bin/` directory; a recognition check that \
         assumes one is what made a second create re-download and collide"
    );

    // Second instance must reuse it: no download, no runtime collision.
    let second = h.create(rustfs_spec("s3-two", super::harness::test_port()));
    let second_stages = super::harness::stages(&second);
    assert!(
        !second_stages.contains(&"downloading"),
        "an installed RustFS runtime must be recognised: {second_stages:?}"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the archive may be downloaded only once"
    );

    assert_eq!(h.instances().len(), 2, "both instances exist");
    assert_eq!(h.instance_status("s3-one"), Some(InstanceStatus::Stopped));
    assert_eq!(h.instance_status("s3-two"), Some(InstanceStatus::Stopped));

    let runtimes: Vec<_> = h
        .service
        .list_runtimes()
        .expect("runtimes")
        .into_iter()
        .filter(|entry| entry.engine() == EngineKey::Rustfs)
        .collect();
    assert_eq!(runtimes.len(), 1, "exactly one RustFS runtime is recorded");
}

/// Deleting the only instance must not remove or orphan the runtime: creating
/// again afterwards reuses it. Before the fix, RustFS re-downloaded here too.
#[test]
fn creating_after_deleting_the_first_instance_reuses_the_runtime() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt");
    let (body, sha) = rustfs_zip();
    let size = body.len() as u64;
    let (url, hits) = runtime.block_on(serve(body));

    let source = RuntimeSource {
        id: "rustfs-1.0.7-linux-x86_64".to_owned(),
        url,
        sha256: sha,
        size_bytes: size,
        format: ArchiveFormat::Zip,
        strip_prefix: None,
        engine: EngineKey::Rustfs,
        version: "1.0.7".to_owned(),
        expected_binaries: vec!["rustfs".to_owned()],
    };
    let h = Harness::with_options(EngineKey::Rustfs, |options| {
        options.runtime_sources = vec![source];
        options.download_policy = Some(UrlPolicy::allow_loopback_http_for_tests());
    });

    h.create(rustfs_spec("s3-first", super::harness::test_port()));
    let id = h.instances()[0].id().clone();
    let (_op, run) = h.run_call(|| {
        h.service
            .delete_instance(&id, crate::model::DeleteMode::PurgeData)
    });
    assert!(matches!(run.result(), Some(Ok(()))), "{:?}", run.result());
    assert!(h.instances().is_empty(), "the instance is gone");

    let again = h.create(rustfs_spec("s3-again", super::harness::test_port()));
    let stages = super::harness::stages(&again);
    assert!(
        !stages.contains(&"downloading"),
        "deleting an instance keeps the runtime; the next create must reuse it: \
         {stages:?}"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the archive is downloaded only for the very first create"
    );
}

/// A home left by the broken build: the runtime is on disk at the root but the
/// store never recorded it, so it is invisible to listing and removal. The fixed
/// build must adopt it — reuse it and add the missing row — with no manual
/// cleanup.
#[test]
fn a_runtime_left_by_the_broken_build_is_adopted_without_cleanup() {
    let h = Harness::with_options(EngineKey::Rustfs, |_| {});
    // Exactly what the current build produced: `runtimes/rustfs/1.0.7/<arch>/rustfs`.
    let dir = h
        .paths
        .runtime_dir(EngineKey::Rustfs, "1.0.7", crate::platform::Arch::X86_64)
        .expect("runtime dir");
    std::fs::create_dir_all(&dir).expect("runtime dir");
    std::fs::write(dir.join("rustfs"), b"\x7fELF legacy rustfs").expect("rustfs");

    assert!(
        h.service.list_runtimes().expect("runtimes").is_empty(),
        "the broken build left no runtime row"
    );

    let first = h.create(rustfs_spec("s3-legacy", super::harness::test_port()));
    let stages = super::harness::stages(&first);
    assert!(
        !stages.contains(&"downloading"),
        "the leftover runtime must be reused, not re-downloaded: {stages:?}"
    );

    let runtimes = h.service.list_runtimes().expect("runtimes");
    assert_eq!(runtimes.len(), 1, "reuse records the missing row");
    assert_eq!(
        runtimes[0].id(),
        "rustfs-1.0.7-x86_64",
        "so the runtime is now listable and removable"
    );

    // And a second instance keeps working, so adoption is not one-shot.
    let second = h.create(rustfs_spec("s3-legacy-two", super::harness::test_port()));
    let second_stages = super::harness::stages(&second);
    assert!(!second_stages.contains(&"downloading"), "{second_stages:?}");
    assert_eq!(h.instances().len(), 2);
}

/// Installs the **shipped** RustFS 1.0.7 zip twice through the real pipeline.
///
/// The fixture above encodes the archive layout from the catalog and the bytes
/// upstream publishes; this one proves the same against the real 107 MiB
/// archive, digest-checked against `catalog/engines-v3.json` before it runs.
///
/// Run with:
/// `cargo test -p dblocal-core --lib -- --ignored a_real_rustfs_archive_is_reused`
#[test]
#[ignore = "e2e: downloads the real 107 MiB RustFS archive from GitHub"]
fn a_real_rustfs_archive_is_reused() {
    let archive = std::env::var_os("DBLOCAL_RUSTFS_ZIP")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_default()
                .join(".cache/dblocal-dev/rustfs-linux-x86_64-musl-v1.0.7.zip")
        });
    let body =
        std::fs::read(&archive).unwrap_or_else(|err| panic!("read {}: {err}", archive.display()));
    let sha = sha2::Sha256::digest(&body)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        sha, "c90a40ba8cdc1468c71029a4d65fdd13449720183d3a8e00bc61ab09fcd448a5",
        "the local archive must be the exact catalog artifact"
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt");
    let size = body.len() as u64;
    let (url, hits) = runtime.block_on(serve(body));
    let source = RuntimeSource {
        id: "rustfs-1.0.7-linux-x86_64".to_owned(),
        url,
        sha256: sha,
        size_bytes: size,
        format: ArchiveFormat::Zip,
        strip_prefix: None,
        engine: EngineKey::Rustfs,
        version: "1.0.7".to_owned(),
        expected_binaries: vec!["rustfs".to_owned()],
    };
    let h = Harness::with_options(EngineKey::Rustfs, |options| {
        options.runtime_sources = vec![source];
        options.download_policy = Some(UrlPolicy::allow_loopback_http_for_tests());
    });

    h.create(rustfs_spec("real-one", super::harness::test_port()));
    let dir = h
        .paths
        .runtime_dir(EngineKey::Rustfs, "1.0.7", crate::platform::Arch::X86_64)
        .expect("runtime dir");
    assert!(
        dir.join("rustfs").is_file(),
        "the real binary is at the root"
    );
    assert!(!dir.join("bin").exists(), "the real archive has no `bin/`");

    let second = h.create(rustfs_spec("real-two", super::harness::test_port()));
    let stages = super::harness::stages(&second);
    assert!(!stages.contains(&"downloading"), "{stages:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "downloaded once");
    assert_eq!(h.instances().len(), 2);
}
