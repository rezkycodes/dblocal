//! Install planning, atomic application, desktop/icon sync, rollback, and restart
//! for self-updates.
//!
//! Replacing DBLocal never touches or signals any running database server: engine
//! processes are spawned detached and adopted on next startup by reconciliation.

use std::path::{Path, PathBuf};
use std::time::Duration;

use semver::Version;

use super::download::StagedUpdate;
use super::error::{UpdateError, UpdateResult};
use super::extract::{
    ExtractedLinuxTarball, ensure_private_dir, extract_linux_tarball, extract_windows_zip,
};
use super::kind::InstallKind;
use super::model::{RestartAction, UpdateAssetKind};

const SMOKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to keep retrying a spawn that failed with ETXTBSY.
const EXEC_BUSY_RETRY_WINDOW: Duration = Duration::from_secs(5);
/// Delay between ETXTBSY spawn retries.
const EXEC_BUSY_RETRY_DELAY: Duration = Duration::from_millis(20);
/// Maximum time the detached Windows helper waits for the parent process to exit.
const WINDOWS_HELPER_WAIT_TIMEOUT: Duration = Duration::from_secs(60);
/// `msiexec.exe` exit code indicating success with a reboot required
/// (`ERROR_SUCCESS_REBOOT_REQUIRED`).
pub const MSI_EXIT_REBOOT_REQUIRED: i32 = 3010;
/// File inside the update staging root where the detached Windows helper
/// persists a failure message so the relaunched app or CLI can display it.
pub const LAST_UPDATE_ERROR_FILE: &str = "last-error.txt";
/// File name of the copied updater binary placed inside the private staging
/// directory so Windows never executes the helper from the target being replaced.
const STAGED_HELPER_EXE_NAME: &str = "dblocal-updater.exe";

/// A pure, serializable-ish description of how to apply an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    kind: InstallKind,
    target_version: Version,
    staged_file: PathBuf,
    current_exe: PathBuf,
    target_path: PathBuf,
    steps: Vec<InstallStep>,
}

impl InstallPlan {
    /// Install kind this plan targets.
    pub fn kind(&self) -> &InstallKind {
        &self.kind
    }

    /// Version to be installed.
    pub fn target_version(&self) -> &Version {
        &self.target_version
    }

    /// Staged verified update file.
    pub fn staged_file(&self) -> &Path {
        &self.staged_file
    }

    /// Current executable path.
    pub fn current_exe(&self) -> &Path {
        &self.current_exe
    }

    /// Actual file that will be replaced.
    pub fn target_path(&self) -> &Path {
        &self.target_path
    }

    /// Planned steps.
    pub fn steps(&self) -> &[InstallStep] {
        &self.steps
    }
}

/// One step in an install plan.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstallStep {
    /// Write staged file next to target binary, copying mode from the live target.
    WriteNextToCurrent {
        src: PathBuf,
        dst: PathBuf,
        mode_from: PathBuf,
    },
    /// fsync the written file.
    Fsync { path: PathBuf },
    /// Create/refresh backup link/copy before swap.
    Backup { target: PathBuf, backup: PathBuf },
    /// Atomically rename over target binary.
    RenameOver { src: PathBuf, dst: PathBuf },
    /// Keep backup until next successful start.
    KeepBackup { backup: PathBuf },
    /// Spawn detached Windows helper to swap or run MSI after exit.
    SpawnWindowsHelper { command: Vec<String> },
    /// macOS bundle replacement plan (deferred; not executed).
    ReplaceMacBundle { bundle: PathBuf, staged: PathBuf },
    /// Refuse because install kind cannot self-update.
    Refuse { reason: String },
}

/// Builds an install plan. Pure function, no filesystem mutation.
pub fn plan_install(
    kind: &InstallKind,
    staged_file: &Path,
    current_exe: &Path,
    target_version: &Version,
) -> UpdateResult<InstallPlan> {
    if !kind.can_self_update() {
        let reason = kind.refusal_reason().unwrap_or_else(|| {
            "Automatic updates are not available for this install method.".to_owned()
        });
        let target_path = install_target_path(kind, current_exe)?;
        return Ok(InstallPlan {
            kind: kind.clone(),
            target_version: target_version.clone(),
            staged_file: staged_file.to_path_buf(),
            current_exe: current_exe.to_path_buf(),
            target_path,
            steps: vec![InstallStep::Refuse { reason }],
        });
    }

    let target_path = install_target_path(kind, current_exe)?;
    let steps = match kind {
        InstallKind::AppImage { .. } | InstallKind::PortableTarball { .. } => {
            plan_linux_replace(staged_file, &target_path)
        }
        InstallKind::WindowsZip { exe_path } => {
            plan_windows_helper(staged_file, exe_path, target_version)
        }
        InstallKind::WindowsMsi { exe_path } => {
            let cmd = windows_msi_helper_command(exe_path, staged_file, std::process::id());
            vec![InstallStep::SpawnWindowsHelper { command: cmd }]
        }
        InstallKind::MacApp { .. }
        | InstallKind::DistPackage { .. }
        | InstallKind::Homebrew { .. }
        | InstallKind::Unknown { .. } => {
            return Err(UpdateError::UnsupportedInstallKind {
                reason: "this install kind does not support self-update".to_owned(),
            });
        }
    };
    Ok(InstallPlan {
        kind: kind.clone(),
        target_version: target_version.clone(),
        staged_file: staged_file.to_path_buf(),
        current_exe: current_exe.to_path_buf(),
        target_path,
        steps,
    })
}

