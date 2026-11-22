//! File materialisation for extraction: regular files, Unix modes, symlinks.

use std::io::Read;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::{DblError, Result};

use super::{ExtractCaps, ExtractState};

pub(super) fn write_regular_file<R: Read>(
    reader: &mut R,
    dest: &Path,
    caps: &ExtractCaps,
    state: &mut ExtractState,
    raw_name: &str,
) -> Result<u64> {
    let mut file = std::fs::File::create(dest).map_err(|source| DblError::Io { source })?;
    let mut buf = [0u8; 64 * 1024];
    let mut written: u64 = 0;
    loop {
        let n = reader.read(&mut buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::InvalidData {
                DblError::InvalidInput {
                    field: "archive",
                    reason: format!("corrupt or truncated entry '{raw_name}': {e}"),
                }
            } else {
                DblError::Io { source: e }
            }
        })?;
        if n == 0 {
            break;
        }
        written = written.saturating_add(n as u64);
        state.add_bytes(n as u64, caps, raw_name)?;
        std::io::Write::write_all(&mut file, &buf[..n])
            .map_err(|source| DblError::Io { source })?;
    }
    file.sync_all().map_err(|source| DblError::Io { source })?;
    Ok(written)
}

#[cfg(unix)]
pub(super) fn apply_unix_mode(path: &Path, mode: u32) -> Result<()> {
    // Preserve permission bits as stored; strip file-type bits.
    let perms = std::fs::Permissions::from_mode(mode & 0o7777);
    std::fs::set_permissions(path, perms).map_err(|source| DblError::Io { source })
}

#[cfg(unix)]
pub(super) fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|source| DblError::Io { source })
}

/// Creates a hard link `link -> target`, both inside the staging root.
///
/// If the OS or filesystem refuses hard links (for example a Windows volume
/// without hard-link support, or a cross-device error which cannot occur inside
/// one staging root but is handled defensively), falls back to copying the
/// target's bytes into `link`. The fallback reads the already-extracted target;
/// callers must prove the target is a regular file first, so the copy never
/// follows a symlink.
///
/// `link_fn` is injectable for tests so the fallback path can be exercised
/// without depending on the OS refusing hard links.
pub(super) fn create_hard_link_with<F>(
    target: &Path,
    link: &Path,
    caps: &ExtractCaps,
    state: &mut ExtractState,
    raw_name: &str,
    link_fn: F,
) -> Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    if link.exists() || std::fs::symlink_metadata(link).is_ok() {
        return Err(DblError::UnsafeArchive {
            entry: format!("{raw_name}: hard link destination already exists"),
        });
    }
    match link_fn(target, link) {
        Ok(()) => Ok(()),
        Err(err) if hard_link_fallback(&err) => {
            copy_target_as_fallback(target, link, caps, state, raw_name)
        }
        Err(source) => Err(DblError::Io { source }),
    }
}

/// Whether a hard-link failure should fall back to copying bytes.
///
/// We only fall back for errors that mean hard links are impossible on this
/// filesystem/OS, not for transient I/O failures. `AlreadyExists`/EEXIST must
/// NOT fall back: it indicates a destination collision and falling back would
/// risk overwriting or hiding it.
fn hard_link_fallback(err: &std::io::Error) -> bool {
    if matches!(err.kind(), std::io::ErrorKind::AlreadyExists) {
        return false;
    }
    if matches!(
        err.kind(),
        std::io::ErrorKind::Unsupported | std::io::ErrorKind::CrossesDevices
    ) {
        return true;
    }
    hard_link_fallback_os(err)
}

#[cfg(unix)]
fn hard_link_fallback_os(err: &std::io::Error) -> bool {
    // EXDEV (18): cross-device link. EMLINK (31): too many links. ENOSYS (38):
    // filesystem does not support link. EPERM (1) can mean fs doesn't allow hard
    // links in some configurations, but on Linux it also means privilege; we do
    // not fall back on EPERM/EACCES to avoid masking permission problems.
    // EEXIST (17) is explicitly excluded above.
    matches!(err.raw_os_error(), Some(18) | Some(31) | Some(38))
}

