//! Download, verification and extraction tests.
//!
//! Network access is forbidden: every HTTP scenario runs against an in-process
//! TCP server bound to `127.0.0.1:0` that can serve byte ranges, truncate a
//! response mid-body, ignore `Range`, or return a wrong payload. Archive
//! fixtures are built in memory; nothing is downloaded from the internet and no
//! real engine process is started.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod bounds;
mod download_flow;
mod extraction;
mod finalisation;
mod install;

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::model::ArchiveFormat;

fn temp_base() -> PathBuf {
    std::env::temp_dir().join(format!("dblocal-test-{:016x}", rand::random::<u64>()))
}

// ---------------------------------------------------------------------------
// HTTP test server
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ServeConfig {
    body: Vec<u8>,
    etag: Option<String>,
    /// Ignore `Range` and always reply 200 with the full body.
    ignore_range: bool,
    /// On the first request, stop after this many body bytes and close abruptly.
    truncate_first_to: Option<usize>,
    /// Always reply with this status and an empty body.
    force_status: Option<u16>,
    /// Reply with this status on the first N requests, then behave normally.
    fail_first: Option<(usize, u16)>,
    /// Reply 206 with a `Content-Range` whose start does not match the request.
    lie_about_content_range: bool,
    /// Close the connection without writing any response on the first request.
    drop_first: bool,
    /// Stream this many extra body bytes past the declared size (an endless or
    /// overlong body). `None` serves exactly `body`.
    extra_bytes: Option<usize>,
    /// Advertise `Content-Length: <body.len() + extra>` instead of the real
    /// length, simulating a server that promises more than it will send, or
    /// more than the catalog expects.
    advertise_extra_length: Option<u64>,
    /// Send the body with no `Content-Length` (chunked framing) and stop after
    /// only this many bytes: a complete response whose body ends early.
    clean_short_body_to: Option<usize>,
    /// On the first request only, send a complete chunked body of this many
    /// bytes (a clean short response); later requests behave normally.
    short_first_to: Option<usize>,
    /// Send the whole payload with chunked framing and no `Content-Length`, so
    /// an overlong body cannot be rejected from a length header and the streamed
    /// guard has to catch it.
    chunked: bool,
    /// Add `Accept-Encoding: identity` handling: gzip the body unless the request
    /// already asks for identity.
    gzip_unless_identity: bool,
    /// Always answer with `Content-Encoding: gzip` and a gzip body, even when the
    /// request asked for identity (a server that ignores the request).
    always_gzip: bool,
    /// Emit `Content-Encoding: identity` explicitly (legal and equivalent to
    /// omitting the header).
    explicit_identity_encoding: bool,
}

impl ServeConfig {
    fn normal(body: Vec<u8>) -> Self {
        Self {
            body,
            etag: Some("\"test-etag\"".to_owned()),
            ignore_range: false,
            truncate_first_to: None,
            force_status: None,
            fail_first: Some((0, 500)),
            lie_about_content_range: false,
            drop_first: false,
            extra_bytes: None,
            advertise_extra_length: None,
            clean_short_body_to: None,
            short_first_to: None,
            chunked: false,
            gzip_unless_identity: false,
            always_gzip: false,
            explicit_identity_encoding: false,
        }
    }

    fn with_fail_first(mut self, count: usize, status: u16) -> Self {
        self.fail_first = Some((count, status));
        self
    }

    fn with_lie_about_content_range(mut self) -> Self {
        self.lie_about_content_range = true;
        self
    }

    fn with_drop_first(mut self) -> Self {
        self.drop_first = true;
        self
    }

    /// Serve `body` followed by `extra` more bytes, so the stream is longer than
    /// the catalog's expected size.
    fn with_extra_bytes(mut self, extra: usize) -> Self {
        self.extra_bytes = Some(extra);
        self
    }

    /// Advertise a `Content-Length` larger than the real body.
    fn with_advertised_extra_length(mut self, extra: u64) -> Self {
        self.advertise_extra_length = Some(extra);
        self
    }

    /// Send the body without `Content-Length` and close after `n` bytes.
    fn with_clean_short_body_to(mut self, n: usize) -> Self {
        self.clean_short_body_to = Some(n);
        self
    }

    /// Answer the first request with a complete chunked body of only `n` bytes;
    /// later requests behave normally.
    fn with_short_first(mut self, n: usize) -> Self {
        self.short_first_to = Some(n);
        self
    }

