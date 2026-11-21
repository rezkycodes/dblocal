//! Minimal reproduction of a defect this suite found (W2-C).
//!
//! **Defect:** `download::extract_runtime` rejects *every* tar hard link with
//! `UnsafeArchive` (`crates/core/src/download/extract/tarball.rs`, the
//! `EntryType::Link` arm). Every shipped PostgreSQL artifact from
//! `theseus-rs/postgresql-binaries` contains hard links — 253 in the Linux
//! x86_64 18.7.0 tarball, all under `share/timezone/`, none under `bin/` or
//! `lib/`. Creating a PostgreSQL instance from the shipped catalog therefore
//! always fails at the `Extracting` stage with `DBL-2002`, so Phase 1's primary
//! engine cannot be created at all.
//!
//! The links are *in-tree*: each one points at a sibling inside the same
//! `postgresql-…/share/timezone/` directory, so there is no escape to guard
//! against. Rejecting them is stricter than zip-slip safety requires, and it is
//! what blocks PostgreSQL. An in-tree hard link is not an escape vector; the
//! unsafe cases are a link whose target is outside the extraction root, or an
//! absolute target.
//!
//! **Not patched here.** Extraction belongs to W1-C, and this task may not
//! change non-test code in `dblocal-core`.
//!
//! Run the reproduction:
//!
//! ```sh
//! DBLOCAL_E2E=1 cargo test -p dblocal-core --test service_e2e -- --ignored \
//!   --nocapture repro::hardlink
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::path::{Path, PathBuf};

use dblocal_core::download::{ExtractOptions, extract_runtime};
use dblocal_core::model::ArchiveFormat;

/// Builds a `.tar.gz` holding `root/bin/engine` plus a hard link in the same
/// directory, `root/bin/alias -> root/bin/engine`. This is exactly the shape the
/// real PostgreSQL archive repeats: a link whose target is a sibling inside the
/// extraction root.
fn fixture_with_in_tree_hard_link() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let encoder = flate2::write::GzEncoder::new(&mut bytes, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);

        let data = b"#!/bin/sh\nexit 0\n";
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o755);
        header.set_size(data.len() as u64);
        builder
            .append_data(&mut header, "root/bin/engine", &data[..])
            .expect("append the regular file");

        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Link);
        header.set_mode(0o755);
        header.set_size(0);
        header.set_link_name("root/bin/engine").expect("link name");
        builder
            .append_data(&mut header, "root/bin/alias", &[][..])
            .expect("append the hard link");

        let mut encoder = builder.into_inner().expect("finish the tar stream");
        encoder.flush().expect("flush the gzip stream");
    }
    bytes
}

fn extract_options() -> ExtractOptions {
    ExtractOptions {
        expected_binaries: vec!["bin/engine".to_owned()],
        strip_prefix: Some("root".to_owned()),
        max_total_bytes: None,
        max_entries: None,
    }
}

fn extract(
    archive: &Path,
    final_dir: &Path,
    options: &ExtractOptions,
) -> dblocal_core::Result<PathBuf> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the extraction runtime");
    let size = std::fs::metadata(archive).expect("archive metadata").len();
    runtime.block_on(extract_runtime(
        archive,
        ArchiveFormat::TarGz,
        size,
        final_dir,
        options,
    ))
}

