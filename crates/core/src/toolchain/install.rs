//! Installing and removing one toolchain version.
//!
//! The install pipeline reuses the engine downloader and extractor rather than
//! reimplementing them:
//!
//! 1. **Preflight** — refuse when the target filesystem lacks space, using the
//!    same `size × 3` rule the engines use.
//! 2. **Download + verify** — [`crate::download::install_source_to`] streams the
//!    artifact to a `.part` under `<root>/toolchains/.tmp`, verifies the SHA-256
//!    *before* any byte is extracted, and refuses an oversize body.
//! 3. **Extract** — tar-slip/zip-slip protection, entry count and size caps, and
//!    symlink-escape rejection, all in the shared extractor. Node's tarballs have
//!    a top-level `node-vX-linux-x64/` that is stripped; PHP's prebuilt trees and
//!    Windows zips are laid out from the archive root.
//! 4. **Validate** — the tool's expected binaries must exist, derived from
//!    [`ToolId::primary_binary_paths`] so Windows names come from product code.
//! 5. **Finalise** — the manifest is written and the staging tree is renamed into
//!    `<root>/toolchains/<tool>/<version>[-<profile>]`. A crash mid-install leaves
//!    either the old tree or nothing at the final path; retrying is safe.
//!
//! Removal renames the install aside first, so a locked file (Windows) leaves the
//! version intact, then deletes it; leftovers are swept on the next attempt. A
//! process started from the install refuses the removal (best effort).

use std::path::{Path, PathBuf};

use crate::DblError;
use crate::download::{
    CancellationToken, FreeSpaceFn, InstallOptions, ProgressSink, SourceSpec, UrlPolicy,
};
use crate::model::ArchiveFormat;
use crate::platform::{Arch, Os};
use crate::toolchain::catalog::{Artifact, Format, ToolId};
use crate::toolchain::consts;
use crate::toolchain::fsops::ToolchainPaths;
use crate::toolchain::lock::ToolchainLock;
use crate::toolchain::registry;
use sha2::Digest as _;

/// Whether an install wrote anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// A normal install.
    Fresh,
    /// Every file of this version was already in place.
    AlreadyInstalled,
}

/// What one install produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    /// The install directory.
    pub dir: PathBuf,
    /// Whether anything was written.
    pub kind: InstallKind,
    /// Bytes the archive occupied (0 when already installed).
    pub downloaded_bytes: u64,
}

/// Everything an install needs besides the paths.
pub struct InstallRequest<'a> {
    /// Tool being installed.
    pub tool: ToolId,
    /// The catalog artifact to install.
    pub artifact: &'a Artifact,
    /// Version tag.
    pub version: &'a str,
    /// Profile (`Some` for PHP Linux builds).
    pub profile: Option<&'a str>,
    /// URL policy override (tests reach a loopback fixture server).
    pub policy: Option<UrlPolicy>,
    /// Progress sink, shared with the engine downloader's callback shape.
    pub progress: Option<ProgressSink>,
    /// Cancellation shared with the downloader.
    pub cancel: Option<CancellationToken>,
    /// Free-space probe override (tests).
    pub free_space: Option<FreeSpaceFn>,
    /// Allow loopback `http://` in debug/test builds only. Ignored in release builds.
    pub allow_loopback_http: bool,
}

impl<'a> InstallRequest<'a> {
    /// Builds a request for one artifact/version.
    pub fn new(
        tool: ToolId,
        artifact: &'a Artifact,
        version: &'a str,
        profile: Option<&'a str>,
    ) -> Self {
        Self {
            tool,
            artifact,
            version,
            profile,
            policy: None,
            progress: None,
            cancel: None,
            free_space: None,
            allow_loopback_http: false,
        }
    }
}

