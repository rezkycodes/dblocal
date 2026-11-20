//! Archive entry path validation.
//!
//! Every entry name is normalised and checked before it touches the filesystem:
//! absolute paths, Windows drive/UNC prefixes, `..`, NUL bytes, and paths that
//! would land outside the extraction root are all rejected. Stripping of the
//! catalog `strip_prefix` also happens here so the two concerns stay in one
//! place.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use crate::{DblError, Result};

/// Strips `strip_prefix` from `path`, returning the relative remainder.
///
/// Returns `Ok(None)` when the entry is the prefix itself (or its root marker)
/// and should be skipped. Errors if `strip_prefix` is set but the entry does not
/// start with it (caller tracks whether any entry matched to emit the required
/// "archive does not have that top-level directory" failure at the end — except
/// for entries that clearly belong to a different root, which we reject early).
pub(super) fn stripped_relative_path(
    path: &Path,
    strip_prefix: Option<&str>,
    raw_name: &str,
) -> Result<Option<PathBuf>> {
    let normalized = normalize_archive_path(path, raw_name)?;
    let Some(prefix) = strip_prefix else {
        return Ok(Some(normalized));
    };
    let prefix_norm = normalize_prefix(prefix);
    if prefix_norm.as_os_str().is_empty() {
        return Ok(Some(normalized));
    }

    // Match by components so "foo" does not match "foobar".
    let prefix_components: Vec<Component<'_>> = Path::new(&prefix_norm).components().collect();
    let path_components: Vec<Component<'_>> = normalized.components().collect();

    if path_components.len() < prefix_components.len() {
        // Cannot be under prefix; if prefix required, this is a mismatch entry.
        return Err(strip_prefix_mismatch(raw_name, prefix));
    }
    for (a, b) in prefix_components.iter().zip(path_components.iter()) {
        if a != b {
            return Err(strip_prefix_mismatch(raw_name, prefix));
        }
    }
    if path_components.len() == prefix_components.len() {
        // The prefix directory entry itself.
        return Ok(None);
    }
    let mut out = PathBuf::new();
    for c in &path_components[prefix_components.len()..] {
        out.push(c.as_os_str());
    }
    if out.as_os_str().is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

fn strip_prefix_mismatch(entry: &str, expected: &str) -> DblError {
    DblError::InvalidInput {
        field: "strip_prefix",
        reason: format!(
            "archive entry '{entry}' is not under expected top-level directory '{expected}'"
        ),
    }
}

/// Whether `raw` starts with a Windows drive designator such as `C:`.
fn has_windows_drive_prefix(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn normalize_prefix(prefix: &str) -> PathBuf {
    let trimmed = prefix.trim_end_matches(['/', '\\']);
    PathBuf::from(trimmed)
}

/// Normalises and validates a raw archive member path.
///
/// Rejects absolute paths, Windows drive/UNC prefixes, `..`, and NULs. Returns
/// a clean relative path made of `Normal` components only.
fn normalize_archive_path(path: &Path, raw_name: &str) -> Result<PathBuf> {
    if raw_name.contains('\0') {
        return Err(DblError::UnsafeArchive {
            entry: format!("{raw_name}: path contains NUL"),
        });
    }
    // Windows drive letters, UNC prefixes and backslash separators are rejected
    // on every host so an archive built on one OS cannot smuggle an escape on
    // another. `\` is treated as `/` first, matching what Windows extractors do.
    let probe = raw_name.replace('\\', "/");
    if has_windows_drive_prefix(&probe) || probe.starts_with("//") {
        return Err(DblError::UnsafeArchive {
            entry: format!("{raw_name}: Windows drive or UNC paths are not allowed"),
        });
    }
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(s) => {
                if s.as_encoded_bytes().is_empty() {
                    continue;
                }
                out.push(s);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(DblError::UnsafeArchive {
                    entry: format!("{raw_name}: path traversal ('..') is not allowed"),
                });
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(DblError::UnsafeArchive {
                    entry: format!("{raw_name}: absolute paths are not allowed"),
                });
            }
        }
    }
    if out.as_os_str().is_empty() {
        // e.g. "." or an empty name — treat as skippable root marker by caller.
        Ok(PathBuf::new())
    } else {
        Ok(out)
    }
}