fn install_target_path(kind: &InstallKind, current_exe: &Path) -> UpdateResult<PathBuf> {
    let target = match kind {
        InstallKind::AppImage { appimage_path } => appimage_path.as_path(),
        InstallKind::MacApp { bundle_path } => bundle_path.as_path(),
        _ => current_exe,
    };
    Ok(std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf()))
}

fn ensure_target_writable(target: &Path) -> UpdateResult<()> {
    let Some(dir) = target.parent().filter(|dir| !dir.as_os_str().is_empty()) else {
        return Err(UpdateError::ApplyFailed {
            reason: "the update target directory is not valid".to_owned(),
        });
    };
    let meta = std::fs::metadata(dir).map_err(|source| UpdateError::ApplyFailed {
        reason: format!("could not read the update target directory permissions: {source}"),
    })?;
    if !meta.is_dir() {
        return Err(UpdateError::ApplyFailed {
            reason: "the update target directory path is not valid".to_owned(),
        });
    }
    if !super::kind::directory_is_writable(dir) {
        return Err(UpdateError::UnsupportedInstallKind {
            reason: format!(
                "the update target directory is not writable: {}",
                dir.display()
            ),
        });
    }
    Ok(())
}

fn plan_linux_replace(staged_file: &Path, target: &Path) -> Vec<InstallStep> {
    let dst = target.to_path_buf();
    let tmp = sibling_temp_path(target);
    let backup = backup_path(target);
    vec![
        InstallStep::WriteNextToCurrent {
            src: staged_file.to_path_buf(),
            dst: tmp.clone(),
            mode_from: dst.clone(),
        },
        InstallStep::Fsync { path: tmp.clone() },
        InstallStep::Backup {
            target: dst.clone(),
            backup: backup.clone(),
        },
        InstallStep::RenameOver { src: tmp, dst },
        InstallStep::KeepBackup { backup },
    ]
}

fn plan_windows_helper(staged_file: &Path, exe_path: &Path, target: &Version) -> Vec<InstallStep> {
    let cmd = windows_helper_command(exe_path, staged_file, std::process::id(), target);
    vec![InstallStep::SpawnWindowsHelper { command: cmd }]
}

/// Builds the command line for the Windows zip swap helper.
pub fn windows_helper_command(
    current_exe: &Path,
    staged_file: &Path,
    pid: u32,
    target_version: &Version,
) -> Vec<String> {
    vec![
        current_exe.display().to_string(),
        crate::consts::APPLY_UPDATE_FLAG.to_owned(),
        "--pid".to_owned(),
        pid.to_string(),
        "--staged".to_owned(),
        staged_file.display().to_string(),
        "--target".to_owned(),
        current_exe.display().to_string(),
        "--version".to_owned(),
        target_version.to_string(),
    ]
}

/// Builds the command line for the Windows per-user MSI upgrade helper.
pub fn windows_msi_helper_command(current_exe: &Path, msi_file: &Path, pid: u32) -> Vec<String> {
    vec![
        current_exe.display().to_string(),
        crate::consts::APPLY_UPDATE_FLAG.to_owned(),
        "--pid".to_owned(),
        pid.to_string(),
        "--msi".to_owned(),
        msi_file.display().to_string(),
        "--target".to_owned(),
        current_exe.display().to_string(),
    ]
}

fn sibling_temp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "dblocal".to_owned());
    target.with_file_name(format!(".{name}.update-{}", std::process::id()))
}

fn sibling_extract_dir(target: &Path) -> PathBuf {
    let nonce: u64 = rand::random();
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "dblocal".to_owned());
    target.with_file_name(format!(".{name}.stage-{}-{nonce:016x}", std::process::id()))
}

fn backup_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "dblocal".to_owned());
    target.with_file_name(format!("{name}.old"))
}