    /// Frame the payload as chunked with no `Content-Length` at all.
    fn with_chunked(mut self) -> Self {
        self.chunked = true;
        self
    }

    /// gzip the body unless the request asks for `Accept-Encoding: identity`.
    fn with_gzip_unless_identity(mut self) -> Self {
        self.gzip_unless_identity = true;
        self
    }

    /// Always gzip and mark `Content-Encoding: gzip`, ignoring what was asked.
    fn with_always_gzip(mut self) -> Self {
        self.always_gzip = true;
        self
    }

    /// Send `Content-Encoding: identity` explicitly.
    fn with_explicit_identity_encoding(mut self) -> Self {
        self.explicit_identity_encoding = true;
        self
    }
}

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

async fn start_server(cfg: ServeConfig) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind :0");
    let addr = listener.local_addr().expect("local addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let cfg = cfg.clone();
            let hits = counter.clone();
            tokio::spawn(async move {
                let _ = serve(stream, cfg, hits).await;
            });
        }
    });
    TestServer { addr, hits }
}

async fn serve(
    mut stream: tokio::net::TcpStream,
    cfg: ServeConfig,
    hits: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    let req = read_request_head(&mut stream).await?;
    let hit = hits.fetch_add(1, Ordering::SeqCst);

    if cfg.drop_first && hit == 0 {
        return Ok(());
    }

    if let Some((count, status)) = cfg.fail_first
        && count > 0
        && hit < count
    {
        let resp =
            format!("HTTP/1.1 {status} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        stream.write_all(resp.as_bytes()).await?;
        return Ok(());
    }

    if let Some(status) = cfg.force_status {
        let resp =
            format!("HTTP/1.1 {status} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        stream.write_all(resp.as_bytes()).await?;
        return Ok(());
    }

    let range = parse_range_start(&req);
    let mut headers = Vec::new();
    let (status, mut payload) = match (range, cfg.ignore_range) {
        (Some(start), false) if start < cfg.body.len() => {
            let slice = cfg.body[start..].to_vec();
            headers.push("Accept-Ranges: bytes".to_owned());
            let reported_start = if cfg.lie_about_content_range && start > 0 {
                start - 1
            } else {
                start
            };
            let advertised_total =
                (cfg.body.len() as u64).saturating_add(cfg.advertise_extra_length.unwrap_or(0));
            headers.push(format!(
                "Content-Range: bytes {}-{}/{}",
                reported_start,
                cfg.body.len() - 1,
                advertised_total
            ));
            (206u16, slice)
        }
        (Some(_), false) => {
            let resp = format!(
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                cfg.body.len()
            );
            stream.write_all(resp.as_bytes()).await?;
            return Ok(());
        }
        _ => {
            headers.push("Accept-Ranges: bytes".to_owned());
            (200u16, cfg.body.clone())
        }
    };

    // --- content encoding ---------------------------------------------------
    // A server that "gzips unless identity is requested" models the Valkey CDN:
    // two different objects for the same URL depending on `Accept-Encoding`.
    let asked_identity = request_accepts_identity(&req);
    let gzip_body = cfg.always_gzip || (cfg.gzip_unless_identity && !asked_identity);
    if gzip_body {
        payload = gzip_bytes(&payload);
        headers.push("Content-Encoding: gzip".to_owned());
    } else if cfg.explicit_identity_encoding {
        headers.push("Content-Encoding: identity".to_owned());
    }

    // --- overlong bodies ----------------------------------------------------
    if let Some(extra) = cfg.extra_bytes {
        payload.extend(std::iter::repeat_n(0xABu8, extra));
    }

    // --- length headers ----------------------------------------------------
    // A "clean early EOF" must not advertise a length at all; otherwise the
    // client would (correctly) see a transport truncation instead. The same is
    // true for the first-response-only short body.
    let short_body_limit = cfg
        .clean_short_body_to
        .or_else(|| (hit == 0).then_some(cfg.short_first_to).flatten());
    let tolerate_short = short_body_limit.is_some();
    if cfg.chunked {
        headers.push("Transfer-Encoding: chunked".to_owned());
    } else if !tolerate_short {
        // On a `206` the length header must describe only the bytes in this
        // response, so `advertise_extra_length` is carried by the `Content-Range`
        // total above instead; anything else would be a transport-level lie.
        let advertised = if status == 206 {
            payload.len() as u64
        } else {
            (payload.len() as u64).saturating_add(cfg.advertise_extra_length.unwrap_or(0))
        };
        headers.push(format!("Content-Length: {advertised}"));
    } else {
        headers.push("Transfer-Encoding: chunked".to_owned());
    }
    if let Some(etag) = &cfg.etag {
        headers.push(format!("ETag: {etag}"));
    }

    let reason = if status == 206 {
        "Partial Content"
    } else {
        "OK"
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    for h in &headers {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;

    if hit == 0
        && let Some(limit) = cfg.truncate_first_to
    {
        let send = limit.min(payload.len());
        stream.write_all(&payload[..send]).await?;
        stream.flush().await?;
        return Ok(());
    }

    if cfg.chunked {
        // Chunked framing of the entire payload: a complete response with no
        // declared length, so only the streamed size guard can reject it.
        let mut framed = Vec::new();
        for chunk in payload.chunks(4096) {
            framed.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            framed.extend_from_slice(chunk);
            framed.extend_from_slice(b"\r\n");
        }
        framed.extend_from_slice(b"0\r\n\r\n");
        stream.write_all(&framed).await?;
        stream.flush().await?;
        return Ok(());
    }

    if let Some(short_to) = short_body_limit {
        // Chunked framing with a final zero-length chunk: a *complete* response
        // whose body merely stops early. This is not a transport error, which is
        // exactly the case the retry logic must classify as a truncation.
        let send = short_to.min(payload.len());
        let mut framed = Vec::new();
        if send > 0 {
            framed.extend_from_slice(format!("{send:x}\r\n").as_bytes());
            framed.extend_from_slice(&payload[..send]);
            framed.extend_from_slice(b"\r\n");
        }
        framed.extend_from_slice(b"0\r\n\r\n");
        stream.write_all(&framed).await?;
        stream.flush().await?;
        return Ok(());
    }

    stream.write_all(&payload).await?;
    stream.flush().await?;
    Ok(())
}

fn gzip_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
        enc.write_all(data).expect("gzip write");
        enc.finish().expect("gzip finish");
    }
    out
}

fn request_accepts_identity(req: &str) -> bool {
    for line in req.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.eq_ignore_ascii_case("accept-encoding") {
            continue;
        }
        return value
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("identity"));
    }
    false
}

