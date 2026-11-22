//! The real download pipeline, driven over loopback HTTP.
//!
//! This is the only W2-A test that exercises network code end to end. It runs a
//! tiny in-process server on `127.0.0.1:0` serving a real `tar.gz` whose SHA-256
//! is computed in the test, so verification, extraction and atomic finalisation
//! all run for real (the URL policy is the test-only loopback one).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::download::{CancellationToken, UrlPolicy};
use crate::model::{EngineKey, InstanceStatus};
use crate::service::{DblService, ServiceEvent};

use super::harness::{Harness, RuntimeSource, ServiceOptions, test_port};

/// Builds a `tar.gz` with `root/bin/engine` and a `root/data.txt` blob.
///
/// `padding` extra bytes are appended to the data file so a test can make the
/// body large enough to cancel mid-stream.
fn fixture_padded(padding: usize) -> (Vec<u8>, String) {
    let mut blob = b"hello\n".to_vec();
    blob.extend(std::iter::repeat_n(b'x', padding));
    let mut tar_bytes = Vec::new();
    {
        let enc = flate2::write::GzEncoder::new(&mut tar_bytes, flate2::Compression::fast());
        let mut ar = tar::Builder::new(enc);
        for (path, data, mode) in [
            ("root/bin/engine", &b"#!/bin/sh\nexit 0\n"[..], 0o755u32),
            ("root/data.txt", &blob[..], 0o644u32),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(mode);
            header.set_size(data.len() as u64);
            ar.append_data(&mut header, path, data).expect("append");
        }
        let enc = ar.into_inner().expect("tar finish");
        enc.finish().expect("gz finish");
    }
    use sha2::Digest;
    let digest = sha2::Sha256::digest(&tar_bytes);
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    (tar_bytes, hex)
}

/// The small fixture used by the plain success and checksum tests.
fn fixture() -> (Vec<u8>, String) {
    fixture_padded(0)
}

/// A server answering `GET` with a fixed body, optionally slow on the first hit.
async fn serve(body: Vec<u8>, slow_first: Option<Duration>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let first = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let body = body.clone();
            let counter = Arc::clone(&counter);
            let first = Arc::clone(&first);
            tokio::spawn(async move {
                let mut head = [0u8; 1024];
                let _ = socket.read(&mut head).await;
                counter.fetch_add(1, Ordering::SeqCst);
                let response_head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(response_head.as_bytes()).await;
                if first.fetch_add(1, Ordering::SeqCst) == 0
                    && let Some(delay) = slow_first
                {
                    // Trickle the body in small chunks with pauses, so the
                    // downloader is between chunks often enough to observe a
                    // cancellation (it only checks the token between chunks).
                    let chunk = (body.len() / 40).max(1);
                    for piece in body.chunks(chunk) {
                        if socket.write_all(piece).await.is_err() {
                            return;
                        }
                        tokio::time::sleep(delay).await;
                    }
                    return;
                }
                let _ = socket.write_all(&body).await;
            });
        }
    });
    (format!("http://{addr}/engine.tar.gz"), hits)
}

fn harness_with_source(url: String, sha256: String, size: u64, engine: EngineKey) -> Harness {
    let source = RuntimeSource {
        id: "test-engine".to_owned(),
        url,
        sha256,
        size_bytes: size,
        format: crate::model::ArchiveFormat::TarGz,
        strip_prefix: Some("root".to_owned()),
        engine,
        version: "18.7".to_owned(),
        // The fixture ships `bin/engine`, not a real engine binary.
        expected_binaries: vec!["bin/engine".to_owned()],
    };
    let harness = Harness::with_options(engine, |options| {
        options.runtime_sources = vec![source];
        options.download_policy = Some(UrlPolicy::allow_loopback_http_for_tests());
    });
    harness.uninstall_runtime(engine);
    harness
}