/// Applies a verified [`StagedUpdate`] for `kind`, returning the [`RestartAction`]
/// that launches the new version.
///
/// - `PortableTarball`: extracts `.tar.gz` into a fresh private directory on the
///   same filesystem as the target binary, runs `--version` smoke check **before**
///   swapping, atomically replaces the binary (keeping `<exe>.old`), and
///   atomically updates the desktop entry and hicolor icons under `data_home`
///   (or `$XDG_DATA_HOME` / `$HOME/.local/share`).
/// - `AppImage`: copies into a sibling temp file, runs `--version` smoke check
///   **before** swapping, backs up `<appimage>.old`, and atomically renames over
///   `$APPIMAGE`.
/// - `WindowsZip`: extracts `dblocal.exe`, attempts in-place rename swap, or
///   falls back to [`RestartAction::RunWindowsZipHelper`].
/// - `WindowsMsi` (per-user): stages the `.msi` and returns
///   [`RestartAction::RunWindowsMsi`].
pub fn apply_staged_update(
    kind: &InstallKind,
    staged: &StagedUpdate,
    current_exe: &Path,
    data_home_override: Option<&Path>,
) -> UpdateResult<RestartAction> {
    if !kind.can_self_update() {
        return Err(UpdateError::UnsupportedInstallKind {
            reason: kind.refusal_reason().unwrap_or_else(|| {
                "Automatic updates are not available for this install method.".to_owned()
            }),
        });
    }

    match kind {
        InstallKind::PortableTarball { exe_path } => {
            apply_linux_tarball(exe_path, staged, data_home_override)
        }
        InstallKind::AppImage { appimage_path } => apply_linux_appimage(appimage_path, staged),
        InstallKind::WindowsZip { exe_path } => apply_windows_zip(exe_path, current_exe, staged),
        InstallKind::WindowsMsi { exe_path } => {
            let helper_exe = stage_windows_helper_copy(current_exe, staged.run_dir())?;
            Ok(RestartAction::RunWindowsMsi {
                helper_exe,
                msi_path: staged.staged_path().to_path_buf(),
                relaunch_exe: exe_path.clone(),
            })
        }
        _ => Err(UpdateError::UnsupportedInstallKind {
            reason: "this install kind does not support self-update".to_owned(),
        }),
    }
}

fn apply_linux_tarball(
    exe_path: &Path,
    staged: &StagedUpdate,
    data_home_override: Option<&Path>,
) -> UpdateResult<RestartAction> {
    let target = std::fs::canonicalize(exe_path).unwrap_or_else(|_| exe_path.to_path_buf());
    ensure_target_writable(&target)?;

    let extract_dir = sibling_extract_dir(&target);
    let outcome = (|| -> UpdateResult<RestartAction> {
        ensure_private_dir(&extract_dir)?;
        let extracted = match staged.asset_kind() {
            UpdateAssetKind::TarGz => extract_linux_tarball(staged.staged_path(), &extract_dir)?,
            _ => {
                return Err(UpdateError::ApplyFailed {
                    reason: format!(
                        "expected a .tar.gz asset for portable Linux install, got {}",
                        staged.asset_kind()
                    ),
                });
            }
        };

        // Verify the extracted candidate binary BEFORE touching the live installation.
        smoke_check(&extracted.binary, staged.version())?;

        // Atomically swap binary using the proven plan/backup/rollback path.
        let plan = plan_install(
            &InstallKind::PortableTarball {
                exe_path: target.clone(),
            },
            &extracted.binary,
            &target,
            staged.version(),
        )?;
        apply_linux(&plan)?;

        // Atomically update desktop entry and hicolor icons if shipped in the tarball.
        // Non-fatal: the new binary is already installed and verified, so a
        // desktop/icon failure must log a visible warning rather than report the
        // whole update as failed in a half state.
        if let Some(data_home) = resolve_linux_data_home(data_home_override)
            && let Err(err) = install_desktop_and_icons(
                &extracted,
                &target,
                &data_home,
                data_home_override.is_some(),
            )
        {
            tracing::warn!(
                error = %err,
                "binary updated successfully, but updating the desktop entry/icons failed"
            );
        }

        Ok(RestartAction::Relaunch { exe_path: target })
    })();

    let _ = std::fs::remove_dir_all(&extract_dir);
    outcome
}

fn apply_linux_appimage(
    appimage_path: &Path,
    staged: &StagedUpdate,
) -> UpdateResult<RestartAction> {
    let target =
        std::fs::canonicalize(appimage_path).unwrap_or_else(|_| appimage_path.to_path_buf());
    ensure_target_writable(&target)?;

    // Stage into a private directory on the same filesystem so we can chmod and
    // smoke-check before touching the live AppImage.
    let stage_dir = sibling_extract_dir(&target);
    let outcome = (|| -> UpdateResult<RestartAction> {
        ensure_private_dir(&stage_dir)?;
        let candidate = stage_dir.join("dblocal.AppImage");
        copy_with_mode(staged.staged_path(), &candidate, &target)?;
        ensure_executable(&candidate)?;
        fsync_path(&candidate)?;
        smoke_check(&candidate, staged.version())?;

        let plan = plan_install(
            &InstallKind::AppImage {
                appimage_path: target.clone(),
            },
            &candidate,
            &target,
            staged.version(),
        )?;
        apply_linux(&plan)?;
        Ok(RestartAction::Relaunch { exe_path: target })
    })();

    let _ = std::fs::remove_dir_all(&stage_dir);
    outcome
}

