//! Safe `.tar.gz` extraction and host smoke testing for Linux PHP, Linux
//! Node.js, and Composer artifacts.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};

/// Maximum cumulative bytes extracted during a smoke test (512 MiB).
pub const MAX_EXTRACTED_BYTES: u64 = 512 * 1024 * 1024;
/// Wall-clock timeout for a single smoke-test command.
const SMOKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Abstraction over running downloaded binaries so unit tests stay offline.
pub trait SmokeRunner {
    /// Extracts a Linux PHP `.tar.gz`, runs `php -n -v` and `php -n -m`, and
    /// returns the sorted extension list. Optionally copies the verified `php`
    /// binary to `keep_binary_at` for reuse by the Composer smoke test.
    fn verify_php_linux(
        &self,
        archive_path: &Path,
        expected_version: &str,
        keep_binary_at: Option<&Path>,
    ) -> Result<Vec<String>, String>;

    /// Extracts a Linux Node.js `.tar.gz` and runs `bin/node -v` and `bin/npm -v`.
    fn verify_node_linux(&self, archive_path: &Path, expected_version: &str) -> Result<(), String>;

    /// Runs `php -n composer.phar --version` against a verified PHP binary.
    fn verify_composer(
        &self,
        phar_path: &Path,
        php_binary: &Path,
        expected_version: &str,
    ) -> Result<(), String>;
}

/// Real host smoke runner (Linux).
pub struct HostSmokeRunner;

impl SmokeRunner for HostSmokeRunner {
    fn verify_php_linux(
        &self,
        archive_path: &Path,
        expected_version: &str,
        keep_binary_at: Option<&Path>,
    ) -> Result<Vec<String>, String> {
        let extract_dir = tempfile::tempdir()
            .map_err(|err| format!("failed to create PHP extract tempdir: {err}"))?;
        safe_extract_tar_gz(archive_path, extract_dir.path(), MAX_EXTRACTED_BYTES)?;

        let php_bin = find_binary(extract_dir.path(), "php")
            .ok_or_else(|| "extracted PHP archive does not contain a php binary".to_owned())?;
        ensure_executable(&php_bin)?;

        let version_out = run_captured(&php_bin, &["-n", "-v"], &[])?;
        if !version_out.contains(expected_version) {
            return Err(format!(
                "php -v output did not contain {expected_version}: {}",
                version_out.lines().next().unwrap_or("")
            ));
        }

        let modules_out = run_captured(&php_bin, &["-n", "-m"], &[])?;
        let extensions = parse_php_modules(&modules_out);
        if extensions.is_empty() {
            return Err("php -m returned zero extensions".to_owned());
        }

        if let Some(dest) = keep_binary_at {
            std::fs::copy(&php_bin, dest)
                .map_err(|err| format!("failed to copy php binary for composer smoke: {err}"))?;
            ensure_executable(dest)?;
        }

        Ok(extensions)
    }

    fn verify_node_linux(&self, archive_path: &Path, expected_version: &str) -> Result<(), String> {
        let extract_dir = tempfile::tempdir()
            .map_err(|err| format!("failed to create Node extract tempdir: {err}"))?;
        safe_extract_tar_gz(archive_path, extract_dir.path(), MAX_EXTRACTED_BYTES)?;

        let node_bin = find_binary(extract_dir.path(), "node")
            .ok_or_else(|| "extracted Node archive does not contain bin/node".to_owned())?;
        let npm_bin = find_binary(extract_dir.path(), "npm")
            .ok_or_else(|| "extracted Node archive does not contain bin/npm".to_owned())?;

        let bin_dir = node_bin
            .parent()
            .ok_or_else(|| "node binary has no parent dir".to_owned())?;
        let path_env = prepend_path(bin_dir);

        let node_out = run_captured(&node_bin, &["-v"], &[("PATH", &path_env)])?;
        if !node_out.contains(expected_version) {
            return Err(format!(
                "node -v output did not contain {expected_version}: {}",
                node_out.trim()
            ));
        }

        let npm_out = run_captured(&npm_bin, &["-v"], &[("PATH", &path_env)])?;
        if npm_out.trim().is_empty() {
            return Err("npm -v returned empty output".to_owned());
        }
        Ok(())
    }

