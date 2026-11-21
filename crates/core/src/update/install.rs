//! Install planning and application for self-updates.
//!
//! Planning is pure: it produces an [`InstallPlan`] describing steps. Applying
//! executes the plan. Windows/macOS apply are represented as plans; Linux apply
//! is implemented for AppImage/tarball.

use std::path::{Path, PathBuf};

use semver::Version;

use super::error::{UpdateError, UpdateResult};
use super::kind::InstallKind;

/// A pure, serializable-ish description of how to apply an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    kind: InstallKind,
    target_version: Version,
    staged_file: PathBuf,
    current_exe: PathBuf,
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

    /// Planned steps.
    pub fn steps(&self) -> &[InstallStep] {
        &self.steps
    }
}

/// One step in an install plan.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstallStep {
    /// Write staged file next to current binary with same mode.
    WriteNextToCurrent { src: PathBuf, dst: PathBuf },
    /// fsync the written file.
    Fsync { path: PathBuf },
    /// Atomically rename over current binary.
    RenameOver { src: PathBuf, dst: PathBuf },
    /// Keep backup until next successful start.
    KeepBackup { backup: PathBuf },
    /// Spawn detached Windows helper to swap after exit.
    SpawnWindowsHelper { command: Vec<String> },
    /// macOS bundle replacement plan (not executed).
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
        let reason = kind
            .package_manager_hint()
            .map(|h| h.message())
            .unwrap_or_else(|| {
                "Pembaruan otomatis tidak tersedia untuk cara pemasangan ini.".to_owned()
            });
        return Ok(InstallPlan {
            kind: kind.clone(),
            target_version: target_version.clone(),
            staged_file: staged_file.to_path_buf(),
            current_exe: current_exe.to_path_buf(),
            steps: vec![InstallStep::Refuse { reason }],
        });
    }
    let steps = match kind {
        InstallKind::AppImage { appimage_path } => plan_linux_replace(staged_file, appimage_path),
        InstallKind::PortableTarball { exe_path } => plan_linux_replace(staged_file, exe_path),
        InstallKind::WindowsZip { exe_path } => {
            plan_windows_helper(staged_file, exe_path, target_version)
        }
        InstallKind::MacApp { bundle_path } => vec![InstallStep::ReplaceMacBundle {
            bundle: bundle_path.clone(),
            staged: staged_file.to_path_buf(),
        }],
        InstallKind::WindowsMsi { .. }
        | InstallKind::DistPackage { .. }
        | InstallKind::Homebrew { .. }
        | InstallKind::Unknown { .. } => {
            return Err(UpdateError::UnsupportedInstallKind {
                reason: "jenis instalasi tidak mendukung self-update".to_owned(),
            });
        }
    };
    Ok(InstallPlan {
        kind: kind.clone(),
        target_version: target_version.clone(),
        staged_file: staged_file.to_path_buf(),
        current_exe: current_exe.to_path_buf(),
        steps,
    })
}

fn plan_linux_replace(staged_file: &Path, target: &Path) -> Vec<InstallStep> {
    let dst = target.to_path_buf();
    let tmp = sibling_temp_path(target);
    let backup = backup_path(target);
    vec![
        InstallStep::WriteNextToCurrent {
            src: staged_file.to_path_buf(),
            dst: tmp.clone(),
        },
        InstallStep::Fsync { path: tmp.clone() },
        InstallStep::RenameOver { src: tmp, dst },
        InstallStep::KeepBackup { backup },
    ]
}

fn plan_windows_helper(staged_file: &Path, exe_path: &Path, target: &Version) -> Vec<InstallStep> {
    let cmd = windows_helper_command(exe_path, staged_file, std::process::id(), target);
    vec![InstallStep::SpawnWindowsHelper { command: cmd }]
}

/// Builds the command line for the Windows swap helper.
///
/// The helper is a mode of the `dblocal` binary (`--apply-update ...`); CLI
/// wiring is owned by the CLI worker. This function only constructs args.
pub fn windows_helper_command(
    current_exe: &Path,
    staged_file: &Path,
    pid: u32,
    target_version: &Version,
) -> Vec<String> {
    vec![
        current_exe.display().to_string(),
        "--apply-update".to_owned(),
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

fn sibling_temp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "dblocal".to_owned());
    target.with_file_name(format!(".{name}.update-{}", std::process::id()))
}

fn backup_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "dblocal".to_owned());
    target.with_file_name(format!("{name}.old"))
}