fn apply_windows_zip(
    exe_path: &Path,
    current_exe: &Path,
    staged: &StagedUpdate,
) -> UpdateResult<RestartAction> {
    let target = std::fs::canonicalize(exe_path).unwrap_or_else(|_| exe_path.to_path_buf());
    ensure_target_writable(&target)?;

    let extract_dir = staged.run_dir().join("extracted");
    let extracted_exe = extract_windows_zip(staged.staged_path(), &extract_dir)?;

    #[cfg(windows)]
    {
        smoke_check(&extracted_exe, staged.version())?;
        // On Windows, a running .exe can be renamed to .old so the new .exe
        // can take its place immediately. If locked by AV/indexer, fall back
        // to the detached post-exit helper.
        let backup = backup_path(&target);
        let _ = std::fs::remove_file(&backup);
        if std::fs::rename(&target, &backup).is_ok() {
            match std::fs::copy(&extracted_exe, &target) {
                Ok(_) => {
                    let _ = fsync_path(&target);
                    return Ok(RestartAction::Relaunch { exe_path: target });
                }
                Err(_) => {
                    let _ = std::fs::rename(&backup, &target);
                }
            }
        }
    }

    let helper_exe = stage_windows_helper_copy(current_exe, staged.run_dir())?;
    Ok(RestartAction::RunWindowsZipHelper {
        helper_exe,
        staged_exe: extracted_exe,
        target_exe: target,
        version: staged.version().clone(),
    })
}

/// Copies `current_exe` into `run_dir/dblocal-updater.exe` so the detached
/// Windows helper runs from the private staging directory rather than from the
/// installed target binary being replaced by `msiexec` or zip swap.
fn stage_windows_helper_copy(current_exe: &Path, run_dir: &Path) -> UpdateResult<PathBuf> {
    let helper_dest = run_dir.join(STAGED_HELPER_EXE_NAME);
    std::fs::copy(current_exe, &helper_dest).map_err(|source| UpdateError::ApplyFailed {
        reason: format!(
            "could not copy updater helper to staging directory ({}): {source}",
            helper_dest.display()
        ),
    })?;
    Ok(helper_dest)
}

/// Removes leftover `run-*` staging directories on a later startup when no
/// update lock is held and the current process is not executing from inside one.
pub fn cleanup_stale_staging(staging_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(staging_dir) else {
        return;
    };
    let current_exe = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(&p).ok().or(Some(p)));
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("run-") {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if current_exe
            .as_ref()
            .is_some_and(|exe| exe.starts_with(&canon))
        {
            continue;
        }
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// Persists an update failure message in `staging_dir/last-error.txt` so the
/// next start (GUI or CLI) can surface it.
pub fn record_update_failure(staging_dir: &Path, message: &str) {
    let _ = std::fs::create_dir_all(staging_dir);
    let _ = std::fs::write(staging_dir.join(LAST_UPDATE_ERROR_FILE), message);
}

/// Reads and clears any persisted update failure message written by the detached
/// Windows helper.
pub fn take_last_update_failure(staging_dir: &Path) -> Option<String> {
    let path = staging_dir.join(LAST_UPDATE_ERROR_FILE);
    let contents = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn resolve_linux_data_home(override_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = override_dir {
        return Some(dir.to_path_buf());
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg));
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return Some(PathBuf::from(home).join(".local/share"));
    }
    None
}

fn install_desktop_and_icons(
    extracted: &ExtractedLinuxTarball,
    installed_bin: &Path,
    data_home: &Path,
    allow_create_for_override: bool,
) -> UpdateResult<()> {
    let Some(desktop_src) = &extracted.desktop_entry else {
        return Ok(());
    };
    let abs_bin = installed_bin.to_string_lossy();
    let path_safe = !abs_bin.is_empty()
        && abs_bin.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '+' | '@' | '~' | '-')
        });
    if !path_safe {
        tracing::warn!(
            path = %abs_bin,
            "skipping desktop entry update: binary path contains characters outside [A-Za-z0-9._/+@~-]"
        );
        return Ok(());
    }

    let apps_dir = data_home.join("applications");
    let desktop_dest = apps_dir.join("dev.dblocal.DBLocal.desktop");

    // Ownership guard:
    // - If a desktop entry already exists, only rewrite it when its Exec= points
    //   at THIS binary, so a second portable copy never hijacks the menu entry.
    // - If no desktop entry exists yet, only create one when installed where
    //   `packaging/install.sh` installs by default (`$HOME/.local/bin/dblocal`,
    //   or an explicit test override).
    if desktop_dest.is_file() {
        if !desktop_entry_points_to(&desktop_dest, installed_bin) {
            tracing::info!(
                desktop = %desktop_dest.display(),
                binary = %installed_bin.display(),
                "existing desktop entry points at a different binary; leaving it untouched"
            );
            return Ok(());
        }
    } else if !allow_create_for_override && !is_default_install_sh_location(installed_bin) {
        tracing::info!(
            binary = %installed_bin.display(),
            "portable copy is outside ~/.local/bin and no desktop entry exists; skipping menu entry creation"
        );
        return Ok(());
    }

    let raw_desktop =
        std::fs::read_to_string(desktop_src).map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not read extracted desktop entry: {source}"),
        })?;
    let mut rewritten = String::with_capacity(raw_desktop.len() + 64);
    for line in raw_desktop.lines() {
        if line.starts_with("Exec=") {
            rewritten.push_str("Exec=");
            rewritten.push_str(&abs_bin);
        } else if line.starts_with("TryExec=") {
            rewritten.push_str("TryExec=");
            rewritten.push_str(&abs_bin);
        } else {
            rewritten.push_str(line);
        }
        rewritten.push('\n');
    }

    atomic_write_file(&desktop_dest, rewritten.as_bytes(), 0o644)?;

    if let Some(svg_src) = &extracted.svg_icon {
        let bytes = std::fs::read(svg_src).map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not read extracted SVG icon: {source}"),
        })?;
        atomic_write_file(
            &data_home.join("icons/hicolor/scalable/apps/dblocal.svg"),
            &bytes,
            0o644,
        )?;
    }

    for (size, png_src) in &extracted.png_icons {
        let bytes = std::fs::read(png_src).map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not read extracted {size}x{size} PNG icon: {source}"),
        })?;
        let dest = data_home.join(format!("icons/hicolor/{size}x{size}/apps/dblocal.png"));
        atomic_write_file(&dest, &bytes, 0o644)?;
    }

    refresh_linux_desktop_caches(data_home, &apps_dir);
    Ok(())
}

