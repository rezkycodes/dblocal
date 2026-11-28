//! CLI toolchain handler tests.
//!
//! These drive `run_with` against a real `ToolchainService` over a temp home, with
//! injected writers and confirmation, so no terminal and no network are involved.
//! Install tests point the service at a loopback fixture server through the
//! test-only URL policy.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use dblocal_core::platform::{Arch, Os};
use dblocal_core::toolchain::catalog::{CatalogState, CatalogStatus, parse_catalog_allow_loopback};
use dblocal_core::toolchain::fsops::ToolchainPaths;
use dblocal_core::toolchain::ops::ToolchainService;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::cli::ToolchainCommand;

/// Captured output for one handler run.
struct Captured {
    code: u8,
    stdout: String,
    stderr: String,
}

fn run(
    service: &ToolchainService,
    command: ToolchainCommand,
    interactive: bool,
    answer: Option<&str>,
) -> Captured {
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let reply = answer.map(|s| s.to_owned());
    let confirm = move || reply.clone();
    let code = super::dispatch_on(service, &command, &mut out, &mut err, interactive, &confirm);
    Captured {
        code,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    }
}

/// A temp home. No process environment is changed: the injected service already
/// carries its root, so tests stay parallel-safe.
struct Home {
    dir: TempDir,
}

