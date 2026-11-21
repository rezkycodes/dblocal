#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::update::verify::sha256_file_hex;

struct TestServer {
    addr: SocketAddr,
    hits: Arc<AtomicUsize>,
}

impl TestServer {
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

async fn start_server(body: Vec<u8>) -> TestServer {
    start_server_with_status(200, body).await
}

async fn start_server_with_status(status: u16, body: Vec<u8>) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let body = body.clone();
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                counter.fetch_add(1, Ordering::SeqCst);
                let resp = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    TestServer { addr, hits }
}

#[tokio::test]
async fn download_and_verify_accepts_valid_file() {
    let body = b"hello-update".to_vec();
    let server = start_server(body.clone()).await;
    let dir = tempdir().expect("tmp");
    let dest = dir.path().join("asset.bin");
    tokio::fs::write(&dest.with_extension("tmp-source"), &body)
        .await
        .unwrap();
    let expected = sha256_file_hex(&dest.with_extension("tmp-source"))
        .await
        .unwrap();
    tokio::fs::remove_file(dest.with_extension("tmp-source"))
        .await
        .unwrap();

    download_and_verify(
        &server.url("/asset.bin"),
        &dest,
        &expected,
        body.len() as u64,
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect("download");
    assert_eq!(tokio::fs::read(&dest).await.unwrap(), body);
    assert!(server.hits() >= 1);
}

#[tokio::test]
async fn download_and_verify_rejects_mismatch() {
    let body = b"hello-update".to_vec();
    let server = start_server(body.clone()).await;
    let dir = tempdir().expect("tmp");
    let dest = dir.path().join("asset.bin");
    let err = download_and_verify(
        &server.url("/asset.bin"),
        &dest,
        &"0".repeat(64),
        body.len() as u64,
        &UpdateDownloadOptions::loopback_for_tests(),
    )
    .await
    .expect_err("mismatch");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_VERIFY_FAILED);
    assert!(!dest.exists());
}

#[test]
fn validate_url_rejects_non_allowed_host() {
    let url = reqwest::Url::parse("https://evil.example.com/file.tar.gz").unwrap();
    let err = validate_url(&url, &UpdateDownloadOptions::default()).expect_err("host");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
}

#[test]
fn validate_url_rejects_http_in_production() {
    let url = reqwest::Url::parse("http://github.com/file.tar.gz").unwrap();
    let err = validate_url(&url, &UpdateDownloadOptions::default()).expect_err("http");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_NETWORK);
}

#[test]
fn validate_url_allows_github_hosts() {
    let opts = UpdateDownloadOptions::default();
    let url = reqwest::Url::parse("https://github.com/a/b/releases/download/v1/x.tar.gz").unwrap();
    validate_url(&url, &opts).expect("github");
    let url = reqwest::Url::parse("https://objects.githubusercontent.com/a/b").unwrap();
    validate_url(&url, &opts).expect("objects");
}

#[test]
fn effective_max_rejects_declared_over_hard_cap() {
    let err = effective_max(MAX_UPDATE_BYTES + 1, MAX_UPDATE_BYTES).expect_err("over");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_MALFORMED);
}
