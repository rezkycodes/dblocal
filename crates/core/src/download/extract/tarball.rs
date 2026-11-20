//! `tar` extraction for `.tar.gz` and `.tar.xz` archives.

use std::io::Read;
use std::path::Path;

use crate::{DblError, Result};

#[cfg(unix)]
use super::fsops::apply_unix_mode;
use super::fsops::{create_symlink, write_regular_file};
use super::path::{ensure_parent_safe, safe_join, stripped_relative_path, validate_symlink_target};
use super::{ExtractCaps, ExtractState};

pub(super) fn extract_tar_gz(
    archive_path: &Path,
    staging: &Path,
    strip_prefix: Option<&str>,
    caps: &ExtractCaps,
) -> Result<()> {
    let file = std::fs::File::open(archive_path).map_err(|source| DblError::Io { source })?;
    let dec = flate2::read::GzDecoder::new(file);
    extract_tar(dec, staging, strip_prefix, caps)
}

pub(super) fn extract_tar_xz(
    archive_path: &Path,
    staging: &Path,
    strip_prefix: Option<&str>,
    caps: &ExtractCaps,
) -> Result<()> {
    let file = std::fs::File::open(archive_path).map_err(|source| DblError::Io { source })?;
    let dec = liblzma::read::XzDecoder::new(file);
    extract_tar(dec, staging, strip_prefix, caps)
}

fn extract_tar<R: Read>(
    reader: R,
    staging: &Path,
    strip_prefix: Option<&str>,
    caps: &ExtractCaps,
) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    // Do not let tar follow its own defaults blindly; we validate every entry.
    let entries = archive
        .entries()
        .map_err(|source| DblError::Io { source })?;
    let mut state = ExtractState::default();

    for entry in entries {
        let mut entry = entry.map_err(|source| DblError::Io { source })?;
        state.count_entry(caps)?;

        let header = entry.header();
        let entry_type = header.entry_type();

        // PAX/GNU metadata records (`x`, `g`, `L`, `K`) are consumed by the tar
        // reader itself. They carry empty or truncated names and must never be
        // measured against the strip prefix or written to disk.
        if matches!(
            entry_type,
            tar::EntryType::GNULongName
                | tar::EntryType::GNULongLink
                | tar::EntryType::XHeader
                | tar::EntryType::XGlobalHeader
        ) {
            continue;
        }

        let raw_path = entry
            .path()
            .map_err(|source| DblError::Io { source })?
            .into_owned();
        let raw_name = raw_path.to_string_lossy().into_owned();

        // Reject hard links outright.
        if matches!(entry_type, tar::EntryType::Link) {
            return Err(DblError::UnsafeArchive {
                entry: format!("{raw_name}: hard links are not allowed"),
            });
        }
        // Reject special device/fifo entries.
        if matches!(
            entry_type,
            tar::EntryType::Char | tar::EntryType::Block | tar::EntryType::Fifo
        ) {
            return Err(DblError::UnsafeArchive {
                entry: format!("{raw_name}: special device entries are not allowed"),
            });
        }

        let rel = match stripped_relative_path(&raw_path, strip_prefix, &raw_name)? {
            Some(p) => p,
            None => {
                // Entry is (part of) the stripped prefix itself; skip it but record.
                if strip_prefix.is_some() {
                    state.stripped_any = true;
                }
                continue;
            }
        };
        if strip_prefix.is_some() {
            state.stripped_any = true;
        }

        let dest = safe_join(staging, &rel, &raw_name)?;
        ensure_parent_safe(staging, &dest, &mut state.seen_dirs, &raw_name)?;

        match entry_type {
            tar::EntryType::Directory => {
                std::fs::create_dir_all(&dest).map_err(|source| DblError::Io { source })?;
                state.seen_dirs.insert(dest.clone());
            }
            tar::EntryType::Regular | tar::EntryType::GNUSparse | tar::EntryType::Continuous => {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|source| DblError::Io { source })?;
                    state.seen_dirs.insert(parent.to_path_buf());
                }
                let mode = header.mode().ok();
                write_regular_file(&mut entry, &dest, caps, &mut state, &raw_name)?;
                #[cfg(unix)]
                if let Some(m) = mode {
                    apply_unix_mode(&dest, m)?;
                }
                #[cfg(not(unix))]
                let _ = mode;
            }
            tar::EntryType::Symlink => {
                let target = entry
                    .link_name()
                    .map_err(|source| DblError::Io { source })?;
                let target = target.ok_or_else(|| DblError::UnsafeArchive {
                    entry: format!("{raw_name}: symlink has no target"),
                })?;
                validate_symlink_target(staging, &dest, &target, &raw_name, &state.seen_dirs)?;
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|source| DblError::Io { source })?;
                    state.seen_dirs.insert(parent.to_path_buf());
                }
                create_symlink(&target, &dest)?;
            }
            _ => {
                return Err(DblError::UnsafeArchive {
                    entry: format!("{raw_name}: unsupported tar entry type"),
                });
            }
        }
    }

    if strip_prefix.is_some() && !state.stripped_any {
        return Err(DblError::InvalidInput {
            field: "strip_prefix",
            reason: "archive does not contain the expected top-level directory".to_owned(),
        });
    }
    Ok(())
}