/// Joins `staging/rel` ensuring the result stays within `staging` lexically.
pub(super) fn safe_join(staging: &Path, rel: &Path, raw_name: &str) -> Result<PathBuf> {
    if rel.as_os_str().is_empty() {
        return Err(DblError::UnsafeArchive {
            entry: format!("{raw_name}: empty path after stripping"),
        });
    }
    let dest = staging.join(rel);
    // Lexical containment check.
    let mut norm_dest = PathBuf::new();
    for c in dest.components() {
        match c {
            Component::ParentDir => {
                norm_dest.pop();
            }
            Component::CurDir => {}
            other => norm_dest.push(other.as_os_str()),
        }
    }
    let mut norm_staging = PathBuf::new();
    for c in staging.components() {
        match c {
            Component::ParentDir => {
                norm_staging.pop();
            }
            Component::CurDir => {}
            other => norm_staging.push(other.as_os_str()),
        }
    }
    if !norm_dest.starts_with(&norm_staging) {
        return Err(DblError::UnsafeArchive {
            entry: format!("{raw_name}: path escapes destination"),
        });
    }
    Ok(dest)
}

/// Ensures no existing component on the path from `staging` to `dest`'s parent
/// is a symlink, which would allow an escape even when the lexical path looks
/// safe. Allowed symlinks are ones we ourselves are about to create only as the
/// leaf entry (handled separately) — any pre-existing symlink in an ancestor
/// directory within staging is rejected.
pub(super) fn ensure_parent_safe(
    staging: &Path,
    dest: &Path,
    _seen_dirs: &mut HashSet<PathBuf>,
    raw_name: &str,
) -> Result<()> {
    let Some(parent) = dest.parent() else {
        return Ok(());
    };
    // Walk existing components under staging and reject symlinks.
    let rel = match parent.strip_prefix(staging) {
        Ok(r) => r.to_path_buf(),
        Err(_) => {
            return Err(DblError::UnsafeArchive {
                entry: format!("{raw_name}: escapes staging root"),
            });
        }
    };
    let mut cur = staging.to_path_buf();
    for comp in rel.components() {
        cur.push(comp.as_os_str());
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(DblError::UnsafeArchive {
                        entry: format!(
                            "{raw_name}: would traverse through symlink '{}'",
                            cur.display()
                        ),
                    });
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(source) => return Err(DblError::Io { source }),
        }
    }
    Ok(())
}

/// Validates that a symlink at `link_path` with target `target` resolves inside
/// `staging`. Both relative and absolute-looking targets are resolved relative to
/// the link's parent within staging; the final resolved path must remain inside.
pub(super) fn validate_symlink_target(
    staging: &Path,
    link_path: &Path,
    target: &Path,
    raw_name: &str,
    _seen_dirs: &HashSet<PathBuf>,
) -> Result<()> {
    // Absolute symlink targets are never safe in a relocatable runtime.
    if target.is_absolute() || matches!(target.components().next(), Some(Component::Prefix(_))) {
        return Err(DblError::UnsafeArchive {
            entry: format!(
                "{raw_name}: absolute symlink target '{}' is not allowed",
                target.display()
            ),
        });
    }
    let link_parent = link_path.parent().unwrap_or(staging);
    let resolved = lexical_resolve(link_parent, target);
    let staging_norm = normalize_lexical(staging);
    if !resolved.starts_with(&staging_norm) {
        return Err(DblError::UnsafeArchive {
            entry: format!(
                "{raw_name}: symlink target '{}' escapes extraction root",
                target.display()
            ),
        });
    }
    Ok(())
}

