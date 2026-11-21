//! Regression tests for D-1 (W2-F): safe in-tree tar hard links.
//!
//! The shipped PostgreSQL archives contain in-tree hard links (253 in the
//! Linux x86_64 17.12.0 tarball, all under `share/timezone/`). The extractor
//! must accept them when they satisfy the containment rules documented in
//! `docs/architecture/download.md`, and still reject unsafe hard links.
//!
//! Run:
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

/// A safe, in-tree hard link must extract and preserve content.
///
/// This used to be D-1's minimal reproduction (it asserted rejection). It is now
/// a regression test: the extractor accepts the link and both names resolve to
/// the same regular-file content.
#[test]
#[ignore = "e2e: run with DBLOCAL_E2E=1 cargo test --test service_e2e -- --ignored repro::hardlink"]
fn repro_a_safe_in_tree_hard_link_is_accepted() {
    super::require_e2e();

    let dir = tempfile::tempdir().expect("tempdir");
    let archive = dir.path().join("in-tree-hard-link.tar.gz");
    std::fs::write(&archive, fixture_with_in_tree_hard_link()).expect("write the fixture");
    let final_dir = dir.path().join("runtime");

    extract(&archive, &final_dir, &extract_options()).expect("safe in-tree hard link must extract");
    assert_eq!(
        std::fs::read(final_dir.join("bin/engine")).expect("engine"),
        b"#!/bin/sh\nexit 0\n"
    );
    assert_eq!(
        std::fs::read(final_dir.join("bin/alias")).expect("alias"),
        b"#!/bin/sh\nexit 0\n",
        "hard link alias must carry the same content"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let a = std::fs::metadata(final_dir.join("bin/engine")).expect("meta a");
        let b = std::fs::metadata(final_dir.join("bin/alias")).expect("meta b");
        assert_eq!(
            a.ino(),
            b.ino(),
            "safe hard link should share inode on Unix"
        );
    }
}

/// The shipped PostgreSQL artifact must extract through the production path.
///
/// This used to reproduce D-1 by expecting `CODE_UNSAFE_ARCHIVE`. It is now a
/// regression test: the real archive extracts, the expected binaries exist, and
/// hard-linked timezone files remain readable.
#[test]
#[ignore = "e2e: run with DBLOCAL_E2E=1 cargo test --test service_e2e -- --ignored repro::hardlink"]
fn repro_the_shipped_postgres_archive_extracts() {
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

    extract(&archive, &final_dir, &options).expect("shipped PostgreSQL archive must extract");
    for bin in ["bin/postgres", "bin/initdb", "bin/pg_ctl"] {
        let path = final_dir.join(bin);
        assert!(
            path.is_file(),
            "expected binary missing: {}",
            path.display()
        );
    }

    // Spot-check a known hard-linked timezone pair from the shipped archive.
    let a = final_dir.join("share/postgresql/timezone/America/Santa_Isabel");
    let b = final_dir.join("share/postgresql/timezone/America/Tijuana");
    if a.exists() && b.exists() {
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                std::fs::metadata(&a).unwrap().ino(),
                std::fs::metadata(&b).unwrap().ino(),
                "timezone hard link should share inode"
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
