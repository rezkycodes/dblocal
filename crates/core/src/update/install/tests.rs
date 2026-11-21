#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use semver::Version;
use tempfile::tempdir;

use super::*;
use crate::update::kind::PackageManagerHint;

fn ver(s: &str) -> Version {
    Version::parse(s).unwrap()
}

#[test]
fn windows_helper_command_shape() {
    let cmd = windows_helper_command(
        Path::new("C:\\dblocal.exe"),
        Path::new("C:\\staged.zip"),
        42,
        &ver("1.2.3"),
    );
    assert_eq!(cmd[1], "--apply-update");
    assert!(cmd.contains(&"--pid".to_owned()));
    assert!(cmd.contains(&"42".to_owned()));
    assert!(cmd.contains(&"--staged".to_owned()));
    assert!(cmd.contains(&"--target".to_owned()));
    assert!(cmd.contains(&"--version".to_owned()));
    assert!(cmd.contains(&"1.2.3".to_owned()));
}

#[test]
fn plan_linux_appimage_has_replace_steps() {
    let plan = plan_install(
        &InstallKind::AppImage {
            appimage_path: PathBuf::from("/tmp/DBLocal.AppImage"),
        },
        Path::new("/tmp/staged"),
        Path::new("/tmp/DBLocal.AppImage"),
        &ver("1.2.0"),
    )
    .unwrap();
    assert!(matches!(
        plan.steps()[0],
        InstallStep::WriteNextToCurrent { .. }
    ));
    assert!(matches!(plan.steps()[1], InstallStep::Fsync { .. }));
    assert!(matches!(plan.steps()[2], InstallStep::Backup { .. }));
    assert!(matches!(plan.steps()[3], InstallStep::RenameOver { .. }));
    assert!(matches!(plan.steps()[4], InstallStep::KeepBackup { .. }));
}

#[test]
fn plan_windows_zip_is_helper_command() {
    let plan = plan_install(
        &InstallKind::WindowsZip {
            exe_path: PathBuf::from("C:\\dblocal.exe"),
        },
        Path::new("C:\\staged.zip"),
        Path::new("C:\\dblocal.exe"),
        &ver("1.2.0"),
    )
    .unwrap();
    assert!(matches!(
        plan.steps()[0],
        InstallStep::SpawnWindowsHelper { .. }
    ));
}

#[test]
fn plan_mac_is_plan_only() {
    let plan = plan_install(
        &InstallKind::MacApp {
            bundle_path: PathBuf::from("/Applications/DBLocal.app"),
        },
        Path::new("/tmp/staged.dmg"),
        Path::new("/Applications/DBLocal.app/Contents/MacOS/dblocal"),
        &ver("1.2.0"),
    )
    .unwrap();
    assert!(matches!(
        plan.steps()[0],
        InstallStep::ReplaceMacBundle { .. }
    ));
}

#[test]
fn plan_dist_package_is_refused() {
    let plan = plan_install(
        &InstallKind::DistPackage {
            exe_path: PathBuf::from("/usr/bin/dblocal"),
            hint: PackageManagerHint::Dpkg,
        },
        Path::new("/tmp/staged"),
        Path::new("/usr/bin/dblocal"),
        &ver("1.2.0"),
    )
    .unwrap();
    match &plan.steps()[0] {
        InstallStep::Refuse { reason } => assert!(reason.contains("dpkg/apt")),
        other => panic!("unexpected {other:?}"),
    }
}

#[cfg(unix)]
fn write_fake_binary(path: &Path, version: &str, exit_ok: bool) {
    let status = if exit_ok { 0 } else { 1 };
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo dblocal {version}; exit {status}; fi\nexit 1\n"
    );
    fs::write(path, script).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[cfg(unix)]
#[test]
fn apply_linux_replaces_binary_and_keeps_backup() {
    let dir = tempdir().unwrap();
    let current = dir.path().join("dblocal");
    let staged = dir.path().join("staged");
    write_fake_binary(&current, "1.0.0", true);
    write_fake_binary(&staged, "1.1.0", true);
    let plan = plan_install(
        &InstallKind::PortableTarball {
            exe_path: current.clone(),
        },
        &staged,
        &current,
        &ver("1.1.0"),
    )
    .unwrap();
    apply(&plan).expect("apply");
    let out = std::process::Command::new(&current)
        .arg("--version")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("1.1.0"));
    let backup = dir.path().join("dblocal.old");
    assert!(backup.exists(), "backup should exist");
}

#[cfg(unix)]
#[test]
fn apply_linux_rolls_back_on_smoke_failure() {
    let dir = tempdir().unwrap();
    let current = dir.path().join("dblocal");
    let staged = dir.path().join("staged");
    write_fake_binary(&current, "1.0.0", true);
    write_fake_binary(&staged, "1.1.0", false);
    let plan = plan_install(
        &InstallKind::PortableTarball {
            exe_path: current.clone(),
        },
        &staged,
        &current,
        &ver("1.1.0"),
    )
    .unwrap();
    let err = apply(&plan).expect_err("smoke failure");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
    let out = std::process::Command::new(&current)
        .arg("--version")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("1.0.0"), "old binary restored");
}

