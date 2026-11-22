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
fn version_output(path: &Path) -> std::process::Output {
    // A sibling test thread may fork while a write fd to this file is open, so
    // exec can transiently return ETXTBSY (code 26). Retry like production does.
    for attempt in 0..10 {
        match std::process::Command::new(path).arg("--version").output() {
            Ok(out) => return out,
            Err(err) if err.raw_os_error() == Some(26) && attempt < 9 => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(err) => panic!("failed to run {}: {err}", path.display()),
        }
    }
    unreachable!("loop always returns or panics")
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
    let out = version_output(&current);
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
    let out = version_output(&current);
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
    let app_out = version_output(&appimage);
    assert!(String::from_utf8_lossy(&app_out.stdout).contains("9.9.9"));
    let exe_out = version_output(&current_exe);
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
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let real_out = version_output(&real);
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
    assert!(
        !missing.load(Ordering::SeqCst),
        "target went missing during swap"
    );
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
    let out = version_output(&current);
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

#[test]
fn plan_accessors_and_apply_refusals() {
    let plan = plan_install(
        &InstallKind::PortableTarball {
            exe_path: PathBuf::from("/tmp/db-local-example/dblocal"),
        },
        Path::new("/tmp/staged"),
        Path::new("/tmp/db-local-example/dblocal"),
        &ver("1.2.0"),
    )
    .unwrap();
    assert_eq!(plan.staged_file(), Path::new("/tmp/staged"));
    assert_eq!(
        plan.current_exe(),
        Path::new("/tmp/db-local-example/dblocal")
    );
    assert_eq!(plan.target_version(), &ver("1.2.0"));

    let win = plan_install(
        &InstallKind::WindowsZip {
            exe_path: PathBuf::from("C:\\dblocal.exe"),
        },
        Path::new("C:\\staged.zip"),
        Path::new("C:\\dblocal.exe"),
        &ver("1.2.0"),
    )
    .unwrap();
    let err = apply(&win).expect_err("windows apply refused");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);

    let mac = plan_install(
        &InstallKind::MacApp {
            bundle_path: PathBuf::from("/Applications/DBLocal.app"),
        },
        Path::new("/tmp/staged.dmg"),
        Path::new("/Applications/DBLocal.app/Contents/MacOS/dblocal"),
        &ver("1.2.0"),
    )
    .unwrap();
    let err = apply(&mac).expect_err("mac apply refused");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
}

#[test]
fn apply_refuses_non_installable_kind() {
    let mut plan = plan_install(
        &InstallKind::PortableTarball {
            exe_path: PathBuf::from("/tmp/db-local-example/dblocal"),
        },
        Path::new("/tmp/staged"),
        Path::new("/tmp/db-local-example/dblocal"),
        &ver("1.2.0"),
    )
    .unwrap();
    plan = InstallPlan {
        kind: InstallKind::Homebrew {
            exe_path: PathBuf::from("/opt/homebrew/bin/dblocal"),
        },
        ..plan
    };
    let err = apply(&plan).expect_err("homebrew refused");
    assert_eq!(
        err.code(),
        crate::update::error::CODE_UPDATE_UNSUPPORTED_KIND
    );
}

#[test]
fn restore_backup_requires_existing_backup() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("dblocal");
    let missing = dir.path().join("dblocal.old");
    let err = restore_backup(&missing, &target).expect_err("missing backup");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
}

#[cfg(unix)]
#[test]
fn restore_backup_renames_over_target() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("dblocal");
    let backup = dir.path().join("dblocal.old");
    write_fake_binary(&target, "2.0.0", true);
    write_fake_binary(&backup, "1.0.0", true);
    restore_backup(&backup, &target).expect("restore");
    let out = version_output(&target);
    assert!(String::from_utf8_lossy(&out.stdout).contains("1.0.0"));
}

#[cfg(unix)]
#[test]
fn create_backup_copies_when_target_missing_and_refreshes_existing() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("dblocal");
    let backup = dir.path().join("dblocal.old");
    create_backup(&target, &backup).expect("no target yet");
    assert!(!backup.exists());

    write_fake_binary(&target, "1.0.0", true);
    write_fake_binary(&backup, "0.9.0", true);
    create_backup(&target, &backup).expect("refresh");
    let out = version_output(&backup);
    assert!(String::from_utf8_lossy(&out.stdout).contains("1.0.0"));
}

#[test]
fn fsync_path_reports_missing_file() {
    let dir = tempdir().unwrap();
    let err = fsync_path(&dir.path().join("nope")).expect_err("missing");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
}

#[test]
fn ensure_target_writable_rejects_relative_target_without_parent() {
    let err = ensure_target_writable(Path::new("dblocal")).expect_err("no parent");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
}