fn lexical_resolve(base: &Path, rel: &Path) -> PathBuf {
    let mut out = normalize_lexical(base);
    for c in rel.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            Component::Normal(s) => out.push(s),
            Component::RootDir | Component::Prefix(_) => {
                // Should have been rejected earlier; keep it absolute-ish so the
                // starts_with(staging) check fails.
                out.push(c.as_os_str());
            }
        }
    }
    out
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn reject_nul() {
        let err = normalize_archive_path(Path::new("a/b"), "a/b\0c").unwrap_err();
        assert!(matches!(err, DblError::UnsafeArchive { .. }), "{err:?}");
    }

    #[test]
    fn nul_byte_in_name_is_rejected() {
        reject_nul();
    }

    #[test]
    fn dot_components_are_dropped() {
        let normalised = normalize_archive_path(Path::new("./a/./b"), "./a/./b").expect("ok");
        assert_eq!(normalised, PathBuf::from("a/b"));
        // A bare "." normalises to an empty path (skippable root marker).
        assert!(
            normalize_archive_path(Path::new("."), ".")
                .expect("ok")
                .as_os_str()
                .is_empty()
        );
    }

    #[test]
    fn escape_through_parent_directory_is_rejected() {
        let err = normalize_archive_path(Path::new("a/../../b"), "a/../../b").unwrap_err();
        assert!(matches!(err, DblError::UnsafeArchive { .. }), "{err:?}");
    }

    #[test]
    fn windows_prefixes_and_unc_paths_are_rejected() {
        for raw in [
            "C:/x",
            "c:\\\\x",
            "//server/share/x",
            "\\\\\\\\server\\\\share\\\\x",
        ] {
            let err = normalize_archive_path(Path::new(raw), raw).unwrap_err();
            assert!(
                matches!(err, DblError::UnsafeArchive { .. }),
                "{raw}: {err:?}"
            );
        }
    }

    #[test]
    fn strip_prefix_returns_none_for_the_prefix_entry_itself() {
        let got = stripped_relative_path(Path::new("root"), Some("root"), "root").expect("ok");
        assert_eq!(got, None);
        let got = stripped_relative_path(Path::new("root/"), Some("root/"), "root/").expect("ok");
        assert_eq!(got, None);
    }

    #[test]
    fn strip_prefix_rejects_a_shorter_entry() {
        let err = stripped_relative_path(Path::new("root"), Some("root/sub"), "root").unwrap_err();
        assert!(
            matches!(
                err,
                DblError::InvalidInput {
                    field: "strip_prefix",
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn strip_prefix_requires_component_alignment() {
        let err =
            stripped_relative_path(Path::new("rootx/a"), Some("root"), "rootx/a").unwrap_err();
        assert!(matches!(err, DblError::InvalidInput { .. }), "{err:?}");
    }

    #[test]
    fn strip_prefix_is_a_noop_when_empty() {
        let got = stripped_relative_path(Path::new("a/b"), Some(""), "a/b").expect("ok");
        assert_eq!(got, Some(PathBuf::from("a/b")));
        let got = stripped_relative_path(Path::new("a/b"), None, "a/b").expect("ok");
        assert_eq!(got, Some(PathBuf::from("a/b")));
    }

    #[test]
    fn safe_join_rejects_an_empty_relative_path() {
        let err = safe_join(Path::new("/tmp/stage"), Path::new(""), "x").unwrap_err();
        assert!(matches!(err, DblError::UnsafeArchive { .. }), "{err:?}");
    }

    #[test]
    fn safe_join_normalises_cur_and_parent_components() {
        let joined = safe_join(Path::new("/tmp/stage"), Path::new("a/./b"), "a/./b").expect("ok");
        assert_eq!(joined, Path::new("/tmp/stage/a/b"));
        let err = safe_join(
            Path::new("/tmp/stage"),
            Path::new("../outside"),
            "../outside",
        )
        .unwrap_err();
        assert!(matches!(err, DblError::UnsafeArchive { .. }), "{err:?}");
    }

    #[test]
    fn empty_prefix_after_trimming_is_a_noop() {
        assert!(normalize_prefix("/").as_os_str().is_empty());
        assert_eq!(normalize_prefix("root/"), PathBuf::from("root"));
        assert_eq!(normalize_prefix("root\\\\"), PathBuf::from("root"));
    }

    #[test]
    fn absolute_symlink_targets_are_rejected() {
        let err = validate_symlink_target(
            Path::new("/tmp/stage"),
            Path::new("/tmp/stage/bin/x"),
            Path::new("/etc/passwd"),
            "bin/x",
            &HashSet::new(),
        )
        .unwrap_err();
        assert!(matches!(err, DblError::UnsafeArchive { .. }), "{err:?}");
    }

    #[test]
    fn relative_symlink_that_stays_inside_is_allowed() {
        validate_symlink_target(
            Path::new("/tmp/stage"),
            Path::new("/tmp/stage/bin/x"),
            Path::new("y"),
            "bin/x",
            &HashSet::new(),
        )
        .expect("inside");
        validate_symlink_target(
            Path::new("/tmp/stage"),
            Path::new("/tmp/stage/bin/x"),
            Path::new("../lib/y"),
            "bin/x",
            &HashSet::new(),
        )
        .expect("parent but still inside");
    }

    #[test]
    fn relative_symlink_that_walks_out_is_rejected() {
        let err = validate_symlink_target(
            Path::new("/tmp/stage"),
            Path::new("/tmp/stage/bin/x"),
            Path::new("../../y"),
            "bin/x",
            &HashSet::new(),
        )
        .unwrap_err();
        assert!(matches!(err, DblError::UnsafeArchive { .. }), "{err:?}");
    }
}