/// The minimal case: one safe, in-tree hard link is rejected.
#[test]
#[ignore = "e2e: run with DBLOCAL_E2E=1 cargo test --test service_e2e -- --ignored repro::hardlink"]
fn repro_a_safe_in_tree_hard_link_is_rejected() {
    super::require_e2e();

    let dir = tempfile::tempdir().expect("tempdir");
    let archive = dir.path().join("in-tree-hard-link.tar.gz");
    std::fs::write(&archive, fixture_with_in_tree_hard_link()).expect("write the fixture");
    let final_dir = dir.path().join("runtime");

    let result = extract(&archive, &final_dir, &extract_options());
    match result {
        Err(err) => {
            // Characterisation of the defect: this test documents what the code
            // does today, and must start failing (or be deleted) once W1-C
            // relaxes the rule. The message spells out the trigger so nobody has
            // to reverse-engineer it from a red run.
            eprintln!(
                "DEFECT CONFIRMED: extracting `root/bin/engine` + hard link \
                 `root/bin/alias -> root/bin/engine` failed with [{}] {}",
                err.code(),
                err.user_message()
            );
            assert_eq!(
                err.code(),
                dblocal_core::error::CODE_UNSAFE_ARCHIVE,
                "the rejection must be the typed UnsafeArchive error"
            );
            assert!(
                !final_dir.exists(),
                "a rejected archive must leave no finalised runtime"
            );
        }
        Ok(_) => {
            assert!(
                final_dir.join("bin/alias").is_file(),
                "when hard links are accepted, the alias must exist"
            );
            eprintln!(
                "the extractor accepts in-tree hard links now — remove this reproduction \
                 and re-enable the PostgreSQL cases"
            );
        }
    }
}

/// The real artifact through the production extraction path, reporting the first
/// entry the extractor refuses.
#[test]
#[ignore = "e2e: run with DBLOCAL_E2E=1 cargo test --test service_e2e -- --ignored repro::hardlink"]
fn repro_the_shipped_postgres_archive_fails_extraction() {
    super::require_e2e();

    let Some(archive) = cached_archive("postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz") else {
        eprintln!("skipping: the PostgreSQL 18.7.0 archive is not cached in ~/.cache/dblocal-dev");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let final_dir = dir.path().join("runtime");
    let options = ExtractOptions {
        expected_binaries: vec![
            "bin/postgres".to_owned(),
            "bin/initdb".to_owned(),
            "bin/pg_ctl".to_owned(),
        ],
        strip_prefix: Some("postgresql-18.7.0-x86_64-unknown-linux-gnu".to_owned()),
        max_total_bytes: None,
        max_entries: None,
    };

    match extract(&archive, &final_dir, &options) {
        Err(err) => {
            eprintln!(
                "DEFECT CONFIRMED: the shipped archive {} failed extraction with [{}] {}",
                archive.display(),
                err.code(),
                err.user_message()
            );
            // The raw `Display` form carries the offending entry, which the
            // user-facing message deliberately drops.
            eprintln!("rejected entry, as the extractor named it: {err}");
            assert_eq!(err.code(), dblocal_core::error::CODE_UNSAFE_ARCHIVE);
            // The tar listing proves *which* entries are hard links, since the
            // error itself only carries the code.
            let listing = std::process::Command::new("tar")
                .arg("-tvzf")
                .arg(&archive)
                .output()
                .expect("tar -tvzf");
            let text = String::from_utf8_lossy(&listing.stdout);
            let hard_links: Vec<&str> = text.lines().filter(|line| line.starts_with('h')).collect();
            let outside_bin_lib = hard_links
                .iter()
                .filter(|line| {
                    !line.contains("/bin/") && !line.contains("/lib/") && !line.contains("/lib64/")
                })
                .count();
            eprintln!(
                "{}: {} hard links, {outside_bin_lib} of them outside bin/, lib/ and lib64/",
                archive.display(),
                hard_links.len()
            );
            for line in hard_links.iter().take(3) {
                eprintln!("  {line}");
            }
            assert!(
                !hard_links.is_empty(),
                "the archive must contain the hard links that trigger the rejection"
            );
            assert!(
                !final_dir.exists(),
                "a rejected archive must leave no finalised runtime"
            );
        }
        Ok(_) => {
            eprintln!(
                "the shipped PostgreSQL archive extracts now — the defect is fixed; \
                 re-enable the PostgreSQL e2e cases"
            );
        }
    }
}

fn cached_archive(name: &str) -> Option<PathBuf> {
    let cache = std::env::var_os("HOME").map(|home| Path::new(&home).join(".cache/dblocal-dev"))?;
    let path = cache.join(name);
    path.is_file().then_some(path)
}

#[allow(dead_code)]
fn _unused(_: &[u8]) {}
