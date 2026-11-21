//! `tar` extraction for `.tar.gz` and `.tar.xz` archives.

use std::io::Read;
use std::path::Path;

use crate::{DblError, Result};

#[cfg(unix)]
use super::fsops::apply_unix_mode;
use super::fsops::{create_hard_link_with, create_symlink, write_regular_file};
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

fn validate_hard_link_target(
    staging: &Path,
    target: &Path,
    strip_prefix: Option<&str>,
    raw_name: &str,
    regular_files: &std::collections::HashSet<std::path::PathBuf>,
) -> Result<std::path::PathBuf> {
    let target_name = target.to_string_lossy().into_owned();
    let rel = match stripped_relative_path(target, strip_prefix, &target_name)? {
        Some(p) => p,
        None => {
            return Err(DblError::UnsafeArchive {
                entry: format!("{raw_name}: hard link target is the stripped prefix"),
            });
        }
    };
    // Ensure the resolved target stays inside the staging root. safe_join also
    // performs lexical containment checks.
    let _ = safe_join(staging, &rel, &target_name)?;
    if !regular_files.contains(&rel) {
        return Err(DblError::UnsafeArchive {
            entry: format!(
                "{raw_name}: hard link target '{}' was not extracted earlier as a regular file",
                target.display()
            ),
        });
    }
    let target_dest = staging.join(&rel);
    let meta = std::fs::symlink_metadata(&target_dest).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            DblError::UnsafeArchive {
                entry: format!(
                    "{raw_name}: hard link target '{}' does not exist on disk",
                    target.display()
                ),
            }
        } else {
            DblError::Io { source }
        }
    })?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return Err(DblError::UnsafeArchive {
            entry: format!(
                "{raw_name}: hard link target '{}' is not a regular file",
                target.display()
            ),
        });
    }
    Ok(rel)
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

        // Hard links are validated below; they may only target a regular file
        // already extracted inside the staging root.
        let is_hard_link = matches!(entry_type, tar::EntryType::Link);
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

        if is_hard_link {
            let target = entry
                .link_name()
                .map_err(|source| DblError::Io { source })?;
            let target = target.ok_or_else(|| DblError::UnsafeArchive {
                entry: format!("{raw_name}: hard link has no target"),
            })?;
            let target_rel = validate_hard_link_target(
                staging,
                &target,
                strip_prefix,
                &raw_name,
                &state.regular_files,
            )?;
            let target_dest = staging.join(&target_rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|source| DblError::Io { source })?;
                state.seen_dirs.insert(parent.to_path_buf());
            }
            create_hard_link_with(&target_dest, &dest, caps, &mut state, &raw_name, |t, l| {
                std::fs::hard_link(t, l)
            })?;
            state.regular_files.insert(rel);
            continue;
        }

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
                state.regular_files.insert(rel);
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

#[cfg(test)]
mod tests {
    //! Layer-pinning unit tests. The hard-link defences are defence in depth, so
    //! some are only reachable when a later layer is bypassed. These call
    //! `validate_hard_link_target` directly so each layer has one test that only
    //! that layer can satisfy.

    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::*;

    #[cfg(unix)]
    #[test]
    fn on_disk_layer_rejects_a_target_recorded_as_a_file_but_symlinked_on_disk() {
        // Pins the on-disk `symlink_metadata` layer (M3): the extracted set says
        // this relative path is a regular file, but the file on disk is a
        // symlink. Only the on-disk type check can stop this, so removing it
        // (M3) makes this test fail where the earlier set check passes.
        let dir = tempfile::tempdir().expect("tmp");
        let staging = dir.path();
        std::fs::write(staging.join("real"), b"x").expect("real file");
        std::os::unix::fs::symlink("real", staging.join("alias")).expect("symlink");

        let mut regular_files = HashSet::new();
        regular_files.insert(PathBuf::from("alias"));

        let err = validate_hard_link_target(
            staging,
            Path::new("root/alias"),
            Some("root"),
            "root/h",
            &regular_files,
        )
        .expect_err("an on-disk symlink must be refused even if the set claims a file");
        assert!(
            matches!(&err, DblError::UnsafeArchive { entry } if entry.contains("not a regular file")),
            "expected the on-disk layer to reject, got {err:?}"
        );
    }

    #[test]
    fn lexical_layer_rejects_a_parent_escape_before_other_checks() {
        // Pins the lexical/containment layer (M1). `stripped_relative_path`
        // normalises and rejects `..`, so `safe_join` in this function is a
        // redundant second lexical guard for this input; the assertion is on the
        // rejection, whichever of the two normalisers catches it first.
        let dir = tempfile::tempdir().expect("tmp");
        let regular_files: HashSet<PathBuf> = HashSet::new();
        let err = validate_hard_link_target(
            dir.path(),
            Path::new("root/../outside"),
            Some("root"),
            "root/h",
            &regular_files,
        )
        .expect_err("a parent escape must be refused");
        assert!(matches!(err, DblError::UnsafeArchive { .. }), "{err:?}");
    }
}