fn desktop_entry_points_to(desktop_path: &Path, installed_bin: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(desktop_path) else {
        return false;
    };
    let expected =
        std::fs::canonicalize(installed_bin).unwrap_or_else(|_| installed_bin.to_path_buf());
    for line in contents.lines() {
        if let Some(exec_val) = line.strip_prefix("Exec=") {
            let token = exec_val
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_matches('"');
            if token.is_empty() {
                return false;
            }
            let exec_path = Path::new(token);
            let actual =
                std::fs::canonicalize(exec_path).unwrap_or_else(|_| exec_path.to_path_buf());
            return actual == expected;
        }
    }
    false
}

fn is_default_install_sh_location(installed_bin: &Path) -> bool {
    let Ok(home) = std::env::var("HOME") else {
        return false;
    };
    if home.is_empty() {
        return false;
    }
    let default_bin = PathBuf::from(home).join(".local/bin/dblocal");
    let expected = std::fs::canonicalize(&default_bin).unwrap_or(default_bin);
    let actual =
        std::fs::canonicalize(installed_bin).unwrap_or_else(|_| installed_bin.to_path_buf());
    actual == expected
}

fn atomic_write_file(dest: &Path, bytes: &[u8], mode: u32) -> UpdateResult<()> {
    let Some(parent) = dest.parent() else {
        return Err(UpdateError::ApplyFailed {
            reason: format!("invalid destination path: {}", dest.display()),
        });
    };
    std::fs::create_dir_all(parent).map_err(|source| UpdateError::ApplyFailed {
        reason: format!("could not create directory {}: {source}", parent.display()),
    })?;
    let tmp = sibling_temp_path(dest);
    let write_res = (|| -> UpdateResult<()> {
        std::fs::write(&tmp, bytes).map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not write {}: {source}", tmp.display()),
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)).map_err(
                |source| UpdateError::ApplyFailed {
                    reason: format!("could not chmod {}: {source}", tmp.display()),
                },
            )?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        fsync_path(&tmp)?;
        std::fs::rename(&tmp, dest).map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not place {}: {source}", dest.display()),
        })?;
        Ok(())
    })();
    if write_res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_res
}

fn refresh_linux_desktop_caches(data_home: &Path, apps_dir: &Path) {
    let hicolor = data_home.join("icons/hicolor");
    if hicolor.join("icon-theme.cache").is_file() {
        let _ = std::process::Command::new("gtk-update-icon-cache")
            .args(["-q", "-t", "-f"])
            .arg(&hicolor)
            .status();
    }
    let _ = std::process::Command::new("update-desktop-database")
        .arg("-q")
        .arg(apps_dir)
        .status();
}

/// Applies an install plan. Linux AppImage/tarball are applied for real;
/// Windows/macOS are refused at apply time with a clear plan-only error.
pub fn apply(plan: &InstallPlan) -> UpdateResult<()> {
    match plan.kind() {
        InstallKind::AppImage { .. } | InstallKind::PortableTarball { .. } => apply_linux(plan),
        InstallKind::WindowsZip { .. } | InstallKind::WindowsMsi { .. } => {
            Err(UpdateError::ApplyFailed {
                reason:
                    "applying a Windows update needs the runtime helper; use the planned commands"
                        .to_owned(),
            })
        }
        InstallKind::MacApp { .. } => Err(UpdateError::UnsupportedInstallKind {
            reason: "macOS self-update is not supported yet".to_owned(),
        }),
        _ => Err(UpdateError::UnsupportedInstallKind {
            reason: "this install kind does not support self-update".to_owned(),
        }),
    }
}

fn apply_linux(plan: &InstallPlan) -> UpdateResult<()> {
    let target = plan.target_path().to_path_buf();
    ensure_target_writable(&target)?;
    let backup = backup_path(&target);
    let temp = sibling_temp_path(&target);
    let mut created_backup = false;
    for step in plan.steps() {
        if let Err(err) = apply_step(step, &mut created_backup) {
            let _ = std::fs::remove_file(&temp);
            if created_backup {
                let _ = restore_backup(&backup, &target);
            }
            return Err(err);
        }
    }
    smoke_or_rollback(&target, &backup, plan.target_version())?;
    Ok(())
}

