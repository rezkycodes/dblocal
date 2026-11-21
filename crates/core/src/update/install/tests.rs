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
    assert!(matches!(plan.steps()[2], InstallStep::RenameOver { .. }));
    assert!(matches!(plan.steps()[3], InstallStep::KeepBackup { .. }));
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
