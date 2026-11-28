//! Registry and install-scan tests.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use tempfile::tempdir;

use crate::toolchain::catalog::ToolId;
use crate::toolchain::fsops::ToolchainPaths;
use crate::toolchain::registry::{
    ActiveVersion, Registry, read_registry_at, scan_installed_at, write_manifest, write_registry_at,
};

fn paths(root: &Path) -> ToolchainPaths {
    ToolchainPaths::with_root(root)
}

#[test]
fn missing_registry_means_nothing_active() {
    let dir = tempdir().expect("tmp");
    let registry = read_registry_at(&dir.path().join("active.json"));
    assert!(registry.is_empty());
    assert!(registry.get(ToolId::Php).is_none());
}

#[test]
fn registry_round_trips() {
    let dir = tempdir().expect("tmp");
    let path = dir.path().join("active.json");
    let registry = Registry::empty()
        .with_active(ActiveVersion::new(
            ToolId::Php,
            "8.4.15",
            Some("common".into()),
        ))
        .with_active(ActiveVersion::new(ToolId::Node, "24.11.0", None));
    write_registry_at(&path, &registry).expect("write");
    let read = read_registry_at(&path);
    assert_eq!(read, registry);
    assert_eq!(read.get(ToolId::Php).unwrap().dir_leaf(), "8.4.15-common");
    assert_eq!(read.get(ToolId::Node).unwrap().dir_leaf(), "24.11.0");
}

#[test]
fn corrupt_or_unknown_registry_is_empty_not_a_crash() {
    let dir = tempdir().expect("tmp");
    let path = dir.path().join("active.json");

    std::fs::write(&path, b"{ not json").expect("write");
    assert!(read_registry_at(&path).is_empty());

    std::fs::write(
        &path,
        br#"{"schema": 99, "active": [{"tool":"php","version":"8.4.15"}]}"#,
    )
    .expect("write");
    assert!(read_registry_at(&path).is_empty());

    // Unknown tools are ignored individually; known ones survive.
    std::fs::write(
        &path,
        br#"{"schema":1,"active":[{"tool":"ruby","version":"3.3"},{"tool":"node","version":"24.11.0"}]}"#,
    )
    .expect("write");
    let registry = read_registry_at(&path);
    assert_eq!(registry.active().len(), 1);
    assert_eq!(registry.get(ToolId::Node).unwrap().version(), "24.11.0");
}

#[test]
fn unknown_fields_are_tolerated() {
    let dir = tempdir().expect("tmp");
    let path = dir.path().join("active.json");
    std::fs::write(
        &path,
        br#"{"schema":1,"future":true,"active":[{"tool":"php","version":"8.4.15","profile":"common","extra":"x"}]}"#,
    )
    .expect("write");
    let registry = read_registry_at(&path);
    assert_eq!(registry.get(ToolId::Php).unwrap().version(), "8.4.15");
}

#[test]
fn setting_a_tool_replaces_its_previous_entry() {
    let registry = Registry::empty()
        .with_active(ActiveVersion::new(ToolId::Php, "8.3.2", None))
        .with_active(ActiveVersion::new(
            ToolId::Php,
            "8.4.15",
            Some("bulk".into()),
        ));
    assert_eq!(registry.active().len(), 1);
    assert_eq!(registry.get(ToolId::Php).unwrap().version(), "8.4.15");
    assert_eq!(registry.without(ToolId::Php).active().len(), 0);
}

fn install(dir: &Path, tool: ToolId, version: &str, profile: Option<&str>) {
    let leaf = match profile {
        Some(p) => format!("{version}-{p}"),
        None => version.to_owned(),
    };
    let root = dir.join(tool.as_str()).join(leaf);
    std::fs::create_dir_all(root.join("bin")).expect("mkdir");
    std::fs::write(root.join("bin/payload"), b"x").expect("payload");
    write_manifest(
        &root,
        tool,
        version,
        profile,
        "https://e.t/x",
        &"a".repeat(64),
    )
    .expect("manifest");
}

#[test]
fn scan_finds_installed_versions_and_ignores_junk() {
    let dir = tempdir().expect("tmp");
    let toolchains = dir.path().join("toolchains");
    std::fs::create_dir_all(&toolchains).expect("mkdir");

    install(&toolchains, ToolId::Php, "8.4.15", Some("common"));
    install(&toolchains, ToolId::Php, "8.3.2", Some("common"));
    install(&toolchains, ToolId::Node, "24.11.0", None);

    // Junk that must be ignored:
    std::fs::create_dir_all(toolchains.join(".tmp/staging")).expect("staging");
    std::fs::create_dir_all(toolchains.join("php/.removing-8.1.0")).expect("removing");
    std::fs::create_dir_all(toolchains.join("php/8.2.0")).expect("no manifest");
    std::fs::write(toolchains.join("php/stray-file"), b"x").expect("stray");

    let installed = scan_installed_at(&toolchains);
    let summary: Vec<(String, String, Option<String>)> = installed
        .iter()
        .map(|v| {
            (
                v.tool().to_string(),
                v.version().to_owned(),
                v.profile().map(str::to_owned),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("node".to_owned(), "24.11.0".to_owned(), None),
            (
                "php".to_owned(),
                "8.4.15".to_owned(),
                Some("common".to_owned())
            ),
            (
                "php".to_owned(),
                "8.3.2".to_owned(),
                Some("common".to_owned())
            ),
        ]
    );
}

#[test]
fn scan_ignores_a_manifest_that_disagrees_with_its_directory() {
    let dir = tempdir().expect("tmp");
    let toolchains = dir.path().join("toolchains");
    std::fs::create_dir_all(&toolchains).expect("mkdir");
    // Directory says 8.4.15-common, manifest says 8.3.2.
    let root = toolchains.join("php/8.4.15-common");
    std::fs::create_dir_all(&root).expect("mkdir");
    write_manifest(
        &root,
        ToolId::Php,
        "8.3.2",
        Some("common"),
        "https://e.t/x",
        &"a".repeat(64),
    )
    .expect("manifest");
    assert!(scan_installed_at(&toolchains).is_empty());
}

#[test]
fn scan_reports_a_size_and_install_time() {
    let dir = tempdir().expect("tmp");
    let toolchains = dir.path().join("toolchains");
    install(&toolchains, ToolId::Composer, "2.8.4", None);
    let installed = scan_installed_at(&toolchains);
    assert_eq!(installed.len(), 1);
    assert!(installed[0].size_bytes() > 0);
    assert_eq!(installed[0].dir_leaf(), "2.8.4");
    assert!(installed[0].dir().ends_with("composer/2.8.4"));
}

#[test]
fn scan_of_a_missing_root_is_empty() {
    let dir = tempdir().expect("tmp");
    assert!(scan_installed_at(&dir.path().join("nope")).is_empty());
}

#[test]
fn write_registry_uses_the_shared_home_resolver() {
    let dir = tempdir().expect("tmp");
    let paths = paths(dir.path());
    let registry =
        Registry::empty().with_active(ActiveVersion::new(ToolId::Composer, "2.8.4", None));
    crate::toolchain::registry::write_registry(&paths, &registry).expect("write");
    assert!(paths.active_registry().exists());
    assert_eq!(
        crate::toolchain::registry::read_registry(&paths)
            .get(ToolId::Composer)
            .unwrap()
            .version(),
        "2.8.4"
    );
}