    fn verify_composer(
        &self,
        phar_path: &Path,
        php_binary: &Path,
        expected_version: &str,
    ) -> Result<(), String> {
        let home_dir = tempfile::tempdir()
            .map_err(|err| format!("failed to create composer tempdir: {err}"))?;
        let home_str = home_dir.path().to_string_lossy().into_owned();
        let phar_str = phar_path.to_string_lossy().into_owned();
        let out = run_captured(
            php_binary,
            &["-n", &phar_str, "--version"],
            &[("COMPOSER_HOME", &home_str), ("HOME", &home_str)],
        )?;
        if !out.contains(expected_version) {
            return Err(format!(
                "composer --version output did not contain {expected_version}: {}",
                out.trim()
            ));
        }
        Ok(())
    }
}

/// Parses `php -n -m` output into a sorted, deduplicated lowercase list.
///
/// Normalizes `"Zend OPcache"` to `"opcache"` to match the catalog convention.
pub fn parse_php_modules(output: &str) -> Vec<String> {
    let mut set = BTreeSet::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('[') {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        let normalized = if lower == "zend opcache" {
            "opcache".to_owned()
        } else {
            lower
        };
        set.insert(normalized);
    }
    set.into_iter().collect()
}

/// Safely extracts a `.tar.gz` archive into `dest_dir`.
///
/// Rejects absolute paths, `..` components, links that escape `dest_dir`,
/// device/FIFO entries, setuid/setgid mode bits, and archives exceeding
/// `max_extracted_bytes`.
pub fn safe_extract_tar_gz(
    archive_path: &Path,
    dest_dir: &Path,
    max_extracted_bytes: u64,
) -> Result<(), String> {
    let file = File::open(archive_path)
        .map_err(|err| format!("cannot open archive {}: {err}", archive_path.display()))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|err| format!("cannot read tar entries: {err}"))?;

    let mut total_bytes: u64 = 0;
    for entry_res in entries {
        let mut entry = entry_res.map_err(|err| format!("corrupt tar entry: {err}"))?;
        let rel_path = entry
            .path()
            .map_err(|err| format!("invalid tar entry path: {err}"))?
            .into_owned();
        validate_relative_path(&rel_path)?;

        let entry_type = entry.header().entry_type();
        let mode = entry.header().mode().unwrap_or(0o644);
        if mode & 0o6000 != 0 {
            return Err(format!(
                "refusing setuid/setgid entry {}",
                rel_path.display()
            ));
        }

        let target = dest_dir.join(&rel_path);
        match entry_type {
            EntryType::Directory => {
                std::fs::create_dir_all(&target)
                    .map_err(|err| format!("cannot create dir {}: {err}", target.display()))?;
            }
            EntryType::Regular => {
                extract_regular_file(
                    &mut entry,
                    &target,
                    mode,
                    &mut total_bytes,
                    max_extracted_bytes,
                )?;
            }
            EntryType::Symlink | EntryType::Link => {
                let link_target = entry
                    .link_name()
                    .map_err(|err| format!("invalid link target: {err}"))?
                    .ok_or_else(|| format!("missing link target on {}", rel_path.display()))?
                    .into_owned();
                validate_link_within_root(&rel_path, &link_target)?;
                create_link(&target, &link_target, entry_type, dest_dir)?;
            }
            other => {
                return Err(format!(
                    "refusing unsupported tar entry type {other:?} at {}",
                    rel_path.display()
                ));
            }
        }
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<(), String> {
    if path.is_absolute() {
        return Err(format!("refusing absolute path {}", path.display()));
    }
    for comp in path.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("refusing escaping path {}", path.display()));
            }
        }
    }
    Ok(())
}

fn validate_link_within_root(entry_rel: &Path, link_target: &Path) -> Result<(), String> {
    if link_target.is_absolute() {
        return Err(format!(
            "refusing absolute link target {} on {}",
            link_target.display(),
            entry_rel.display()
        ));
    }
    let mut depth: i32 = entry_rel
        .parent()
        .map(|p| {
            p.components()
                .filter(|c| matches!(c, Component::Normal(_)))
                .count() as i32
        })
        .unwrap_or(0);
    for comp in link_target.components() {
        match comp {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(format!(
                        "refusing symlink {} escaping extract root via {}",
                        entry_rel.display(),
                        link_target.display()
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "refusing non-relative link target {}",
                    link_target.display()
                ));
            }
        }
    }
    Ok(())
}