fn apply_step(step: &InstallStep, created_backup: &mut bool) -> UpdateResult<()> {
    match step {
        InstallStep::WriteNextToCurrent {
            src,
            dst,
            mode_from,
        } => {
            copy_with_mode(src, dst, mode_from)?;
            #[cfg(all(test, unix))]
            maybe_fail_after_write()?;
            Ok(())
        }
        InstallStep::Fsync { path } => fsync_path(path),
        InstallStep::Backup { target, backup } => {
            create_backup(target, backup)?;
            *created_backup = true;
            #[cfg(all(test, unix))]
            maybe_fail_after_backup()?;
            Ok(())
        }
        InstallStep::RenameOver { src, dst } => std::fs::rename(src, dst).map_err(|source| {
            let _ = std::fs::remove_file(src);
            UpdateError::ApplyFailed {
                reason: format!("could not replace the binary: {source}"),
            }
        }),
        InstallStep::KeepBackup { .. }
        | InstallStep::SpawnWindowsHelper { .. }
        | InstallStep::ReplaceMacBundle { .. }
        | InstallStep::Refuse { .. } => Ok(()),
    }
}

#[cfg(all(test, unix))]
fn maybe_fail_after_backup() -> UpdateResult<()> {
    FAIL_AFTER_BACKUP.with(|flag| {
        if flag.get() {
            flag.set(false);
            return Err(UpdateError::ApplyFailed {
                reason: "injected backup failure".to_owned(),
            });
        }
        Ok(())
    })
}

#[cfg(all(test, unix))]
fn maybe_fail_after_write() -> UpdateResult<()> {
    FAIL_AFTER_WRITE.with(|flag| {
        if flag.get() {
            flag.set(false);
            return Err(UpdateError::ApplyFailed {
                reason: "injected write failure".to_owned(),
            });
        }
        Ok(())
    })
}

#[cfg(all(test, unix))]
thread_local! {
    static FAIL_AFTER_BACKUP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_AFTER_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(all(test, unix))]
pub fn inject_fail_after_backup_for_test() {
    FAIL_AFTER_BACKUP.with(|flag| flag.set(true));
}

#[cfg(all(test, unix))]
pub fn clear_fail_after_backup_for_test() {
    FAIL_AFTER_BACKUP.with(|flag| flag.set(false));
}

#[cfg(all(test, unix))]
pub fn inject_fail_after_write_for_test() {
    FAIL_AFTER_WRITE.with(|flag| flag.set(true));
}

fn ensure_executable(path: &Path) -> UpdateResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)
            .map_err(|source| UpdateError::ApplyFailed {
                reason: format!("could not read staged file permissions: {source}"),
            })?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o755)).map_err(
                |source| UpdateError::ApplyFailed {
                    reason: format!("could not mark staged binary executable: {source}"),
                },
            )?;
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn copy_with_mode(src: &Path, dst: &Path, mode_source: &Path) -> UpdateResult<()> {
    std::fs::copy(src, dst).map_err(|source| UpdateError::ApplyFailed {
        reason: format!("could not copy the update file: {source}"),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(mode_source)
            .or_else(|_| std::fs::metadata(src))
            .map_err(|source| UpdateError::ApplyFailed {
                reason: format!("could not read the current file mode: {source}"),
            })?
            .permissions()
            .mode();
        std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode)).map_err(|source| {
            UpdateError::ApplyFailed {
                reason: format!("could not set the update file mode: {source}"),
            }
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = mode_source;
    }
    Ok(())
}

fn fsync_path(path: &Path) -> UpdateResult<()> {
    let file = std::fs::File::open(path).map_err(|source| UpdateError::ApplyFailed {
        reason: format!("could not open the file for fsync: {source}"),
    })?;
    file.sync_all().map_err(|source| UpdateError::ApplyFailed {
        reason: format!("could not fsync the update file: {source}"),
    })
}

fn create_backup(target: &Path, backup: &Path) -> UpdateResult<()> {
    if backup.exists() {
        std::fs::remove_file(backup).map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not remove the old backup: {source}"),
        })?;
    }
    if !target.exists() {
        return Ok(());
    }
    if std::fs::hard_link(target, backup).is_ok() {
        return Ok(());
    }
    copy_with_mode(target, backup, target)
}

fn smoke_or_rollback(target: &Path, backup: &Path, expected: &Version) -> UpdateResult<()> {
    match smoke_check(target, expected) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = restore_backup(backup, target);
            Err(UpdateError::ApplyFailed {
                reason: format!("smoke check failed, the old binary was restored: {err}"),
            })
        }
    }
}

/// Restores `backup` over `target`.
pub fn restore_backup(backup: &Path, target: &Path) -> UpdateResult<()> {
    if !backup.exists() {
        return Err(UpdateError::ApplyFailed {
            reason: "no backup of the old binary was found".to_owned(),
        });
    }
    let temp = sibling_temp_path(target);
    copy_with_mode(backup, &temp, backup)?;
    fsync_path(&temp)?;
    std::fs::rename(&temp, target).map_err(|source| {
        let _ = std::fs::remove_file(&temp);
        UpdateError::ApplyFailed {
            reason: format!("could not restore the backup: {source}"),
        }
    })
}