async fn read_request_head(stream: &mut tokio::net::TcpStream) -> std::io::Result<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(String::from_utf8_lossy(&buf).into_owned());
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(String::from_utf8_lossy(&buf).into_owned());
        }
    }
}

fn parse_range_start(req: &str) -> Option<usize> {
    for line in req.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.eq_ignore_ascii_case("range") {
            continue;
        }
        let rest = value.trim().strip_prefix("bytes=")?;
        let (start, _) = rest.split_once('-')?;
        return start.trim().parse().ok();
    }
    None
}

fn test_download_options() -> super::fetch::DownloadOptions {
    super::fetch::DownloadOptions {
        policy: super::fetch::UrlPolicy::allow_loopback_http_for_tests(),
        cancel: super::fetch::CancellationToken::new(),
        max_retries: 5,
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(40),
    }
}

// ---------------------------------------------------------------------------
// Archive builders
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum TarEntry {
    Dir {
        path: String,
        mode: u32,
    },
    File {
        path: String,
        data: Vec<u8>,
        mode: u32,
    },
    Symlink {
        path: String,
        target: String,
    },
    Hardlink {
        path: String,
        target: String,
    },
    /// Raw header for names the tar writer refuses to construct (absolute paths).
    Raw {
        header: Box<tar::Header>,
        data: Vec<u8>,
    },
}

impl TarEntry {
    fn apply<W: std::io::Write>(&self, ar: &mut tar::Builder<W>) {
        match self {
            TarEntry::Dir { path, mode } => {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Directory);
                h.set_mode(*mode);
                h.set_size(0);
                ar.append_data(&mut h, path, std::io::empty())
                    .expect("append dir");
            }
            TarEntry::File { path, data, mode } => {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Regular);
                h.set_mode(*mode);
                h.set_size(data.len() as u64);
                ar.append_data(&mut h, path, &data[..])
                    .expect("append file");
            }
            TarEntry::Symlink { path, target } => {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_size(0);
                ar.append_link(&mut h, path, target)
                    .expect("append symlink");
            }
            TarEntry::Hardlink { path, target } => {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Link);
                h.set_size(0);
                ar.append_link(&mut h, path, target)
                    .expect("append hardlink");
            }
            TarEntry::Raw { header, data } => {
                let h = (**header).clone();
                ar.append(&h, &data[..]).expect("append raw");
            }
        }
    }
}

