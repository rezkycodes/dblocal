//! `zip` extraction (deflate only).
//!
//! Zip encodes symlinks as entries whose Unix mode marks `S_IFLNK` with the
//! target stored as file contents; those are validated exactly like tar
//! symlinks. Hard links do not exist in the zip format.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::{DblError, Result};

#[cfg(unix)]
use super::fsops::apply_unix_mode;
use super::fsops::{create_symlink, write_regular_file};
use super::path::{ensure_parent_safe, safe_join, stripped_relative_path, validate_symlink_target};
use super::{ExtractCaps, ExtractState};

pub(super) fn extract_zip(
    archive_path: &Path,
    staging: &Path,
    strip_prefix: Option<&str>,
    caps: &ExtractCaps,
) -> Result<()> {
    let file = std::fs::File::open(archive_path).map_err(|source| DblError::Io { source })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| DblError::InvalidInput {
        field: "archive",
        reason: format!("invalid zip archive: {e}"),
    })?;

    let mut state = ExtractState::default();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| DblError::InvalidInput {
            field: "archive",
            reason: format!("cannot read zip entry {i}: {e}"),
        })?;
        state.count_entry(caps)?;

        let raw_name = entry
            .name()
            .map_err(|e| DblError::InvalidInput {
                field: "archive",
                reason: format!("zip entry name is not valid UTF-8: {e}"),
            })?
            .into_owned();

        // Zip stores symlinks as entries whose Unix mode marks `S_IFLNK`, with
        // the target stored as the entry body. They are validated below exactly
        // like tar symlinks.
        let name_path = Path::new(&raw_name);
        let rel = match stripped_relative_path(name_path, strip_prefix, &raw_name)? {
            Some(p) => p,
            None => {
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

        if entry.is_dir() {
            std::fs::create_dir_all(&dest).map_err(|source| DblError::Io { source })?;
            state.seen_dirs.insert(dest.clone());
            continue;
        }

        if entry.is_symlink() {
            // Read symlink target (bounded).
            let mut target_bytes = Vec::new();
            let mut limited = (&mut entry).take(caps.max_total_bytes.saturating_add(1));
            limited
                .read_to_end(&mut target_bytes)
                .map_err(|source| DblError::Io { source })?;
            let target = PathBuf::from(String::from_utf8_lossy(&target_bytes).as_ref());
            validate_symlink_target(staging, &dest, &target, &raw_name, &state.seen_dirs)?;
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|source| DblError::Io { source })?;
                state.seen_dirs.insert(parent.to_path_buf());
            }
            create_symlink(&target, &dest)?;
            continue;
        }

        // Regular file.
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|source| DblError::Io { source })?;
            state.seen_dirs.insert(parent.to_path_buf());
        }
        let unix_mode = entry.unix_mode();
        write_regular_file(&mut entry, &dest, caps, &mut state, &raw_name)?;
        #[cfg(unix)]
        if let Some(m) = unix_mode {
            apply_unix_mode(&dest, m)?;
        }
        #[cfg(not(unix))]
        let _ = unix_mode;
    }

    if strip_prefix.is_some() && !state.stripped_any {
        return Err(DblError::InvalidInput {
            field: "strip_prefix",
            reason: "archive does not contain the expected top-level directory".to_owned(),
        });
    }
    Ok(())
}