#[test]
fn plan_windows_msi_is_refused_with_generic_reason() {
    // WindowsMsi cannot self-update and has no package-manager hint, so the plan
    // falls back to the generic Indonesian refusal instead of an empty reason.
    let plan = plan_install(
        &InstallKind::WindowsMsi {
            exe_path: PathBuf::from("C:\\Program Files\\DBLocal\\dblocal.exe"),
        },
        Path::new("C:\\staged.zip"),
        Path::new("C:\\Program Files\\DBLocal\\dblocal.exe"),
        &ver("1.2.0"),
    )
    .unwrap();
    match &plan.steps()[0] {
        InstallStep::Refuse { reason } => {
            assert_eq!(
                reason,
                "Pembaruan otomatis tidak tersedia untuk cara pemasangan ini."
            );
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn ensure_target_writable_reports_unreadable_directory() {
    let target = Path::new("/definitely-not-a-real-parent-dir-xyz/dblocal");
    let err = ensure_target_writable(target).expect_err("missing parent");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
    assert!(err.detail().contains("izin direktori"));
}

#[cfg(unix)]
#[test]
fn cleanup_backup_reports_directory_in_place_of_backup() {
    let dir = tempdir().unwrap();
    let current = dir.path().join("dblocal");
    write_fake_binary(&current, "1.0.0", true);
    std::fs::create_dir(dir.path().join("dblocal.old")).unwrap();
    let err = cleanup_backup(&current).expect_err("backup is a directory");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
}

#[cfg(unix)]
#[test]
fn restore_backup_failure_removes_its_temp_copy() {
    let dir = tempdir().unwrap();
    let backup = dir.path().join("dblocal.old");
    let target = dir.path().join("dblocal");
    write_fake_binary(&backup, "1.0.0", true);
    // A non-empty directory at the target makes rename(2) fail.
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("file"), b"x").unwrap();
    let err = restore_backup(&backup, &target).expect_err("rename over dir");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
    let leftovers: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with(".dblocal.update-"))
        .collect();
    assert!(leftovers.is_empty(), "temp copy left behind: {leftovers:?}");
}

#[cfg(unix)]
#[test]
fn apply_reports_missing_staged_file_and_leaves_no_sibling() {
    let dir = tempdir().unwrap();
    let current = dir.path().join("dblocal");
    write_fake_binary(&current, "1.0.0", true);
    let plan = plan_install(
        &InstallKind::PortableTarball {
            exe_path: current.clone(),
        },
        &dir.path().join("does-not-exist"),
        &current,
        &ver("1.1.0"),
    )
    .unwrap();
    let err = apply(&plan).expect_err("staged file missing");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
    assert!(err.detail().contains("menyalin file update"));
    let names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["dblocal".to_owned()], "dir changed: {names:?}");
}

#[cfg(unix)]
#[test]
fn ensure_target_writable_asks_the_os_without_writing_a_probe() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::Uid::effective().is_root() {
        return;
    }
    let dir = tempdir().unwrap();
    let target = dir.path().join("dblocal");
    fs::write(&target, b"x").unwrap();
    let before: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    ensure_target_writable(&target).expect("writable dir");
    let after: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(before.len(), after.len(), "probe file was created");

    let original = fs::metadata(dir.path()).unwrap().permissions();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let result = ensure_target_writable(&target);
    fs::set_permissions(dir.path(), original).unwrap();
    let err = result.expect_err("read-only dir");
    assert_eq!(
        err.code(),
        crate::update::error::CODE_UPDATE_UNSUPPORTED_KIND
    );
}

#[cfg(unix)]
#[test]
fn apply_refuses_read_only_target_directory_and_leaves_it_untouched() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::Uid::effective().is_root() {
        return;
    }
    let dir = tempdir().unwrap();
    let current = dir.path().join("dblocal");
    let staged = dir.path().join("staged");
    write_fake_binary(&current, "1.0.0", true);
    write_fake_binary(&staged, "1.1.0", true);
    let original_bytes = fs::read(&current).unwrap();
    let plan = plan_install(
        &InstallKind::PortableTarball {
            exe_path: current.clone(),
        },
        &staged,
        &current,
        &ver("1.1.0"),
    )
    .unwrap();
    let mode = fs::metadata(dir.path()).unwrap().permissions().mode();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let result = apply(&plan);
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(mode)).unwrap();
    let err = result.expect_err("read-only install dir");
    assert_eq!(
        err.code(),
        crate::update::error::CODE_UPDATE_UNSUPPORTED_KIND
    );
    assert_eq!(
        fs::read(&current).unwrap(),
        original_bytes,
        "target changed"
    );
    assert!(!dir.path().join("dblocal.old").exists(), ".old left behind");
    let names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        names.len(),
        2,
        "no sibling temp or probe file should remain: {names:?}"
    );
}

