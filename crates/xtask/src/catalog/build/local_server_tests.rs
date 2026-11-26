//! Offline end-to-end tests: recorded fixture responses served by a real HTTP
//! server on an ephemeral (`:0`) loopback port.
//!
//! The Valkey fixture is per `Accept-Encoding`, so this also proves the builder
//! sends the header it means to send.

use super::sources::Source;
use super::*;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

struct EphemeralSource {
    base: String,
    client: reqwest::blocking::Client,
}

impl EphemeralSource {
    fn new(base: String) -> Self {
        Self {
            base,
            client: reqwest::blocking::Client::new(),
        }
    }

    fn map(&self, url: &str) -> Option<String> {
        if url.starts_with(
            "https://api.github.com/repos/theseus-rs/postgresql-binaries/releases?per_page=100",
        ) {
            return Some(format!("{}/postgres-releases.json", self.base));
        }
        url.strip_prefix("https://example.test/pg/")
            .map(|path| format!("{}/pg/{path}", self.base))
            .or_else(|| {
                url.strip_prefix("https://download.valkey.io/releases/")
                    .map(|path| format!("{}/valkey/{path}", self.base))
            })
    }

    fn call(
        &self,
        method: reqwest::Method,
        url: &str,
        encoding: &str,
    ) -> Result<Vec<u8>, sources::SourceError> {
        let mapped = self
            .map(url)
            .ok_or_else(|| sources::SourceError::new(format!("no local mapping for {url}")))?;
        let response = self
            .client
            .request(method, &mapped)
            .header(reqwest::header::ACCEPT_ENCODING, encoding)
            .send()
            .map_err(|err| sources::SourceError::new(format!("{mapped} failed: {err}")))?;
        if !response.status().is_success() {
            return Err(sources::SourceError::new(format!(
                "{mapped} returned HTTP {}",
                response.status().as_u16()
            )));
        }
        response
            .bytes()
            .map(|bytes| bytes.to_vec())
            .map_err(|err| sources::SourceError::new(format!("{mapped} body failed: {err}")))
    }
}

impl Source for EphemeralSource {
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, sources::SourceError> {
        self.call(reqwest::Method::GET, url, "identity")
    }

    fn get_text_with(
        &self,
        url: &str,
        accept_encoding: &str,
    ) -> Result<String, sources::SourceError> {
        let bytes = self.call(reqwest::Method::GET, url, accept_encoding)?;
        String::from_utf8(bytes)
            .map_err(|err| sources::SourceError::new(format!("{url} is not UTF-8: {err}")))
    }

    fn head_size(&self, url: &str, accept_encoding: &str) -> Result<u64, sources::SourceError> {
        // Mirror a real HEAD: ask the server for the length of the object it
        // would serve for this encoding.
        let mapped = self
            .map(url)
            .ok_or_else(|| sources::SourceError::new(format!("no local mapping for {url}")))?;
        let response = self
            .client
            .head(&mapped)
            .header(reqwest::header::ACCEPT_ENCODING, accept_encoding)
            .send()
            .map_err(|err| sources::SourceError::new(format!("HEAD {mapped} failed: {err}")))?;
        // A HEAD reply has no body, so read the declared length.
        response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| sources::SourceError::new(format!("HEAD {mapped} had no length")))
    }
}

/// One recorded upstream object, as served per `Accept-Encoding`.
#[derive(Default)]
struct Route {
    identity: Vec<u8>,
    gzip: Vec<u8>,
    identity_len: Option<u64>,
    gzip_len: Option<u64>,
}

impl Route {
    fn same(body: Vec<u8>) -> Self {
        let len = body.len() as u64;
        Self {
            identity: body.clone(),
            gzip: body,
            identity_len: Some(len),
            gzip_len: Some(len),
        }
    }
}

fn serve_fixtures() -> String {
    let base = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let releases = std::fs::read(format!("{base}/postgres-releases.json")).expect("releases");
    let pg_sidecars: BTreeMap<String, String> = serde_json::from_str(
        &std::fs::read_to_string(format!("{base}/postgres-sidecars.json")).expect("pg sidecars"),
    )
    .expect("parse pg sidecars");
    let vk_sidecars: BTreeMap<String, BTreeMap<String, String>> = serde_json::from_str(
        &std::fs::read_to_string(format!("{base}/valkey-sidecars.json")).expect("vk sidecars"),
    )
    .expect("parse vk sidecars");

    let mut routes: BTreeMap<String, Route> = BTreeMap::new();
    let vk_heads: BTreeMap<String, BTreeMap<String, u64>> = serde_json::from_str(
        &std::fs::read_to_string(format!("{base}/valkey-heads.json")).expect("vk heads"),
    )
    .expect("parse vk heads");

    routes.insert("/postgres-releases.json".to_owned(), Route::same(releases));
    for (path, body) in pg_sidecars {
        routes.insert(path, Route::same(body.into_bytes()));
    }
    for (path, encs) in vk_sidecars {
        routes.insert(
            path,
            Route {
                identity: encs
                    .get("identity")
                    .cloned()
                    .unwrap_or_default()
                    .into_bytes(),
                gzip: encs.get("gzip").cloned().unwrap_or_default().into_bytes(),
                identity_len: None,
                gzip_len: None,
            },
        );
    }
    // Archive HEAD routes: real upstream sizes, so the builder's HEAD probe
    // sees a length without downloading tens of megabytes.
    for (path, encs) in vk_heads {
        routes.insert(
            path,
            Route {
                identity: Vec::new(),
                gzip: Vec::new(),
                identity_len: encs.get("identity").copied(),
                gzip_len: encs.get("gzip").copied(),
            },
        );
    }

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let read = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..read]);
            let mut lines = request.lines();
            let request_line = lines.next().unwrap_or_default();
            let mut request_parts = request_line.split_whitespace();
            let method = request_parts.next().unwrap_or("GET");
            let path = request_parts.next().unwrap_or("/");
            let wants_gzip = lines.any(|line| {
                line.to_ascii_lowercase().starts_with("accept-encoding:")
                    && line.to_ascii_lowercase().contains("gzip")
            });
            match routes.get(path) {
                Some(route) => {
                    let (body, length) = if wants_gzip {
                        (
                            route.gzip.clone(),
                            route.gzip_len.unwrap_or(route.gzip.len() as u64),
                        )
                    } else {
                        (
                            route.identity.clone(),
                            route.identity_len.unwrap_or(route.identity.len() as u64),
                        )
                    };
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
                    );
                    let _ = stream.write_all(head.as_bytes());
                    if method != "HEAD" {
                        let _ = stream.write_all(&body);
                    }
                }
                None => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 7\r\nConnection: close\r\n\r\nmissing",
                    );
                }
            }
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn build_reads_recorded_fixtures_from_a_local_server_on_port_0() {
    let base = serve_fixtures();
    let source = EphemeralSource::new(base);
    let selection = BuildSelection {
        postgresql: vec![super::tests::fixture_tracked("18.7.0", "18")],
        valkey: vec![super::tests::fixture_tracked("9.2.0", "9.2")],
        redis: Vec::new(),
        rustfs: false,
        mariadb: Vec::new(),
        mysql: Vec::new(),
    };
    let document = build(&source, &selection, Some("2026-11-20T00:00:00Z".to_owned()))
        .expect("build from local fixtures");
    dblocal_core::catalog::validate_document(&document).expect("valid");
    assert_eq!(document.versions.len(), 2);
    // 6 PostgreSQL targets + 2 Valkey jammy targets.
    assert_eq!(document.artifacts.len(), 8);
}