/// Removes a leftover `.old` backup after a successful start.
pub fn cleanup_backup(current_exe: &Path) -> UpdateResult<()> {
    let backup = backup_path(current_exe);
    if backup.exists() {
        std::fs::remove_file(&backup).map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not remove the update backup: {source}"),
        })?;
    }
    Ok(())
}

/// Spawns the updated binary (or detached Windows helper) so the caller can exit
/// cleanly without signalling or disrupting any running database servers.
pub fn execute_restart(action: &RestartAction) -> UpdateResult<()> {
    let mut cmd = match action {
        RestartAction::Relaunch { exe_path } => std::process::Command::new(exe_path),
        RestartAction::RunWindowsMsi {
            helper_exe,
            msi_path,
            relaunch_exe,
        } => {
            let mut c = std::process::Command::new(helper_exe);
            c.args([
                crate::consts::APPLY_UPDATE_FLAG,
                "--pid",
                &std::process::id().to_string(),
                "--msi",
            ])
            .arg(msi_path)
            .arg("--target")
            .arg(relaunch_exe);
            if let Some(error_file) = helper_error_file_for(helper_exe) {
                c.arg("--error-file").arg(error_file);
            }
            c
        }
        RestartAction::RunWindowsZipHelper {
            helper_exe,
            staged_exe,
            target_exe,
            version,
        } => {
            let mut c = std::process::Command::new(helper_exe);
            c.args([
                crate::consts::APPLY_UPDATE_FLAG,
                "--pid",
                &std::process::id().to_string(),
                "--staged",
            ])
            .arg(staged_exe)
            .arg("--target")
            .arg(target_exe)
            .arg("--version")
            .arg(version.to_string());
            if let Some(error_file) = helper_error_file_for(helper_exe) {
                c.arg("--error-file").arg(error_file);
            }
            c
        }
    };

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    cmd.spawn().map_err(|source| UpdateError::ApplyFailed {
        reason: format!("could not launch the updated application: {source}"),
    })?;
    Ok(())
}

/// Derives `<staging_dir>/last-error.txt` from a staged helper path
/// `<staging_dir>/run-.../dblocal-updater.exe`.
fn helper_error_file_for(helper_exe: &Path) -> Option<PathBuf> {
    let run_dir = helper_exe.parent()?;
    let staging_dir = run_dir.parent()?;
    Some(staging_dir.join(LAST_UPDATE_ERROR_FILE))
}

/// Parsed arguments for the detached Windows `--apply-update` helper mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsHelperArgs {
    /// Parent process ID to wait for before replacing/upgrading.
    pub pid: u32,
    /// Verified `.msi` installer path (mutually exclusive with `staged`).
    pub msi: Option<PathBuf>,
    /// Verified extracted `dblocal.exe` path (mutually exclusive with `msi`).
    pub staged: Option<PathBuf>,
    /// Installed executable path to replace and/or relaunch.
    pub target: PathBuf,
    /// Expected version string (for zip swap smoke check).
    pub version: Option<String>,
    /// Optional path where a failure message is persisted for the next start.
    pub error_file: Option<PathBuf>,
}

/// Executes the detached post-exit helper (`dblocal --apply-update ...`).
///
/// Waits up to 60 seconds for `args.pid` to exit; if it does not exit in time,
/// aborts without modifying files and persists the error. Otherwise runs
/// `msiexec.exe /i <msi> /passive` (accepting exit 0 and 3010 reboot-required)
/// or swaps `staged` over `target` with backup + rollback. On failure, persists
/// the error and relaunches the previous app so the user is never left without
/// DBLocal or feedback.
pub fn run_windows_apply_helper(args: &WindowsHelperArgs) -> UpdateResult<()> {
    run_windows_apply_helper_with(
        args,
        |pid| wait_for_pid_exit(pid, WINDOWS_HELPER_WAIT_TIMEOUT),
        run_msiexec_command,
        |target| {
            execute_restart(&RestartAction::Relaunch {
                exe_path: target.to_path_buf(),
            })
        },
    )
}

fn run_msiexec_command(msi: &Path) -> UpdateResult<Option<i32>> {
    let status = std::process::Command::new("msiexec.exe")
        .arg("/i")
        .arg(msi)
        .arg("/passive")
        .status()
        .map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not run msiexec.exe: {source}"),
        })?;
    Ok(status.code())
}

