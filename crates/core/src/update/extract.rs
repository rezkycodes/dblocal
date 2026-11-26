//! Safe, allowlisted archive extraction for DBLocal release archives.
//!
//! Only the exact files produced by `.github/workflows/release.yml` are
//! accepted:
//! - Linux `.tar.gz`: `dblocal`, optional `share/applications/dev.dblocal.DBLocal.desktop`,
//!   optional `share/icons/hicolor/scalable/apps/dblocal.svg`, and optional
//!   `share/icons/hicolor/{16,24,32,48,64,128,256,512}x{size}/apps/dblocal.png`.
//! - Windows `.zip`: `dblocal.exe`.
//!
//! Any path traversal (`..`), absolute path, symlink, hard link, device/FIFO,
//! unexpected member, or oversized extraction is rejected before touching any
//! installed file.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;

use super::error::{UpdateError, UpdateResult};

/// Maximum total uncompressed bytes accepted from a release archive (256 MiB).
pub const MAX_UPDATE_EXTRACTED_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum number of entries accepted in a release archive.
pub const MAX_UPDATE_ARCHIVE_ENTRIES: usize = 64;

/// Allowed desktop entry path inside a Linux release tarball.
pub const DESKTOP_ENTRY_REL: &str = "share/applications/dev.dblocal.DBLocal.desktop";
/// Allowed scalable SVG icon path inside a Linux release tarball.
pub const SVG_ICON_REL: &str = "share/icons/hicolor/scalable/apps/dblocal.svg";
/// Allowed PNG icon sizes shipped by `release.yml`.
pub const ICON_SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256, 512];

/// Extracted payload of a Linux `.tar.gz` release archive.
#[derive(Debug, Clone)]
pub struct ExtractedLinuxTarball {
    /// Path to the extracted `dblocal` executable inside the staging dir.
    pub binary: PathBuf,
    /// Optional extracted `.desktop` file path.
    pub desktop_entry: Option<PathBuf>,
    /// Optional extracted scalable SVG icon path.
    pub svg_icon: Option<PathBuf>,
    /// Extracted PNG icons: `(size, path)`.
    pub png_icons: Vec<(u32, PathBuf)>,
}

/// Safely extracts a Linux `.tar.gz` release archive into `dest_dir`.
pub fn extract_linux_tarball(
    archive_path: &Path,
    dest_dir: &Path,
) -> UpdateResult<ExtractedLinuxTarball> {
    extract_linux_tarball_with_cap(archive_path, dest_dir, MAX_UPDATE_EXTRACTED_BYTES)
}

/// Same as [`extract_linux_tarball`] with a configurable byte cap (for tests).
pub(crate) fn extract_linux_tarball_with_cap(
    archive_path: &Path,
    dest_dir: &Path,
    max_bytes: u64,
) -> UpdateResult<ExtractedLinuxTarball> {
    ensure_private_dir(dest_dir)?;
    let file = std::fs::File::open(archive_path).map_err(|source| UpdateError::ApplyFailed {
        reason: format!("could not open update archive: {source}"),
    })?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|source| UpdateError::UnsafeArchive {
            reason: format!("could not read tar entries: {source}"),
        })?;

    let mut total_bytes: u64 = 0;
    let mut count: usize = 0;
    let mut seen_files: HashSet<String> = HashSet::new();
    let mut binary: Option<PathBuf> = None;
    let mut desktop_entry: Option<PathBuf> = None;
    let mut svg_icon: Option<PathBuf> = None;
    let mut png_icons: Vec<(u32, PathBuf)> = Vec::new();

    for entry_res in entries {
        count += 1;
        if count > MAX_UPDATE_ARCHIVE_ENTRIES {
            return Err(UpdateError::UnsafeArchive {
                reason: "update archive has too many entries".to_owned(),
            });
        }
        let mut entry = entry_res.map_err(|source| UpdateError::UnsafeArchive {
            reason: format!("corrupt tar entry: {source}"),
        })?;
        let entry_type = entry.header().entry_type();
        let raw_path = entry.path().map_err(|source| UpdateError::UnsafeArchive {
            reason: format!("invalid entry path: {source}"),
        })?;
        let norm = normalize_relative_entry(&raw_path)?;

        if entry_type.is_dir() {
            if !is_allowed_linux_dir(&norm) {
                return Err(UpdateError::UnsafeArchive {
                    reason: format!("unexpected directory in update archive: {norm}"),
                });
            }
            continue;
        }
        if !entry_type.is_file() {
            return Err(UpdateError::UnsafeArchive {
                reason: format!("disallowed non-regular entry in update archive: {norm}"),
            });
        }
        let kind =
            classify_allowed_linux_file(&norm).ok_or_else(|| UpdateError::UnsafeArchive {
                reason: format!("unexpected file in update archive: {norm}"),
            })?;
        if !seen_files.insert(norm.clone()) {
            return Err(UpdateError::UnsafeArchive {
                reason: format!("duplicate entry in update archive: {norm}"),
            });
        }

        let out_path = dest_dir.join(&norm);
        write_bounded_file(&mut entry, &out_path, &mut total_bytes, max_bytes, &norm)?;

        match kind {
            LinuxMember::Binary => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755))
                        .map_err(|source| UpdateError::ApplyFailed {
                            reason: format!("could not chmod extracted binary: {source}"),
                        })?;
                }
                binary = Some(out_path);
            }
            LinuxMember::Desktop => desktop_entry = Some(out_path),
            LinuxMember::Svg => svg_icon = Some(out_path),
            LinuxMember::Png(size) => png_icons.push((size, out_path)),
        }
    }

    let binary = binary.ok_or_else(|| UpdateError::UnsafeArchive {
        reason: "update archive did not contain the 'dblocal' binary".to_owned(),
    })?;
    Ok(ExtractedLinuxTarball {
        binary,
        desktop_entry,
        svg_icon,
        png_icons,
    })
}