/// Installs one toolchain version, idempotently.
///
/// An already-installed version is a success that writes nothing. The operation
/// takes the cross-process lock for this tool/version, so a concurrent install or
/// remove of the same version is serialized rather than corrupting the tree.
pub async fn install(
    paths: &ToolchainPaths,
    os: Os,
    _arch: Arch,
    request: InstallRequest<'_>,
) -> crate::Result<InstallOutcome> {
    let version = request.version;
    let profile = request.profile;
    let final_dir = paths.checked_install_dir(request.tool, version, profile)?;

    if is_installed(&final_dir) {
        return Ok(InstallOutcome {
            dir: final_dir,
            kind: InstallKind::AlreadyInstalled,
            downloaded_bytes: 0,
        });
    }

    let lock_path = paths.lock_file(request.tool, version, profile);
    let _lock = ToolchainLock::acquire_default(&lock_path)?;

    // Re-check under the lock: another process may have finished the same install.
    if is_installed(&final_dir) {
        return Ok(InstallOutcome {
            dir: final_dir,
            kind: InstallKind::AlreadyInstalled,
            downloaded_bytes: 0,
        });
    }

    // A PHAR is a single file, not an archive; the shared extractor has no mode
    // for it, so it takes a small dedicated path.
    if matches!(request.artifact.format(), Format::Phar) {
        return install_single_file(paths, &final_dir, &request, request.artifact.size_bytes())
            .await;
    }

    cleanup_stale(&final_dir);

    let tmp = paths.tmp_dir();
    tokio::fs::create_dir_all(&tmp)
        .await
        .map_err(|source| DblError::Io { source })?;

    let artifact = request.artifact;
    let label = download_label(request.tool, version, os, request.profile);
    let strip = strip_prefix(request.tool, artifact.format(), version, os);
    let source = SourceSpec {
        url: artifact.url(),
        label: &label,
        sha256: artifact.sha256(),
        size_bytes: artifact.size_bytes(),
        format: archive_format(artifact.format()),
        strip_prefix: strip.as_deref(),
    };

    let expected: Vec<String> = expected_binaries(request.tool, os);
    let expected_refs: Vec<&str> = expected.iter().map(String::as_str).collect();

    let use_loopback_fetch = {
        #[cfg(any(test, debug_assertions))]
        {
            request.allow_loopback_http
                && request.policy.is_none()
                && crate::toolchain::catalog::is_loopback_http_url(artifact.url())
        }
        #[cfg(not(any(test, debug_assertions)))]
        {
            false
        }
    };

    let result = if use_loopback_fetch {
        crate::download::check_disk_space(
            &tmp,
            artifact.size_bytes(),
            request.free_space.as_ref(),
        )?;
        let part = tmp.join(format!("{label}.part"));
        let download_res = download_verified(artifact, &part, true).await;
        if let Err(err) = download_res {
            let _ = tokio::fs::remove_file(&part).await;
            Err(err)
        } else {
            let opts = crate::download::ExtractOptions {
                expected_binaries: expected.clone(),
                strip_prefix: strip.clone(),
                ..Default::default()
            };
            let extracted = crate::download::extract_runtime(
                &part,
                archive_format(artifact.format()),
                artifact.size_bytes(),
                &final_dir,
                &opts,
            )
            .await;
            let _ = tokio::fs::remove_file(&part).await;
            extracted
        }
    } else {
        crate::download::install_source_to(
            &source,
            &tmp,
            &final_dir,
            &expected_refs,
            install_options(&request),
        )
        .await
    };

    let dir = match result {
        Ok(dir) => dir,
        Err(err) => {
            cleanup_stale(&final_dir);
            return Err(err);
        }
    };

    // Reject any setuid/setgid bits left on extracted files (SPEC-common security rule).
    if let Err(err) = reject_setuid_setgid(&dir) {
        let _ = tokio::fs::remove_dir_all(&dir).await;
        return Err(err);
    }

    // Official Windows PHP zips ship without a php.ini; write our managed base
    // php.ini with the default extension set enabled.
    if request.tool == ToolId::Php && os == Os::Windows {
        let exts: Vec<String> = consts::PHP_WINDOWS_DEFAULT_EXTENSIONS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        if let Err(err) = crate::toolchain::php_ini::write_windows_base_ini(&dir, &exts) {
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return Err(err);
        }
    }

    // Write the manifest last: until it exists the scan ignores the directory, so
    // a crash between extract and here leaves a tree the next install overwrites
    // rather than trusting.
    if let Err(err) = registry::write_manifest(
        &dir,
        request.tool,
        version,
        profile,
        artifact.url(),
        artifact.sha256(),
    ) {
        let _ = tokio::fs::remove_dir_all(&dir).await;
        return Err(err);
    }

    Ok(InstallOutcome {
        dir,
        kind: InstallKind::Fresh,
        downloaded_bytes: artifact.size_bytes(),
    })
}