#[test]
fn create_downloads_verifies_and_extracts_a_real_archive() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt");
    let (body, sha) = fixture();
    let size = body.len() as u64;
    let (url, hits) = runtime.block_on(serve(body, None));

    let h = harness_with_source(url, sha, size, EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("downloaded", EngineKey::Postgresql, port);
    // Subscribe before the call, or the first (`queued`) stage can be missed.
    let mut receiver = h.receiver();
    let op = h.service.create_instance(spec).expect("create");
    let run = h.run_on(&mut receiver, op.clone());
    let events = &run.events;

    let stages = super::harness::stages(events);
    assert_eq!(stages.first(), Some(&"queued"), "{stages:?}");
    assert!(
        stages.contains(&"downloading"),
        "download stage emitted: {stages:?}"
    );
    assert!(
        stages.contains(&"verifying"),
        "verify stage emitted: {stages:?}"
    );
    if !stages.contains(&"extracting") {
        let debug: Vec<String> = events
            .iter()
            .map(|e| format!("{e:?}").chars().take(300).collect())
            .collect();
        panic!("extract stage emitted: {stages:?}\n{debug:#?}");
    }
    assert_eq!(stages.last(), Some(&"done"), "{stages:?}");
    assert!(matches!(run.result(), Some(Ok(()))));
    assert!(
        hits.load(Ordering::SeqCst) >= 1,
        "the server was asked once"
    );

    // The runtime tree exists and the binary kept its executable bit.
    let dir = h
        .paths
        .runtime_dir(EngineKey::Postgresql, "18.7", crate::platform::Arch::X86_64)
        .expect("runtime dir");
    let bin = dir.join("bin").join("engine");
    assert!(bin.is_file(), "extracted binary at {}", bin.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&bin).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "the executable bit is preserved");
    }
    assert!(h.instance_status("downloaded") == Some(InstanceStatus::Stopped));
}

#[test]
fn a_wrong_checksum_fails_and_leaves_no_runtime() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt");
    let (body, _) = fixture();
    let (url, _hits) = runtime.block_on(serve(body.clone(), None));
    let wrong = "0".repeat(64);

    let h = harness_with_source(url, wrong, body.len() as u64, EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("bad-hash", EngineKey::Postgresql, port);
    let (_op, run) = h.run_call(|| h.service.create_instance(spec));
    match run.result() {
        Some(Err(error)) => assert_eq!(error.code, crate::error::CODE_CHECKSUM_MISMATCH),
        other => panic!("expected a checksum mismatch, got {other:?}"),
    }
    let dir = h
        .paths
        .runtime_dir(EngineKey::Postgresql, "18.7", crate::platform::Arch::X86_64)
        .expect("runtime dir");
    assert!(!dir.join("bin").exists(), "no runtime may be finalised");
    assert!(h.instances().is_empty(), "no row may remain");
}

#[test]
fn cancelling_mid_download_stops_the_create() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt");
    let (body, sha) = fixture_padded(256 * 1024);
    let (url, _hits) = runtime.block_on(serve(body.clone(), Some(Duration::from_millis(200))));

    let h = harness_with_source(url, sha, body.len() as u64, EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("cancelled", EngineKey::Postgresql, port);
    // Subscribe before the call so the cancellation's events are all captured.
    let mut receiver = h.receiver();
    let op = h.service.create_instance(spec).expect("create");
    // Let the download begin (the server trickles), then cancel.
    std::thread::sleep(Duration::from_millis(400));
    h.service.cancel_operation(&op).expect("cancel");
    let run = h.run_on(&mut receiver, op.clone());
    match run.result() {
        Some(Err(error)) => assert_eq!(error.code, crate::error::CODE_CANCELLED),
        other => panic!("expected a cancellation, got {other:?}"),
    }
    assert!(h.instances().is_empty(), "nothing half-created remains");
    let dir = h
        .paths
        .runtime_dir(EngineKey::Postgresql, "18.7", crate::platform::Arch::X86_64)
        .expect("runtime dir");
    assert!(!dir.join("bin").exists(), "no runtime may be finalised");
}

#[allow(dead_code)]
fn _unused(_: CancellationToken, _: ServiceEvent, _: ServiceOptions) {}