pub(crate) fn run_windows_apply_helper_with(
    args: &WindowsHelperArgs,
    wait_fn: impl FnOnce(u32) -> bool,
    msi_runner: impl FnOnce(&Path) -> UpdateResult<Option<i32>>,
    relaunch_fn: impl Fn(&Path) -> UpdateResult<()>,
) -> UpdateResult<()> {
    if !wait_fn(args.pid) {
        let reason = format!(
            "previous DBLocal process (PID {}) did not exit within {}s; update aborted",
            args.pid,
            WINDOWS_HELPER_WAIT_TIMEOUT.as_secs()
        );
        persist_helper_error(args, &reason);
        return Err(UpdateError::ApplyFailed { reason });
    }

    let apply_result = if let Some(msi) = &args.msi {
        match msi_runner(msi) {
            Ok(Some(0)) => Ok(()),
            Ok(Some(MSI_EXIT_REBOOT_REQUIRED)) => {
                tracing::info!("msiexec exited with 3010 (success, reboot required)");
                Ok(())
            }
            Ok(code) => Err(UpdateError::ApplyFailed {
                reason: format!("MSI installer failed with exit code {code:?}"),
            }),
            Err(err) => Err(err),
        }
    } else if let Some(staged) = &args.staged {
        apply_windows_zip_swap(staged, &args.target, args.version.as_deref())
    } else {
        Err(UpdateError::Malformed {
            reason: "--apply-update requires either --msi or --staged".to_owned(),
        })
    };

    if let Err(err) = apply_result {
        persist_helper_error(args, &err.to_string());
        if args.target.is_file() {
            let _ = relaunch_fn(&args.target);
        }
        return Err(err);
    }

    if args.target.is_file() {
        let _ = relaunch_fn(&args.target);
    }
    Ok(())
}

fn apply_windows_zip_swap(staged: &Path, target: &Path, version: Option<&str>) -> UpdateResult<()> {
    let backup = backup_path(target);
    create_backup(target, &backup)?;
    if let Err(err) = copy_with_mode(staged, target, target) {
        let _ = restore_backup(&backup, target);
        return Err(err);
    }
    if let Some(ver_str) = version {
        let ver = Version::parse(ver_str).map_err(|err| UpdateError::Malformed {
            reason: format!("invalid expected version '{ver_str}': {err}"),
        })?;
        smoke_or_rollback(target, &backup, &ver)?;
    }
    Ok(())
}

fn persist_helper_error(args: &WindowsHelperArgs, message: &str) {
    if let Some(error_file) = &args.error_file {
        if let Some(parent) = error_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(error_file, message);
    }
}

/// Waits up to `timeout` for `pid` to disappear from the process table.
/// Returns `true` if `pid` exited (or `pid == 0`), `false` if it is still running.
fn wait_for_pid_exit(pid: u32, timeout: Duration) -> bool {
    if pid == 0 {
        return true;
    }
    let start = std::time::Instant::now();
    let target = sysinfo::Pid::from_u32(pid);
    let Ok(mut sys) = sysinfo::System::new() else {
        return false;
    };
    while start.elapsed() < timeout {
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[target]), true);
        if sys.process(target).is_none() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Runs `--version` on the installed binary and checks it reports expected version.
pub fn smoke_check(binary: &Path, expected: &Version) -> UpdateResult<()> {
    let mut cmd = std::process::Command::new(binary);
    cmd.arg("--version");
    let output = run_with_timeout(cmd, SMOKE_TIMEOUT, "smoke check")?;
    if !output.status.success() {
        return Err(UpdateError::ApplyFailed {
            reason: format!("smoke check exited with {}", output.status),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = format!("{stdout}\n{stderr}");
    if !version_reported(&text, expected) {
        return Err(UpdateError::ApplyFailed {
            reason: format!("the smoke check did not report version {}", expected),
        });
    }
    Ok(())
}

fn version_reported(output: &str, expected: &Version) -> bool {
    let expected = expected.to_string();
    output.split_whitespace().any(|token| {
        let token = token.trim_matches(|c: char| {
            !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+')
        });
        let token = token.strip_prefix('v').unwrap_or(token);
        token == expected
    })
}

fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: Duration,
    label: &str,
) -> UpdateResult<std::process::Output> {
    use std::process::Stdio;
    use std::time::Instant;

    let spawn_deadline = Instant::now() + EXEC_BUSY_RETRY_WINDOW;
    let mut child = loop {
        match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
            Ok(child) => break child,
            Err(source)
                if crate::fsutil::is_text_file_busy(&source) && Instant::now() < spawn_deadline =>
            {
                std::thread::sleep(EXEC_BUSY_RETRY_DELAY);
            }
            Err(source) => {
                return Err(UpdateError::ApplyFailed {
                    reason: format!("could not run {label}: {source}"),
                });
            }
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_rx = spawn_reader(stdout);
    let stderr_rx = spawn_reader(stderr);
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|source| UpdateError::ApplyFailed {
                reason: format!("could not wait for {label}: {source}"),
            })?
        {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(UpdateError::ApplyFailed {
                reason: format!("{label} timed out after {} seconds", timeout.as_secs()),
            });
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let stdout = stdout_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    let stderr = stderr_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

fn spawn_reader<R: std::io::Read + Send + 'static>(
    reader: Option<R>,
) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = reader {
            let _ = r.read_to_end(&mut buf);
        }
        let _ = tx.send(buf);
    });
    rx
}

#[cfg(test)]
#[path = "install/tests.rs"]
mod tests;