#[cfg(windows)]
fn hard_link_fallback_os(err: &std::io::Error) -> bool {
    // ERROR_INVALID_FUNCTION (1): filesystem does not support hard links.
    // ERROR_NOT_SUPPORTED (50): operation not supported.
    // ERROR_PRIVILEGE_NOT_HELD (1314): can occur without privilege; treat as
    // unsupported for hard links and fall back to copy.
    // ERROR_ALREADY_EXISTS (183) / ERROR_FILE_EXISTS (80) are excluded above by
    // AlreadyExists kind; raw-code guard here is defensive.
    matches!(err.raw_os_error(), Some(1) | Some(50) | Some(1314))
        && !matches!(err.raw_os_error(), Some(80) | Some(183))
}

#[cfg(not(any(unix, windows)))]
fn hard_link_fallback_os(_err: &std::io::Error) -> bool {
    false
}

fn copy_target_as_fallback(
    target: &Path,
    link: &Path,
    caps: &ExtractCaps,
    state: &mut ExtractState,
    raw_name: &str,
) -> Result<()> {
    // Read through a regular File; open follows the final target but callers
    // have already verified target is a regular non-symlink file.
    let mut src = std::fs::File::open(target).map_err(|source| DblError::Io { source })?;
    match write_regular_file(&mut src, link, caps, state, raw_name) {
        Ok(written) => {
            tracing::debug!(
                target = %target.display(),
                link = %link.display(),
                bytes = written,
                "hard link fell back to byte copy"
            );
            Ok(())
        }
        Err(err) => {
            let _ = std::fs::remove_file(link);
            Err(err)
        }
    }
}

#[cfg(windows)]
pub(super) fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    // Windows distinguishes file/dir symlinks and may require privileges; treat
    // failure as an extraction error. In practice engine archives we support are
    // Unix-origin tarballs, but we must handle them correctly per spec.
    std::os::windows::fs::symlink_file(target, link)
        .or_else(|_| std::os::windows::fs::symlink_dir(target, link))
        .map_err(|source| DblError::Io { source })
}