fn extract_regular_file<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    target: &Path,
    mode: u32,
    total_bytes: &mut u64,
    max_bytes: u64,
) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("cannot create parent dir {}: {err}", parent.display()))?;
    }
    let mut out =
        File::create(target).map_err(|err| format!("cannot create {}: {err}", target.display()))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = entry
            .read(&mut buf)
            .map_err(|err| format!("read error extracting {}: {err}", target.display()))?;
        if n == 0 {
            break;
        }
        *total_bytes = total_bytes.saturating_add(n as u64);
        if *total_bytes > max_bytes {
            return Err(format!(
                "archive exceeded maximum extracted size of {max_bytes} bytes"
            ));
        }
        std::io::Write::write_all(&mut out, &buf[..n])
            .map_err(|err| format!("write error on {}: {err}", target.display()))?;
    }
    set_safe_permissions(target, mode)?;
    Ok(())
}

#[cfg(unix)]
fn create_link(
    target: &Path,
    link_target: &Path,
    entry_type: EntryType,
    dest_dir: &Path,
) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("cannot create parent dir {}: {err}", parent.display()))?;
    }
    if target.exists() || target.symlink_metadata().is_ok() {
        let _ = std::fs::remove_file(target);
    }
    match entry_type {
        EntryType::Symlink => std::os::unix::fs::symlink(link_target, target)
            .map_err(|err| format!("cannot create symlink {}: {err}", target.display())),
        EntryType::Link => std::fs::hard_link(dest_dir.join(link_target), target)
            .map_err(|err| format!("cannot create hard link {}: {err}", target.display())),
        _ => Ok(()),
    }
}

#[cfg(not(unix))]
fn create_link(
    _target: &Path,
    _link_target: &Path,
    _entry_type: EntryType,
    _dest_dir: &Path,
) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_safe_permissions(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let safe_mode = mode & 0o0777;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(safe_mode))
        .map_err(|err| format!("cannot chmod {}: {err}", path.display()))
}

#[cfg(not(unix))]
fn set_safe_permissions(_path: &Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn ensure_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let meta =
        std::fs::metadata(path).map_err(|err| format!("cannot stat {}: {err}", path.display()))?;
    let mode = meta.permissions().mode() | 0o755;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|err| format!("cannot chmod +x {}: {err}", path.display()))
}

#[cfg(not(unix))]
fn ensure_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn find_binary(root: &Path, name: &str) -> Option<PathBuf> {
    let direct = root.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let bin_direct = root.join("bin").join(name);
    if bin_direct.is_file() {
        return Some(bin_direct);
    }
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let nested = path.join("bin").join(name);
            if nested.is_file() {
                return Some(nested);
            }
            let flat = path.join(name);
            if flat.is_file() {
                return Some(flat);
            }
        }
    }
    None
}

fn prepend_path(dir: &Path) -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    if current.is_empty() {
        dir.to_string_lossy().into_owned()
    } else {
        format!("{}:{current}", dir.display())
    }
}

fn run_captured(program: &Path, args: &[&str], envs: &[(&str, &str)]) -> Result<String, String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out_file = tempfile::NamedTempFile::new()
        .map_err(|err| format!("failed to create smoke output file: {err}"))?;
    let err_file = tempfile::NamedTempFile::new()
        .map_err(|err| format!("failed to create smoke stderr file: {err}"))?;
    let out_handle = out_file
        .reopen()
        .map_err(|err| format!("failed to reopen stdout file: {err}"))?;
    let err_handle = err_file
        .reopen()
        .map_err(|err| format!("failed to reopen stderr file: {err}"))?;
    cmd.stdout(out_handle).stderr(err_handle);

    match crate::limit::run_with_limit(cmd, SMOKE_TIMEOUT) {
        crate::limit::Outcome::Finished(status) if status.success() => {
            std::fs::read_to_string(out_file.path())
                .map_err(|err| format!("failed to read smoke stdout: {err}"))
        }
        crate::limit::Outcome::Finished(status) => {
            let stderr = std::fs::read_to_string(err_file.path()).unwrap_or_default();
            Err(format!(
                "{} {} exited with {status}: {}",
                program.display(),
                args.join(" "),
                stderr.trim()
            ))
        }
        crate::limit::Outcome::TimedOut => Err(format!(
            "{} {} timed out after {}s",
            program.display(),
            args.join(" "),
            SMOKE_TIMEOUT.as_secs()
        )),
        crate::limit::Outcome::Failed(err) => Err(format!(
            "failed to spawn {} {}: {err}",
            program.display(),
            args.join(" ")
        )),
    }
}
