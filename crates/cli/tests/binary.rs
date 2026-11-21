//! Real-binary smoke tests. These run the built `dblocal` executable, so they
//! cover clap wiring, exit codes and the "no service needed" paths.
//!
//! Every test points `DBLOCAL_HOME` at a temp dir so nothing touches the
//! developer's real `~/.dblocal`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;

use tempfile::TempDir;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dblocal"))
}

struct Home(TempDir);

impl Home {
    fn new() -> Self {
        Self(TempDir::new().expect("temp home"))
    }
    fn path(&self) -> &std::path::Path {
        self.0.path()
    }
}

#[test]
fn version_succeeds() {
    let out = bin().arg("--version").output().expect("run");
    assert!(out.status.success(), "status {:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("dblocal"), "version text: {text}");
}

#[test]
fn help_succeeds_and_lists_commands() {
    let out = bin().arg("--help").output().expect("run");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for cmd in [
        "list", "status", "start", "stop", "uri", "create", "delete", "logs",
    ] {
        assert!(text.contains(cmd), "help missing {cmd}: {text}");
    }
}

#[test]
fn bad_argument_exits_two() {
    let out = bin().arg("frobnicate").output().expect("run");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn missing_required_argument_exits_two() {
    let out = bin().args(["start"]).output().expect("run");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn list_over_empty_home_succeeds_with_an_empty_table() {
    let home = Home::new();
    let out = bin()
        .arg("list")
        .env("DBLOCAL_HOME", home.path())
        .env("HOME", home.path())
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("NAME"), "table header: {text}");
    assert!(!text.contains("app"), "no rows expected: {text}");
}

#[test]
fn list_json_over_empty_home_is_an_empty_array() {
    let home = Home::new();
    let out = bin()
        .args(["list", "--json"])
        .env("DBLOCAL_HOME", home.path())
        .env("HOME", home.path())
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.trim(), "[]");
}

#[test]
fn status_of_a_missing_instance_exits_three() {
    let home = Home::new();
    let out = bin()
        .args(["status", "does-not-exist"])
        .env("DBLOCAL_HOME", home.path())
        .env("HOME", home.path())
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(3),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