fn make_tar_gz(entries: Vec<TarEntry>) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let enc = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
        let mut ar = tar::Builder::new(enc);
        for entry in entries {
            entry.apply(&mut ar);
        }
        let enc = ar.into_inner().expect("tar finish");
        enc.finish().expect("gz finish");
    }
    out
}

fn make_tar_xz(entries: Vec<TarEntry>) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let enc = liblzma::write::XzEncoder::new(&mut out, 0);
        let mut ar = tar::Builder::new(enc);
        for entry in entries {
            entry.apply(&mut ar);
        }
        let enc = ar.into_inner().expect("tar finish");
        enc.finish().expect("xz finish");
    }
    out
}

#[derive(Clone)]
enum ZipEntry {
    File {
        name: String,
        data: Vec<u8>,
        unix_mode: Option<u32>,
    },
    Dir {
        name: String,
    },
    Symlink {
        name: String,
        target: String,
    },
}

impl ZipEntry {
    fn apply<W: std::io::Write + std::io::Seek>(&self, w: &mut zip::ZipWriter<W>) {
        use zip::write::SimpleFileOptions;
        match self {
            ZipEntry::File {
                name,
                data,
                unix_mode,
            } => {
                let mut opts = SimpleFileOptions::default();
                if let Some(mode) = unix_mode {
                    opts = opts.unix_permissions(*mode);
                }
                w.start_file(name, opts).expect("start file");
                w.write_all(data).expect("write file");
            }
            ZipEntry::Dir { name } => {
                w.add_directory(name, SimpleFileOptions::default())
                    .expect("add dir");
            }
            ZipEntry::Symlink { name, target } => {
                w.add_symlink(name, target, SimpleFileOptions::default())
                    .expect("add symlink");
            }
        }
    }
}

fn make_zip(entries: Vec<ZipEntry>) -> Vec<u8> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut w = zip::ZipWriter::new(cursor);
    for entry in entries {
        entry.apply(&mut w);
    }
    w.finish().expect("zip finish").into_inner()
}

// ---------------------------------------------------------------------------
// Extraction harness
// ---------------------------------------------------------------------------

struct ExtractRun {
    base: PathBuf,
    result: crate::Result<PathBuf>,
}

impl ExtractRun {
    fn final_dir(&self) -> PathBuf {
        self.base.join("runtime")
    }
}

async fn run_extract(
    bytes: &[u8],
    fmt: ArchiveFormat,
    strip: Option<&str>,
    expected: &[&str],
    max_total_bytes: Option<u64>,
    max_entries: Option<u64>,
) -> ExtractRun {
    let base = temp_base();
    tokio::fs::create_dir_all(&base).await.expect("create base");
    let archive_name = match fmt {
        ArchiveFormat::TarGz => "archive.tar.gz",
        ArchiveFormat::TarXz => "archive.tar.xz",
        ArchiveFormat::Zip => "archive.zip",
    };
    let archive = base.join(archive_name);
    tokio::fs::write(&archive, bytes)
        .await
        .expect("write archive");

    let options = super::extract::ExtractOptions {
        expected_binaries: expected.iter().map(|s| (*s).to_owned()).collect(),
        strip_prefix: strip.map(str::to_owned),
        max_total_bytes,
        max_entries,
    };
    let final_dir = base.join("runtime");
    let result =
        super::extract::extract_runtime(&archive, fmt, bytes.len() as u64, &final_dir, &options)
            .await;
    ExtractRun { base, result }
}

fn staging_leftovers(base: &Path) -> Vec<String> {
    std::fs::read_dir(base)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name.contains(".extracting-"))
                .collect()
        })
        .unwrap_or_default()
}

fn assert_rejected(run: &ExtractRun, label: &str) {
    let err = run
        .result
        .as_ref()
        .err()
        .unwrap_or_else(|| panic!("{label}: expected rejection"));
    assert!(
        matches!(
            err,
            crate::DblError::UnsafeArchive { .. } | crate::DblError::InvalidInput { .. }
        ),
        "{label}: expected UnsafeArchive/InvalidInput, got {err:?}"
    );
    assert!(
        !run.final_dir().exists(),
        "{label}: final runtime directory must not exist"
    );
    assert!(
        staging_leftovers(&run.base).is_empty(),
        "{label}: staging directory left behind"
    );
}
