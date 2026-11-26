//! Install planning and application for self-updates.
//!
//! Planning is pure: it produces an [`InstallPlan`] describing steps. Applying
//! executes the plan. Windows/macOS apply are represented as plans; Linux apply
//! is implemented for AppImage/tarball.

use std::path::{Path, PathBuf};
use std::time::Duration;

use semver::Version;

use super::error::{UpdateError, UpdateResult};
use super::kind::InstallKind;

const SMOKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to keep retrying a spawn that failed with ETXTBSY.
const EXEC_BUSY_RETRY_WINDOW: Duration = Duration::from_secs(5);
/// Delay between ETXTBSY spawn retries.
const EXEC_BUSY_RETRY_DELAY: Duration = Duration::from_millis(20);

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
        InstallKind::MacApp { bundle_path } => vec![InstallStep::ReplaceMacBundle {
            bundle: bundle_path.clone(),
            staged: staged_file.to_path_buf(),
        }],
        InstallKind::WindowsMsi { .. }
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
        InstallKind::AppImage { .. } | InstallKind::PortableTarball { .. } => apply_linux(plan),
        InstallKind::WindowsZip { .. } => Err(UpdateError::ApplyFailed {
            reason: "applying a Windows update needs the runtime helper; use the planned commands"
                .to_owned(),
        }),
        InstallKind::MacApp { .. } => Err(UpdateError::ApplyFailed {
            reason: "macOS update application is not executed in this module; use an install plan"
                .to_owned(),
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
            // A failure before the rename leaves our staged sibling behind; the
            // install directory must look untouched to the user.
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

/// Restores `backup` over `target`. Kept for the "next successful start"
/// recovery flow (wiring owned by CLI/GUI later).
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

/// Runs `--version` on the installed binary and checks it reports expected version.
///
/// This is the rollback smoke check. It executes a local binary path from the
/// install plan, not from release metadata.
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

    // A concurrent fork in the same process can briefly keep a write descriptor
    // to the freshly written binary open, so exec fails with ETXTBSY (26).
    // Retry within a bounded window instead of failing the whole update.
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