fn install_options(request: &InstallRequest<'_>) -> InstallOptions {
    let mut options = InstallOptions::default();
    if let Some(policy) = request.policy.clone() {
        options = options.with_policy(policy);
    }
    if let Some(progress) = request.progress.clone() {
        options = options.with_progress(progress);
    }
    if let Some(cancel) = request.cancel.clone() {
        options = options.with_cancel(cancel);
    }
    if let Some(probe) = request.free_space.clone() {
        options = options.with_free_space(probe);
    }
    options
}

/// Downloads a single file (a Composer PHAR) and moves it into place.
async fn install_single_file(
    paths: &ToolchainPaths,
    final_dir: &Path,
    request: &InstallRequest<'_>,
    size_bytes: u64,
) -> crate::Result<InstallOutcome> {
    let tmp = paths.tmp_dir();
    tokio::fs::create_dir_all(&tmp)
        .await
        .map_err(|source| DblError::Io { source })?;
    let artifact = request.artifact;
    let label = download_label(request.tool, request.version, Os::current(), None);

    // Space preflight matches the archive path.
    crate::download::check_disk_space(&tmp, size_bytes, request.free_space.as_ref())?;

    let part = tmp.join(format!("{label}.phar.part"));
    let allow_loopback = request.policy.is_some()
        || (cfg!(any(test, debug_assertions)) && request.allow_loopback_http);
    download_verified(artifact, &part, allow_loopback).await?;

    let staging = tmp.join(format!(".{label}.phar-staging"));
    if staging.exists() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
    }
    tokio::fs::create_dir_all(&staging)
        .await
        .map_err(|source| DblError::Io { source })?;
    let target = staging.join(crate::toolchain::catalog::php_phar_file());
    if let Err(err) = tokio::fs::rename(&part, &target).await {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        let _ = tokio::fs::remove_file(&part).await;
        return Err(DblError::Io { source: err });
    }
    if let Err(err) = registry::write_manifest(
        &staging,
        request.tool,
        request.version,
        None,
        artifact.url(),
        artifact.sha256(),
    ) {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(err);
    }
    if let Some(parent) = final_dir.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| DblError::Io { source })?;
    }
    if final_dir.exists() {
        let _ = tokio::fs::remove_dir_all(final_dir).await;
    }
    match tokio::fs::rename(&staging, final_dir).await {
        Ok(()) => Ok(InstallOutcome {
            dir: final_dir.to_path_buf(),
            kind: InstallKind::Fresh,
            downloaded_bytes: size_bytes,
        }),
        Err(source) => {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            Err(DblError::Io { source })
        }
    }
}