#[cfg(unix)]
#[test]
fn smoke_check_rejects_wrong_version() {
    let dir = tempdir().unwrap();
    let bin = dir.path().join("dblocal");
    write_fake_binary(&bin, "1.0.0", true);
    let err = smoke_check(&bin, &ver("2.0.0")).expect_err("wrong version");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
}

#[cfg(unix)]
#[test]
fn smoke_check_rejects_ambiguous_substring() {
    let dir = tempdir().unwrap();
    let bin = dir.path().join("dblocal");
    write_fake_binary(&bin, "11.2.30", true);
    let err = smoke_check(&bin, &ver("1.2.3")).expect_err("ambiguous");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
}

#[cfg(unix)]
#[test]
fn smoke_check_timeout_triggers_error() {
    let dir = tempdir().unwrap();
    let bin = dir.path().join("dblocal-slow");
    let script = "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then sleep 30; echo dblocal 1.1.0; exit 0; fi\nexit 1\n";
    std::fs::write(&bin, script).unwrap();
    let mut perms = std::fs::metadata(&bin).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&bin, perms).unwrap();
    let start = std::time::Instant::now();
    let err = smoke_check(&bin, &ver("1.1.0")).expect_err("timeout");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
    assert!(start.elapsed() < std::time::Duration::from_secs(15));
}

#[cfg(unix)]
#[test]
fn apply_linux_appimage_replaces_appimage_target_not_current_exe() {
    let dir = tempdir().unwrap();
    let appimage = dir.path().join("DBLocal.AppImage");
    let current_exe = dir.path().join("mount/dblocal");
    std::fs::create_dir_all(current_exe.parent().unwrap()).unwrap();
    write_fake_binary(&appimage, "1.0.0", true);
    write_fake_binary(&current_exe, "1.0.0", true);
    let staged = dir.path().join("staged");
    write_fake_binary(&staged, "9.9.9", true);
    let plan = plan_install(
        &InstallKind::AppImage {
            appimage_path: appimage.clone(),
        },
        &staged,
        &current_exe,
        &ver("9.9.9"),
    )
    .unwrap();
    apply(&plan).expect("apply");
    let app_out = std::process::Command::new(&appimage)
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&app_out.stdout).contains("9.9.9"));
    let exe_out = std::process::Command::new(&current_exe)
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&exe_out.stdout).contains("1.0.0"));
}

#[cfg(unix)]
#[test]
fn apply_preserves_symlinked_executable() {
    let dir = tempdir().unwrap();
    let real = dir.path().join("dblocal-real");
    let link = dir.path().join("dblocal-link");
    write_fake_binary(&real, "1.0.0", true);
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let staged = dir.path().join("staged");
    write_fake_binary(&staged, "1.1.0", true);
    let plan = plan_install(
        &InstallKind::PortableTarball {
            exe_path: link.clone(),
        },
        &staged,
        &link,
        &ver("1.1.0"),
    )
    .unwrap();
    apply(&plan).expect("apply");
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    let real_out = std::process::Command::new(&real)
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&real_out.stdout).contains("1.1.0"));
}

#[cfg(unix)]
#[test]
fn apply_continuity_while_replacing() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let dir = tempdir().unwrap();
    let current = dir.path().join("dblocal");
    write_fake_binary(&current, "1.0.0", true);
    let missing = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let target = Arc::new(current.clone());
    let missing_t = missing.clone();
    let stop_t = stop.clone();
    let target_t = target.clone();
    let reader = std::thread::spawn(move || {
        while !stop_t.load(Ordering::SeqCst) {
            if !target_t.exists() {
                missing_t.store(true, Ordering::SeqCst);
                break;
            }
            std::thread::yield_now();
        }
    });
    for i in 0..200 {
        let ver_s = format!("1.0.{}", i + 1);
        let staged = dir.path().join(format!("staged-{i}"));
        write_fake_binary(&staged, &ver_s, true);
        let plan = plan_install(
            &InstallKind::PortableTarball {
                exe_path: current.clone(),
            },
            &staged,
            &current,
            &Version::parse(&ver_s).unwrap(),
        )
        .unwrap();
        apply(&plan).expect("apply");
    }
    stop.store(true, Ordering::SeqCst);
    reader.join().unwrap();
    assert!(!missing.load(Ordering::SeqCst), "target went missing during swap");
}

#[cfg(unix)]
#[test]
fn apply_failure_after_backup_keeps_target() {
    let dir = tempdir().unwrap();
    let current = dir.path().join("dblocal");
    let staged = dir.path().join("staged");
    write_fake_binary(&current, "1.0.0", true);
    write_fake_binary(&staged, "1.1.0", true);
    let plan = plan_install(
        &InstallKind::PortableTarball {
            exe_path: current.clone(),
        },
        &staged,
        &current,
        &ver("1.1.0"),
    )
    .unwrap();
    super::inject_fail_after_backup_for_test();
    let err = apply(&plan).expect_err("injected");
    super::clear_fail_after_backup_for_test();
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
    let out = std::process::Command::new(&current)
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("1.0.0"));
}

#[cfg(unix)]
#[test]
fn cleanup_backup_removes_old_file() {
    let dir = tempdir().unwrap();
    let current = dir.path().join("dblocal");
    let backup = dir.path().join("dblocal.old");
    write_fake_binary(&current, "1.0.0", true);
    std::fs::write(&backup, b"old").unwrap();
    cleanup_backup(&current).unwrap();
    assert!(!backup.exists());
}