impl Home {
    fn new() -> Self {
        Self {
            dir: TempDir::new().expect("temp home"),
        }
    }
    fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

// A single fixture server shared by the install tests would race on the process
// environment (`DBLOCAL_HOME`), so each install test owns its own home and runs
// serially through one test to avoid the global env.
#[test]
fn list_which_and_doctor_over_an_empty_home() {
    let home = Home::new();

    let listed = run(
        &service_for(&home),
        ToolchainCommand::List {
            tool: None,
            installed: false,
            json: true,
        },
        false,
        None,
    );
    assert_eq!(listed.code, 0, "stderr: {}", listed.stderr);
    // `list --json` always emits a valid JSON array (populated once the real
    // catalog is embedded); `--installed` over an empty home is always empty.
    let parsed: serde_json::Value = serde_json::from_str(listed.stdout.trim()).expect("valid json");
    assert!(parsed.is_array(), "expected JSON array: {}", listed.stdout);

    let installed_only = run(
        &service_for(&home),
        ToolchainCommand::List {
            tool: None,
            installed: true,
            json: true,
        },
        false,
        None,
    );
    assert_eq!(installed_only.code, 0, "stderr: {}", installed_only.stderr);
    assert_eq!(installed_only.stdout.trim(), "[]");

    let which = run(
        &service_for(&home),
        ToolchainCommand::Which {
            tool: "php".to_owned(),
        },
        false,
        None,
    );
    assert_eq!(which.code, EXIT_NOT_FOUND, "no active php");

    let doctor = run(
        &service_for(&home),
        ToolchainCommand::Doctor { json: true },
        false,
        None,
    );
    assert_eq!(doctor.code, 0);
    assert!(
        doctor.stdout.contains("shims_dir_missing"),
        "{}",
        doctor.stdout
    );
}

#[test]
fn list_rejects_an_unknown_tool() {
    let home = Home::new();
    let result = run(
        &service_for(&home),
        ToolchainCommand::List {
            tool: Some("ruby".to_owned()),
            installed: false,
            json: false,
        },
        false,
        None,
    );
    assert_eq!(result.code, EXIT_BAD_ARGS);
    assert!(result.stderr.contains("Unknown tool"), "{}", result.stderr);
}

#[test]
fn path_print_writes_nothing_and_shows_the_block() {
    let home = Home::new();
    let result = run(
        &service_for(&home),
        ToolchainCommand::Path {
            print: true,
            apply: false,
            remove: false,
            yes: false,
        },
        false,
        None,
    );
    assert_eq!(result.code, 0);
    assert!(
        result
            .stdout
            .contains(dblocal_core::toolchain::consts::PATH_BLOCK_BEGIN)
    );
    // Nothing was written anywhere.
    assert!(!home.path().join(".bashrc").exists());
}

#[test]
fn path_apply_needs_consent_and_is_reversible() {
    let home = Home::new();
    std::fs::write(home.path().join(".bashrc"), "# rc\n").expect("rc");
    let service = service_for(&home);

    // Non-interactive apply without consent refuses.
    let refused = run(
        &service,
        ToolchainCommand::Path {
            print: false,
            apply: true,
            remove: false,
            yes: false,
        },
        false,
        None,
    );
    assert_eq!(refused.code, EXIT_BAD_ARGS);
    assert!(refused.stderr.contains("--yes"), "{}", refused.stderr);

    // Interactive apply with "y" writes the block.
    let applied = run(
        &service,
        ToolchainCommand::Path {
            print: false,
            apply: true,
            remove: false,
            yes: false,
        },
        true,
        Some("y"),
    );
    assert_eq!(applied.code, 0, "stderr: {}", applied.stderr);
    let text = std::fs::read_to_string(home.path().join(".bashrc")).expect("read");
    assert!(
        text.contains(dblocal_core::toolchain::consts::PATH_BLOCK_BEGIN),
        "{text}"
    );

    // Remove restores the original bytes.
    let removed = run(
        &service,
        ToolchainCommand::Path {
            print: false,
            apply: false,
            remove: true,
            yes: false,
        },
        false,
        None,
    );
    assert_eq!(removed.code, 0, "stderr: {}", removed.stderr);
    assert_eq!(
        std::fs::read_to_string(home.path().join(".bashrc")).unwrap(),
        "# rc\n"
    );
}

#[test]
fn php_settings_get_and_unknown_action() {
    let home = Home::new();
    let service = service_for(&home);
    let got = run(
        &service,
        ToolchainCommand::PhpSettings {
            action: "get".to_owned(),
            key: None,
            value: None,
            json: true,
        },
        false,
        None,
    );
    assert_eq!(got.code, 0);
    assert!(got.stdout.contains("memory_limit"), "{}", got.stdout);

    let bad = run(
        &service,
        ToolchainCommand::PhpSettings {
            action: "frobnicate".to_owned(),
            key: None,
            value: None,
            json: false,
        },
        false,
        None,
    );
    assert_eq!(bad.code, EXIT_BAD_ARGS);
}

#[test]
fn php_settings_set_without_active_php_is_a_clear_error() {
    let home = Home::new();
    let service = service_for(&home);
    let result = run(
        &service,
        ToolchainCommand::PhpSettings {
            action: "set".to_owned(),
            key: Some("memory_limit".to_owned()),
            value: Some("1G".to_owned()),
            json: false,
        },
        false,
        None,
    );
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("No active"), "{}", result.stderr);
}