/// A filename-safe label for the `.part` file of this download.
fn download_label(tool: ToolId, version: &str, os: Os, profile: Option<&str>) -> String {
    let mut label = format!("toolchain-{}-{version}-{}", tool.as_str(), os.as_str());
    if let Some(profile) = profile {
        label.push('-');
        label.push_str(profile);
    }
    label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Downloads a single verified file into `dest` (the Composer PHAR).
///
/// The URL must be HTTPS; plain loopback HTTP is accepted only when a test policy
/// is supplied, matching the engine downloader's discipline. The SHA-256 is
/// checked before anything is moved into place, and a failed or mismatched
/// download leaves no `dest`. A PHAR is a few megabytes, so buffering it in memory
/// is cheaper than the resume bookkeeping the archive path needs.
async fn download_verified(
    artifact: &Artifact,
    dest: &Path,
    allow_loopback_http: bool,
) -> crate::Result<()> {
    let url = reqwest::Url::parse(artifact.url()).map_err(|e| DblError::InvalidInput {
        field: "url",
        reason: format!("invalid URL: {e}"),
    })?;
    validate_download_url(&url, allow_loopback_http)?;

    let redirect_policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= consts::PHAR_MAX_REDIRECTS {
            return attempt.error("too many redirects");
        }
        if validate_download_url(attempt.url(), allow_loopback_http).is_err() {
            return attempt.error("refusing redirect to non-HTTPS URL");
        }
        attempt.follow()
    });
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(
            consts::PHAR_CONNECT_TIMEOUT_SECS,
        ))
        .timeout(std::time::Duration::from_secs(
            consts::PHAR_OVERALL_TIMEOUT_SECS,
        ))
        .redirect(redirect_policy)
        .build()
        .map_err(|err| DblError::Network {
            reason: err.to_string(),
        })?;
    let response = client.get(url).send().await.map_err(|err| {
        let detail = std::error::Error::source(&err)
            .map(|src| format!("{err}: {src}"))
            .unwrap_or_else(|| err.to_string());
        DblError::Network { reason: detail }
    })?;
    if !response.status().is_success() {
        return Err(DblError::Network {
            reason: format!("download got HTTP {}", response.status().as_u16()),
        });
    }
    if let Some(length) = response.content_length()
        && length != artifact.size_bytes()
    {
        return Err(DblError::ChecksumMismatch {
            artifact: format!(
                "{}: server declared {length} bytes, catalog expects {}",
                artifact.url(),
                artifact.size_bytes()
            ),
        });
    }

    let mut hasher = sha2::Sha256::new();
    let mut bytes: Vec<u8> = Vec::with_capacity(artifact.size_bytes() as usize);
    let mut stream = response;
    loop {
        let chunk = stream.chunk().await.map_err(|err| DblError::Network {
            reason: err.to_string(),
        })?;
        let Some(chunk) = chunk else { break };
        if bytes.len() as u64 + chunk.len() as u64 > artifact.size_bytes() {
            return Err(DblError::Network {
                reason: format!(
                    "{}: server sent more than the catalog's {} bytes",
                    artifact.url(),
                    artifact.size_bytes()
                ),
            });
        }
        hasher.update(&chunk);
        bytes.extend_from_slice(&chunk);
    }
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != artifact.sha256() {
        return Err(DblError::ChecksumMismatch {
            artifact: format!(
                "{} (expected {}, got {actual})",
                artifact.url(),
                artifact.sha256()
            ),
        });
    }
    crate::fsutil::write_private_file_atomic(dest, &bytes).map_err(|source| DblError::Io { source })
}

fn validate_download_url(url: &reqwest::Url, allow_loopback_http: bool) -> crate::Result<()> {
    match url.scheme() {
        "https" => Ok(()),
        "http"
            if allow_loopback_http
                && crate::toolchain::catalog::is_loopback_http_url(url.as_str()) =>
        {
            Ok(())
        }
        other => Err(DblError::Network {
            reason: format!("refusing non-HTTPS download URL scheme '{other}'"),
        }),
    }
}

/// Whether `dir` looks like a finished install.
fn is_installed(dir: &Path) -> bool {
    dir.join(consts::INSTALL_MANIFEST_FILE).is_file()
}