#[cfg(unix)]
#[test]
fn apply_removes_staged_sibling_when_write_step_fails() {
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
    super::inject_fail_after_write_for_test();
    let err = apply(&plan).expect_err("injected write failure");
    assert_eq!(err.code(), crate::update::error::CODE_UPDATE_APPLY_FAILED);
    let names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        !names.iter().any(|n| n.starts_with(".dblocal.update-")),
        "staged sibling left behind: {names:?}"
    );
    assert!(
        !dir.path().join("dblocal.old").exists(),
        "no backup expected"
    );
    let out = version_output(&current);
    assert!(String::from_utf8_lossy(&out.stdout).contains("1.0.0"));
}

#[cfg(unix)]
#[test]
fn smoke_failure_restores_old_bytes_and_removes_staged_sibling() {
    let dir = tempdir().unwrap();
    let current = dir.path().join("dblocal");
    let staged = dir.path().join("staged");
    write_fake_binary(&current, "1.0.0", true);
    write_fake_binary(&staged, "1.1.0", false);
    let old_bytes = fs::read(&current).unwrap();
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
    assert_eq!(fs::read(&current).unwrap(), old_bytes, "old bytes restored");
    // Documented outcome: the rollback is a rename of a copy, so `.old` survives
    // for the "next successful start" cleanup instead of being deleted mid-recovery.
    assert!(
        dir.path().join("dblocal.old").exists(),
        "the backup is kept after a smoke rollback"
    );
    assert_eq!(
        fs::read(dir.path().join("dblocal.old")).unwrap(),
        old_bytes,
        "the kept backup holds the old bytes"
    );
    assert!(
        !fs::read_dir(dir.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".dblocal.update-")),
        "staged sibling left behind after rollback"
    );
}

#[cfg(unix)]
#[test]
fn restore_backup_is_continuous_and_never_unlinks_the_target() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let dir = tempdir().unwrap();
    let target = dir.path().join("dblocal");
    let backup = dir.path().join("dblocal.old");
    write_fake_binary(&target, "2.0.0", true);
    write_fake_binary(&backup, "1.0.0", true);
    let stop = Arc::new(AtomicBool::new(false));
    let missing = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let target_t = target.clone();
    let (stop_t, missing_t, reads_t) = (stop.clone(), missing.clone(), reads.clone());
    // The reader must actually get scheduled; on 4 pinned cores it can be starved
    // until the main thread finishes all its restores, leaving `reads == 0`. It
    // signals the first read, and the loop below waits for that before finishing.
    let first_read = Arc::new(AtomicBool::new(false));
    let first_read_t = Arc::clone(&first_read);
    let reader = std::thread::spawn(move || {
        while !stop_t.load(Ordering::SeqCst) {
            if !target_t.exists() {
                missing_t.store(true, Ordering::SeqCst);
                break;
            }
            let _ = fs::read(&target_t);
            reads_t.fetch_add(1, Ordering::SeqCst);
            first_read_t.store(true, Ordering::SeqCst);
            std::thread::yield_now();
        }
    });
    // Wait, bounded, for the reader to have observed the target at least once before
    // the restores start, so the reader is provably running rather than starved.
    let started = std::time::Instant::now();
    while !first_read.load(Ordering::SeqCst) {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "the reader never got scheduled to observe the target"
        );
        std::thread::yield_now();
    }
    // Keep restoring until the reader has observed the target many times or a
    // generous deadline passes, so the restores genuinely overlap the reads.
    let mut restores = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while reads.load(Ordering::SeqCst) < 100 && std::time::Instant::now() < deadline {
        restore_backup(&backup, &target).expect("restore");
        restores += 1;
        if restores > 5000 {
            break;
        }
    }
    stop.store(true, Ordering::SeqCst);
    reader.join().unwrap();
    assert!(
        !missing.load(Ordering::SeqCst),
        "target went missing during restore"
    );
    assert!(
        reads.load(Ordering::SeqCst) > 0,
        "reader never observed target"
    );
    let out = version_output(&target);
    assert!(String::from_utf8_lossy(&out.stdout).contains("1.0.0"));
}

#[test]
fn install_target_path_prefers_kind_target() {
    let dir = tempdir().unwrap();
    let appimage = dir.path().join("DBLocal.AppImage");
    std::fs::write(&appimage, b"x").unwrap();
    let resolved = install_target_path(
        &InstallKind::AppImage {
            appimage_path: appimage.clone(),
        },
        Path::new("/nonexistent/current"),
    )
    .unwrap();
    assert!(resolved.ends_with("DBLocal.AppImage"));
}