/// Safely extracts `dblocal.exe` from a Windows `.zip` release archive into `dest_dir`.
pub fn extract_windows_zip(archive_path: &Path, dest_dir: &Path) -> UpdateResult<PathBuf> {
    extract_windows_zip_with_cap(archive_path, dest_dir, MAX_UPDATE_EXTRACTED_BYTES)
}

/// Same as [`extract_windows_zip`] with a configurable byte cap (for tests).
pub(crate) fn extract_windows_zip_with_cap(
    archive_path: &Path,
    dest_dir: &Path,
    max_bytes: u64,
) -> UpdateResult<PathBuf> {
    ensure_private_dir(dest_dir)?;
    let file = std::fs::File::open(archive_path).map_err(|source| UpdateError::ApplyFailed {
        reason: format!("could not open zip archive: {source}"),
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|source| UpdateError::UnsafeArchive {
        reason: format!("invalid zip archive: {source}"),
    })?;
    if zip.len() > MAX_UPDATE_ARCHIVE_ENTRIES {
        return Err(UpdateError::UnsafeArchive {
            reason: "update zip has too many entries".to_owned(),
        });
    }

    let mut total_bytes: u64 = 0;
    let mut exe_out: Option<PathBuf> = None;

    for idx in 0..zip.len() {
        let mut entry = zip
            .by_index(idx)
            .map_err(|source| UpdateError::UnsafeArchive {
                reason: format!("corrupt zip entry: {source}"),
            })?;
        let raw_name = entry
            .name()
            .map_err(|source| UpdateError::UnsafeArchive {
                reason: format!("invalid zip entry name: {source}"),
            })?
            .into_owned();
        let norm = normalize_relative_entry(Path::new(&raw_name))?;
        if entry.is_symlink() {
            return Err(UpdateError::UnsafeArchive {
                reason: format!("symlink not allowed in update zip: {norm}"),
            });
        }
        if entry.is_dir() {
            return Err(UpdateError::UnsafeArchive {
                reason: format!("unexpected directory in update zip: {norm}"),
            });
        }
        if !norm.eq_ignore_ascii_case("dblocal.exe") {
            return Err(UpdateError::UnsafeArchive {
                reason: format!("unexpected file in update zip: {norm}"),
            });
        }
        if exe_out.is_some() {
            return Err(UpdateError::UnsafeArchive {
                reason: "duplicate dblocal.exe in update zip".to_owned(),
            });
        }
        let out_path = dest_dir.join("dblocal.exe");
        write_bounded_file(&mut entry, &out_path, &mut total_bytes, max_bytes, &norm)?;
        exe_out = Some(out_path);
    }

    exe_out.ok_or_else(|| UpdateError::UnsafeArchive {
        reason: "update zip did not contain 'dblocal.exe'".to_owned(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinuxMember {
    Binary,
    Desktop,
    Svg,
    Png(u32),
}

fn classify_allowed_linux_file(norm: &str) -> Option<LinuxMember> {
    if norm == "dblocal" {
        return Some(LinuxMember::Binary);
    }
    if norm == DESKTOP_ENTRY_REL {
        return Some(LinuxMember::Desktop);
    }
    if norm == SVG_ICON_REL {
        return Some(LinuxMember::Svg);
    }
    for &size in ICON_SIZES {
        let expected = format!("share/icons/hicolor/{size}x{size}/apps/dblocal.png");
        if norm == expected {
            return Some(LinuxMember::Png(size));
        }
    }
    None
}

fn is_allowed_linux_dir(norm: &str) -> bool {
    if norm.is_empty()
        || matches!(
            norm,
            "share"
                | "share/applications"
                | "share/icons"
                | "share/icons/hicolor"
                | "share/icons/hicolor/scalable"
                | "share/icons/hicolor/scalable/apps"
        )
    {
        return true;
    }
    for &size in ICON_SIZES {
        let size_dir = format!("share/icons/hicolor/{size}x{size}");
        let apps_dir = format!("share/icons/hicolor/{size}x{size}/apps");
        if norm == size_dir || norm == apps_dir {
            return true;
        }
    }
    false
}

/// Validates that an archive entry path is relative, has no `..`, no root/prefix,
/// no backslashes, and no control characters, returning a forward-slash normalized string.
fn normalize_relative_entry(path: &Path) -> UpdateResult<String> {
    let raw = path.to_str().ok_or_else(|| UpdateError::UnsafeArchive {
        reason: "archive entry path is not valid UTF-8".to_owned(),
    })?;
    if raw.contains('\\') || raw.starts_with('/') || raw.chars().any(|c| c.is_control()) {
        return Err(UpdateError::UnsafeArchive {
            reason: format!("unsafe archive path: {raw}"),
        });
    }
    let mut parts: Vec<&str> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(seg) => {
                let s = seg.to_str().ok_or_else(|| UpdateError::UnsafeArchive {
                    reason: "archive path segment is not UTF-8".to_owned(),
                })?;
                if s.is_empty() {
                    continue;
                }
                parts.push(s);
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(UpdateError::UnsafeArchive {
                    reason: format!("path traversal or absolute path in archive: {raw}"),
                });
            }
        }
    }
    Ok(parts.join("/"))
}

fn write_bounded_file(
    reader: &mut dyn std::io::Read,
    out_path: &Path,
    total_bytes: &mut u64,
    max_bytes: u64,
    label: &str,
) -> UpdateResult<()> {
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not create extraction subdirectory: {source}"),
        })?;
    }
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out_path)
        .map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not create extracted file '{label}': {source}"),
        })?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|source| UpdateError::UnsafeArchive {
                reason: format!("could not read archive member '{label}': {source}"),
            })?;
        if n == 0 {
            break;
        }
        *total_bytes = total_bytes.saturating_add(n as u64);
        if *total_bytes > max_bytes {
            let _ = std::fs::remove_file(out_path);
            return Err(UpdateError::UnsafeArchive {
                reason: format!("extracted archive exceeds limit of {max_bytes} bytes"),
            });
        }
        std::io::Write::write_all(&mut out, &buf[..n]).map_err(|source| {
            UpdateError::ApplyFailed {
                reason: format!("could not write extracted file '{label}': {source}"),
            }
        })?;
    }
    out.sync_all().map_err(|source| UpdateError::ApplyFailed {
        reason: format!("could not fsync extracted file '{label}': {source}"),
    })?;
    Ok(())
}

/// Creates a private (`0700` on Unix) directory.
pub(crate) fn ensure_private_dir(dir: &Path) -> UpdateResult<()> {
    std::fs::create_dir_all(dir).map_err(|source| UpdateError::ApplyFailed {
        reason: format!("could not create private staging directory: {source}"),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(
            |source| UpdateError::ApplyFailed {
                reason: format!("could not set staging directory permissions: {source}"),
            },
        )?;
    }
    Ok(())
}