/// Applies an install plan. Linux AppImage/tarball are applied for real;
/// Windows/macOS are refused at apply time with a clear plan-only error.
pub fn apply(plan: &InstallPlan) -> UpdateResult<()> {
    match plan.kind() {
        InstallKind::AppImage { .. } | InstallKind::PortableTarball { .. } => {
            apply_linux(plan)
        }
        InstallKind::WindowsZip { .. } => Err(UpdateError::ApplyFailed {
            reason: "penerapan update Windows membutuhkan helper runtime; gunakan command yang direncanakan".to_owned(),
        }),
        InstallKind::MacApp { .. } => Err(UpdateError::ApplyFailed {
            reason: "penerapan update macOS belum dieksekusi di modul ini; gunakan install plan".to_owned(),
        }),
        _ => Err(UpdateError::UnsupportedInstallKind {
            reason: "jenis instalasi tidak mendukung self-update".to_owned(),
        }),
    }
}

fn apply_linux(plan: &InstallPlan) -> UpdateResult<()> {
    let target = plan.current_exe();
    let backup = backup_path(target);
    let temp = sibling_temp_path(target);
    copy_with_mode(plan.staged_file(), &temp, target)?;
    fsync_path(&temp)?;
    swap_with_backup(&temp, target, &backup)?;
    smoke_or_rollback(target, &backup, plan.target_version())?;
    Ok(())
}

fn copy_with_mode(src: &Path, dst: &Path, mode_source: &Path) -> UpdateResult<()> {
    std::fs::copy(src, dst).map_err(|source| UpdateError::ApplyFailed {
        reason: format!("gagal menyalin file update: {source}"),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(mode_source)
            .map_err(|source| UpdateError::ApplyFailed {
                reason: format!("gagal membaca mode file saat ini: {source}"),
            })?
            .permissions()
            .mode();
        std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode)).map_err(|source| {
            UpdateError::ApplyFailed {
                reason: format!("gagal mengatur mode file update: {source}"),
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
        reason: format!("gagal membuka file untuk fsync: {source}"),
    })?;
    file.sync_all().map_err(|source| UpdateError::ApplyFailed {
        reason: format!("gagal fsync file update: {source}"),
    })
}

fn swap_with_backup(temp: &Path, target: &Path, backup: &Path) -> UpdateResult<()> {
    if backup.exists() {
        let _ = std::fs::remove_file(backup);
    }
    if target.exists() {
        std::fs::rename(target, backup).map_err(|source| UpdateError::ApplyFailed {
            reason: format!("gagal membuat backup binary lama: {source}"),
        })?;
    }
    std::fs::rename(temp, target).map_err(|source| {
        let _ = restore_backup(backup, target);
        UpdateError::ApplyFailed {
            reason: format!("gagal mengganti binary: {source}"),
        }
    })
}

fn smoke_or_rollback(target: &Path, backup: &Path, expected: &Version) -> UpdateResult<()> {
    match smoke_check(target, expected) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = restore_backup(backup, target);
            Err(UpdateError::ApplyFailed {
                reason: format!("smoke check gagal, binary lama dipulihkan: {err}"),
            })
        }
    }
}

fn restore_backup(backup: &Path, target: &Path) -> UpdateResult<()> {
    if !backup.exists() {
        return Err(UpdateError::ApplyFailed {
            reason: "backup binary lama tidak ditemukan".to_owned(),
        });
    }
    if target.exists() {
        let _ = std::fs::remove_file(target);
    }
    std::fs::rename(backup, target).map_err(|source| UpdateError::ApplyFailed {
        reason: format!("gagal memulihkan backup: {source}"),
    })
}

/// Runs `--version` on the installed binary and checks it reports expected version.
///
/// This is the rollback smoke check. It executes a local binary path from the
/// install plan, not from release metadata.
pub fn smoke_check(binary: &Path, expected: &Version) -> UpdateResult<()> {
    let output = std::process::Command::new(binary)
        .arg("--version")
        .output()
        .map_err(|source| UpdateError::ApplyFailed {
            reason: format!("gagal menjalankan smoke check: {source}"),
        })?;
    if !output.status.success() {
        return Err(UpdateError::ApplyFailed {
            reason: format!("smoke check exited with {}", output.status),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = format!("{stdout}\n{stderr}");
    if !text.contains(&expected.to_string()) {
        return Err(UpdateError::ApplyFailed {
            reason: format!("smoke check tidak melaporkan versi {}", expected),
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "install/tests.rs"]
mod tests;