/// Removes staging and set-aside leftovers beside `final_dir`.
fn cleanup_stale(final_dir: &Path) {
    let Some(parent) = final_dir.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let leaf = final_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("install");
    let staging_prefix = format!(".{leaf}{}", crate::download::STAGING_MARKER);
    let removing_prefix = format!(".{leaf}.removing-");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if (name.starts_with(&staging_prefix) || name.starts_with(&removing_prefix))
            && entry.file_type().map(|t| !t.is_symlink()).unwrap_or(false)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn archive_format(format: Format) -> ArchiveFormat {
    match format {
        Format::TarGz => ArchiveFormat::TarGz,
        Format::Zip => ArchiveFormat::Zip,
        // A PHAR never reaches the archive path; this keeps the match exhaustive.
        Format::Phar => ArchiveFormat::Zip,
    }
}

/// Walks `dir` on Unix and refuses any file carrying setuid (`04000`) or
/// setgid (`02000`) permission bits.
#[cfg(unix)]
fn reject_setuid_setgid(dir: &Path) -> crate::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current).map_err(|source| DblError::Io { source })?;
        for entry in entries {
            let entry = entry.map_err(|source| DblError::Io { source })?;
            let path = entry.path();
            let meta =
                std::fs::symlink_metadata(&path).map_err(|source| DblError::Io { source })?;
            if meta.file_type().is_symlink() {
                continue;
            }
            let mode = meta.permissions().mode();
            if mode & 0o6000 != 0 {
                return Err(DblError::UnsafeArchive {
                    entry: format!(
                        "{}: setuid/setgid bit is not allowed (mode {:o})",
                        path.display(),
                        mode & 0o7777
                    ),
                });
            }
            if meta.is_dir() {
                stack.push(path);
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn reject_setuid_setgid(_dir: &Path) -> crate::Result<()> {
    Ok(())
}

/// The top-level directory stripped from an archive, matched exactly.
///
/// * Node Linux tarballs unpack under `node-vX.Y.Z-linux-x64/`.
/// * Node Windows zips unpack under `node-vX.Y.Z-win-x64/`.
/// * PHP static-php-cli tarballs and PHP Windows zips unpack directly.
/// * Composer's PHAR is a single file (handled separately).
fn strip_prefix(tool: ToolId, format: Format, version: &str, os: Os) -> Option<String> {
    match (tool, format, os) {
        (ToolId::Node, Format::TarGz, Os::Linux) => Some(node_root_dir(version)),
        (ToolId::Node, Format::Zip, Os::Windows) => Some(node_windows_root_dir(version)),
        _ => None,
    }
}

/// The wrapper directory of a Node Linux tarball, e.g. `node-v24.11.0-linux-x64`.
pub fn node_root_dir(version: &str) -> String {
    format!("node-v{version}-linux-x64")
}

/// The wrapper directory of a Node Windows zip, e.g. `node-v24.11.0-win-x64`.
pub fn node_windows_root_dir(version: &str) -> String {
    format!("node-v{version}-win-x64")
}

/// Exposes the archive strip prefix to tests without making it public API.
#[cfg(test)]
pub(crate) fn strip_prefix_for_test(
    tool: ToolId,
    format: Format,
    version: &str,
    os: Os,
) -> Option<String> {
    strip_prefix(tool, format, version, os)
}

/// Expected binaries relative to the install root, from product code.
pub fn expected_binaries(tool: ToolId, os: Os) -> Vec<String> {
    tool.primary_binary_paths(os)
}

/// Options controlling a removal.
#[derive(Clone, Default)]
pub struct RemoveOptions {}

/// What a removal produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveOutcome {
    /// The active entry that was cleared, when the removed version was active.
    pub cleared_active: Option<registry::ActiveVersion>,
    /// Whether other versions of the same tool remain installed.
    pub others_remain: bool,
}

/// Removes one installed toolchain version.
///
/// Refuses while a process started from the install is running (best effort),
/// renames the tree aside so a locked file leaves it intact, then deletes it.
/// When the removed version was active, the registry entry is cleared; if other
/// versions of the tool remain, the caller is told so it can explain that nothing
/// was auto-switched.
pub async fn remove(
    paths: &ToolchainPaths,
    tool: ToolId,
    version: &str,
    profile: Option<&str>,
    _options: RemoveOptions,
) -> crate::Result<RemoveOutcome> {
    let final_dir = paths.checked_install_dir(tool, version, profile)?;
    if !final_dir.exists() {
        return Err(crate::toolchain::error::ToolchainError::NotInstalled {
            tool: tool.as_str(),
            version: version.to_owned(),
        }
        .into_dbl_error());
    }

    let lock_path = paths.lock_file(tool, version, profile);
    let _lock = ToolchainLock::acquire_default(&lock_path)?;

    if let Some(pids) = running_pids(&final_dir) {
        return Err(crate::toolchain::error::ToolchainError::ProcessRunning {
            tool: tool.as_str(),
            version: version.to_owned(),
            pids,
        }
        .into_dbl_error());
    }

    set_aside(&final_dir)?;
    let mut first_error: Option<DblError> = None;
    for leftover in leftovers(&final_dir) {
        if let Err(err) = delete_tree(&leftover) {
            first_error.get_or_insert(err);
        }
    }
    if let Some(err) = first_error {
        return Err(err);
    }

    let registry = registry::read_registry(paths);
    let mut cleared = None;
    if let Some(active) = registry.get(tool)
        && active.version() == version
        && active.profile() == profile
    {
        cleared = Some(active.clone());
        registry::write_registry(paths, &registry.without(tool))?;
    }
    let others_remain = registry::scan_installed(paths)
        .iter()
        .any(|v| v.tool() == tool);

    Ok(RemoveOutcome {
        cleared_active: cleared,
        others_remain,
    })
}

/// Renames `dir` to a unique sibling and returns the new path.
fn set_aside(dir: &Path) -> crate::Result<PathBuf> {
    let leaf = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("install");
    let nonce: u64 = rand::random();
    let aside = dir.with_file_name(format!(".{leaf}.removing-{nonce:016x}"));
    match std::fs::rename(dir, &aside) {
        Ok(()) => Ok(aside),
        Err(source) => Err(DblError::RuntimeRemoval {
            reason: format!("{}: {source}", dir.display()),
        }),
    }
}

/// Set-aside and staging trees beside `dir`.
fn leftovers(dir: &Path) -> Vec<PathBuf> {
    let Some(parent) = dir.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let leaf = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("install");
    let removing = format!(".{leaf}.removing-");
    let staging = format!(".{leaf}{}", crate::download::STAGING_MARKER);
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if (name.starts_with(&removing) || name.starts_with(&staging))
            && entry.file_type().map(|t| !t.is_symlink()).unwrap_or(false)
        {
            found.push(entry.path());
        }
    }
    found
}

