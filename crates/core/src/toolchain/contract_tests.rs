//! Contract test against the real `catalog/toolchains-v1.json`.
//!
//! The catalog part generates and owns that file; the core part must be able to
//! read whatever it produces. This test runs only when the file exists in the
//! worktree, and skips cleanly (prints a note and returns) when it does not, so a
//! core-only checkout stays green. It is a plain `#[test]` (run by `cargo test`),
//! not an ignored e2e, because reading a checked-in JSON file touches no network
//! and no engine.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use crate::platform::{Arch, Os};
use crate::toolchain::catalog::{ToolId, parse_catalog};

/// Path to the generated catalog: `<workspace>/catalog/toolchains-v1.json`.
fn catalog_path() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `crates/core`; the workspace root is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("catalog")
        .join("toolchains-v1.json")
}

#[test]
fn real_catalog_file_parses_when_present() {
    let path = catalog_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!(
            "skipping: {} is not present yet (the catalog part generates it)",
            path.display()
        );
        return;
    };

    let (catalog, skips) = parse_catalog(&text).expect("the real catalog must parse");
    assert!(
        !catalog.tools().is_empty(),
        "the real catalog must describe at least one tool"
    );

    // Every known tool that is present must offer at least one version, and every
    // version must yield an artifact for at least one platform we ship.
    for tool in ToolId::all() {
        let Some(entry) = catalog.tool(tool) else {
            continue;
        };
        assert!(!entry.versions().is_empty(), "{tool} has no versions");
        for version in entry.versions() {
            assert!(
                !version.artifacts().is_empty(),
                "{tool} {} has no artifacts",
                version.version()
            );
            for artifact in version.artifacts() {
                assert_eq!(artifact.sha256().len(), 64, "{tool} {} sha256", version.version());
                assert!(
                    artifact.url().starts_with("https://"),
                    "{tool} {} url must be https",
                    version.version()
                );
            }
        }
        // At least one version is installable on the host we test on.
        let installable = entry.versions().iter().any(|version| {
            version
                .select_artifact(Os::current(), Arch::current(), None)
                .is_ok()
                || version
                    .select_artifact(
                        Os::current(),
                        Arch::current(),
                        Some(crate::toolchain::catalog::Profile::Common),
                    )
                    .is_ok()
        });
        assert!(installable, "{tool} has no version installable on this host");
    }

    eprintln!(
        "parsed {} tools, {} entries skipped",
        catalog.tools().len(),
        skips.total()
    );
}