#[test]
fn install_activation_and_use_round_trip() {
    let home = Home::new();

    // Build a catalog with one node version pointed at a loopback fixture server.
    let runtime = Runtime::new();
    let wrapper = "node-v24.11.0-linux-x64";
    let data = {
        let mut out = Vec::new();
        let enc = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
        let mut ar = tar::Builder::new(enc);
        for name in ["node", "npm", "npx"] {
            let path = format!("{wrapper}/bin/{name}");
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Regular);
            h.set_mode(0o755);
            h.set_size(1);
            ar.append_data(&mut h, &path, &b"x"[..]).expect("entry");
        }
        let enc = ar.into_inner().expect("tar");
        enc.finish().expect("gz");
        out
    };
    let url = runtime.serve(data.clone());
    let sha = sha_hex(&data);
    let doc = format!(
        r#"{{"schema":1,"generated_at":"2026-11-27T00:00:00Z","source_url":"https://example.test/t.json",
        "tools":[{{"id":"node","versions":[{{"version":"24.11.0","series":"24","lts":true,
        "artifacts":[{{"os":"linux","arch":"x86_64","profile":null,"format":"tar.gz",
        "url":"{url}","sha256":"{sha}","size_bytes":{},"source":"x","hash_origin":"upstream"}}]}}]}}]}}"#,
        data.len()
    );
    let (catalog, skips) = parse_catalog_allow_loopback(&doc).expect("catalog");
    let catalog_state = CatalogState::new(
        catalog,
        doc,
        CatalogStatus::Fresh,
        None,
        std::time::SystemTime::now(),
        skips,
    );

    let paths = ToolchainPaths::with_root(home.path());
    let service = ToolchainService::new(paths, home.path())
        .with_platform(Os::Linux, Arch::X86_64)
        .with_catalog(catalog_state)
        .with_loopback_http();

    // install activates because nothing of node is active yet.
    let report = runtime
        .block_on(service.install(
            dblocal_core::toolchain::ToolId::Node,
            &dblocal_core::toolchain::catalog::VersionSpec::parse("24").unwrap(),
            None,
            true,
        ))
        .expect("install");
    assert!(report.activated);
    assert!(
        service
            .active(dblocal_core::toolchain::ToolId::Node)
            .is_some()
    );
    assert!(service.paths().shim_path("node", Os::Linux).is_file());

    // which resolves to the installed node.
    let which = service
        .active(dblocal_core::toolchain::ToolId::Node)
        .map(|a| {
            service
                .paths()
                .install_dir(a.tool(), a.version(), a.profile())
                .join("bin/node")
        })
        .expect("active");
    assert!(which.is_file(), "{}", which.display());
}

#[test]
fn use_composer_without_php_succeeds_with_warning() {
    let home = Home::new();
    let paths = ToolchainPaths::with_root(home.path());
    let comp_dir = paths.install_dir(dblocal_core::toolchain::ToolId::Composer, "2.11.3", None);
    std::fs::create_dir_all(&comp_dir).expect("mkdir");
    std::fs::write(comp_dir.join("composer.phar"), b"phar").expect("phar");
    dblocal_core::toolchain::registry::write_manifest(
        &comp_dir,
        dblocal_core::toolchain::ToolId::Composer,
        "2.11.3",
        None,
        "https://example.test/composer.phar",
        &"a".repeat(64),
    )
    .expect("manifest");

    let service = ToolchainService::new(paths, home.path()).with_platform(Os::Linux, Arch::X86_64);
    let used = run(
        &service,
        ToolchainCommand::Use {
            tool: "composer".to_owned(),
            spec: "latest".to_owned(),
        },
        false,
        None,
    );
    assert_eq!(used.code, 0, "stderr: {}", used.stderr);
    assert!(
        used.stderr.contains("composer needs an active PHP"),
        "{}",
        used.stderr
    );
}

// --- fixtures ---------------------------------------------------------------

/// A fixture HTTP server; each instance binds its own ephemeral port.
struct Runtime {
    runtime: tokio::runtime::Runtime,
}

impl Runtime {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        Self { runtime }
    }

    fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// Serves `body` at every path and returns the first URL.
    fn serve(&self, body: Vec<u8>) -> String {
        let listener = self
            .block_on(async { TcpListener::bind("127.0.0.1:0").await })
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let hits = Arc::new(AtomicUsize::new(0));
        self.runtime.spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                hits.fetch_add(1, Ordering::SeqCst);
                let body = body.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = socket.read(&mut buf).await;
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                    let _ = socket.flush().await;
                });
            }
        });
        // Give the spawned accept loop a chance to start before the client asks.
        self.block_on(async { tokio::task::yield_now().await });
        format!("http://{addr}/node.tar.gz")
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // `Runtime` shuts its tasks down when dropped.
    }
}

fn sha_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn service_for(home: &Home) -> ToolchainService {
    let paths = ToolchainPaths::with_root(home.path());
    ToolchainService::new(paths, home.path())
}