#[cfg(not(any(unix, windows)))]
pub(super) fn create_symlink(_target: &Path, _link: &Path) -> Result<()> {
    Err(DblError::Unsupported {
        reason: "symlink creation is not supported on this platform".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use tempfile::tempdir;

    use super::*;

    fn caps(max_bytes: u64) -> ExtractCaps {
        ExtractCaps {
            max_total_bytes: max_bytes,
            max_entries: 10,
        }
    }

    #[test]
    fn write_regular_file_copies_and_counts() {
        let dir = tempdir().expect("tmp");
        let dest = dir.path().join("out");
        let mut state = ExtractState::default();
        let mut src: &[u8] = b"hello";
        let written =
            write_regular_file(&mut src, &dest, &caps(1024), &mut state, "out").expect("write");
        assert_eq!(written, 5);
        assert_eq!(std::fs::read(&dest).expect("read"), b"hello");
        assert_eq!(state.total_bytes, 5);
    }

    #[test]
    fn write_regular_file_propagates_a_read_error_as_io() {
        struct Failing;
        impl Read for Failing {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("boom"))
            }
        }
        let dir = tempdir().expect("tmp");
        let mut state = ExtractState::default();
        let err = write_regular_file(
            &mut Failing,
            &dir.path().join("x"),
            &caps(1024),
            &mut state,
            "x",
        )
        .unwrap_err();
        assert!(matches!(err, DblError::Io { .. }), "{err:?}");
    }

    #[test]
    fn write_regular_file_maps_invalid_data_to_archive_error() {
        struct Corrupt;
        impl Read for Corrupt {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad"))
            }
        }
        let dir = tempdir().expect("tmp");
        let mut state = ExtractState::default();
        let err = write_regular_file(
            &mut Corrupt,
            &dir.path().join("x"),
            &caps(1024),
            &mut state,
            "x",
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                DblError::InvalidInput {
                    field: "archive",
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_and_follow_symlink_round_trip() {
        let dir = tempdir().expect("tmp");
        let target = dir.path().join("target");
        std::fs::write(&target, b"data").expect("write target");
        let link = dir.path().join("link");
        create_symlink(Path::new("target"), &link).expect("symlink");
        let meta = std::fs::symlink_metadata(&link).expect("meta");
        assert!(meta.file_type().is_symlink());
        assert_eq!(std::fs::read(&link).expect("follow"), b"data");
    }

    #[cfg(unix)]
    #[test]
    fn apply_unix_mode_strips_file_type_bits() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("f");
        std::fs::write(&path, b"x").expect("write");
        apply_unix_mode(&path, 0o100755).expect("mode");
        let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn extract_state_enforces_caps() {
        let mut state = ExtractState::default();
        let caps = ExtractCaps {
            max_total_bytes: 3,
            max_entries: 1,
        };
        state.count_entry(&caps).expect("first entry");
        let err = state.count_entry(&caps).unwrap_err();
        assert!(matches!(err, DblError::UnsafeArchive { .. }), "{err:?}");

        let mut state = ExtractState::default();
        state.add_bytes(2, &caps, "a").expect("under");
        let err = state.add_bytes(2, &caps, "a").unwrap_err();
        assert!(matches!(err, DblError::UnsafeArchive { .. }), "{err:?}");
    }

    #[test]
    fn hard_link_fallback_copies_bytes_and_counts_quota() {
        // Item 7: inject a hard-link failure; fallback must copy bytes and count them
        // toward the total-bytes quota.
        let dir = tempdir().expect("tmp");
        let target = dir.path().join("target");
        std::fs::write(&target, b"payload").expect("target");
        let link = dir.path().join("link");
        let caps = ExtractCaps {
            max_total_bytes: 1024,
            max_entries: 10,
        };
        let mut state = ExtractState::default();
        create_hard_link_with(&target, &link, &caps, &mut state, "link", |_t, _l| {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "hard links unsupported",
            ))
        })
        .expect("fallback copy");
        assert_eq!(std::fs::read(&link).expect("read"), b"payload");
        assert_eq!(
            state.total_bytes, 7,
            "copy fallback must count bytes toward quota"
        );
    }

    #[test]
    fn hard_link_fallback_respects_total_bytes_quota() {
        let dir = tempdir().expect("tmp");
        let target = dir.path().join("target");
        std::fs::write(&target, b"payload").expect("target");
        let link = dir.path().join("link");
        let caps = ExtractCaps {
            max_total_bytes: 3,
            max_entries: 10,
        };
        let mut state = ExtractState::default();
        let err = create_hard_link_with(&target, &link, &caps, &mut state, "link", |_t, _l| {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "hard links unsupported",
            ))
        })
        .unwrap_err();
        assert!(matches!(err, DblError::UnsafeArchive { .. }), "{err:?}");
        assert!(!link.exists(), "quota failure must not leave destination");
    }

    #[test]
    fn hard_link_eexist_does_not_fallback() {
        // Item 7: EEXIST/AlreadyExists must NOT fall back to copy.
        let dir = tempdir().expect("tmp");
        let target = dir.path().join("target");
        std::fs::write(&target, b"payload").expect("target");
        let link = dir.path().join("link");
        let caps = ExtractCaps {
            max_total_bytes: 1024,
            max_entries: 10,
        };
        let mut state = ExtractState::default();
        let err = create_hard_link_with(&target, &link, &caps, &mut state, "link", |_t, _l| {
            Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "exists",
            ))
        })
        .unwrap_err();
        assert!(matches!(err, DblError::Io { .. }), "{err:?}");
        assert!(!link.exists());
        assert_eq!(state.total_bytes, 0, "no fallback copy on EEXIST");
    }

    #[cfg(unix)]
    #[test]
    fn hard_link_raw_eexist_code_does_not_fallback() {
        let dir = tempdir().expect("tmp");
        let target = dir.path().join("target");
        std::fs::write(&target, b"payload").expect("target");
        let link = dir.path().join("link");
        let caps = ExtractCaps {
            max_total_bytes: 1024,
            max_entries: 10,
        };
        let mut state = ExtractState::default();
        let err = create_hard_link_with(&target, &link, &caps, &mut state, "link", |_t, _l| {
            Err(std::io::Error::from_raw_os_error(17)) // EEXIST on Linux
        })
        .unwrap_err();
        assert!(matches!(err, DblError::Io { .. }), "{err:?}");
        assert_eq!(state.total_bytes, 0);
    }
}