fn delete_tree(path: &Path) -> std::result::Result<(), DblError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            std::fs::remove_dir_all(path).map_err(|source| DblError::RuntimeRemoval {
                reason: format!("{}: {source}", path.display()),
            })
        }
        Ok(_) => std::fs::remove_file(path).map_err(|source| DblError::RuntimeRemoval {
            reason: format!("{}: {source}", path.display()),
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(DblError::RuntimeRemoval {
            reason: format!("{}: {err}", path.display()),
        }),
    }
}

/// Returns the pids of processes whose executable is inside `install_dir`.
///
/// Best effort and purely observational: it never signals anything. A pid it
/// cannot inspect is ignored, because refusing on an unreadable process would be
/// worse than the race it protects against.
fn running_pids(install_dir: &Path) -> Option<String> {
    let root = std::fs::canonicalize(install_dir).unwrap_or_else(|_| install_dir.to_path_buf());
    let mut system = match sysinfo::System::new() {
        Ok(system) => system,
        Err(err) => {
            tracing::debug!(%err, "could not read the process table; removal proceeds best-effort");
            return None;
        }
    };
    let kind = sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always);
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, kind);
    let mut pids: Vec<String> = Vec::new();
    for (pid, process) in system.processes() {
        let Some(exe) = process.exe() else { continue };
        let exe = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
        if exe.starts_with(&root) {
            pids.push(pid.as_u32().to_string());
        }
    }
    if pids.is_empty() {
        None
    } else {
        pids.sort_unstable();
        Some(pids.join(", "))
    }
}

#[cfg(test)]
mod tests;
