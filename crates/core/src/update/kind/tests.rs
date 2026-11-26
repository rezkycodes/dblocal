#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use super::*;

struct FixedOwnership(Option<PackageManagerHint>);

impl PackageOwnership for FixedOwnership {
    fn owner_of(&self, _path: &Path) -> Option<PackageManagerHint> {
        self.0.clone()
    }
}

#[test]
fn unknown_is_not_self_updatable() {
    let kind = InstallKind::Unknown {
        exe_path: PathBuf::from("/tmp/dblocal"),
        reason: "test".to_owned(),
    };
    assert!(!kind.can_self_update());
    assert!(kind.package_manager_hint().is_none());
}

#[test]
fn dist_package_is_not_self_updatable() {
    let kind = InstallKind::DistPackage {
        exe_path: PathBuf::from("/usr/bin/dblocal"),
        hint: PackageManagerHint::Dpkg,
    };
    assert!(!kind.can_self_update());
    assert_eq!(kind.package_manager_hint(), Some(&PackageManagerHint::Dpkg));
    assert!(
        kind.package_manager_hint()
            .unwrap()
            .message()
            .contains("dpkg/apt")
    );
}

#[test]
fn homebrew_is_not_self_updatable() {
    let kind = InstallKind::Homebrew {
        exe_path: PathBuf::from("/opt/homebrew/bin/dblocal"),
    };
    assert!(!kind.can_self_update());
    assert_eq!(
        kind.package_manager_hint(),
        Some(&PackageManagerHint::Homebrew)
    );
}

#[test]
fn appimage_is_self_updatable() {
    let kind = InstallKind::AppImage {
        appimage_path: PathBuf::from("/home/u/DBLocal.AppImage"),
    };
    assert!(kind.can_self_update());
}

#[test]
fn portable_tarball_is_self_updatable() {
    let kind = InstallKind::PortableTarball {
        exe_path: PathBuf::from("/home/u/bin/dblocal"),
    };
    assert!(kind.can_self_update());
}

#[test]
fn detect_uses_package_ownership_trait() {
    let kind = detect_install_kind_with(
        PathBuf::from("/usr/bin/dblocal"),
        &FixedOwnership(Some(PackageManagerHint::Rpm)),
    )
    .expect("detect");
    assert!(matches!(kind, InstallKind::DistPackage { .. }));
    assert!(!kind.can_self_update());
}

#[test]
fn detect_homebrew_path() {
    let kind = detect_install_kind_with(
        PathBuf::from("/opt/homebrew/bin/dblocal"),
        &FixedOwnership(None),
    )
    .expect("detect");
    assert!(matches!(kind, InstallKind::Homebrew { .. }));
}

#[test]
fn windows_msi_path_detected() {
    let kind = detect_from_path(Path::new("C:\\Program Files\\DBLocal\\dblocal.exe"));
    if cfg!(windows) {
        assert!(matches!(kind, InstallKind::WindowsMsi { .. }));
    } else {
        // on linux this falls to portable/unknown depending on OS; just ensure no panic
        let _ = kind;
    }
}

#[test]
fn mac_app_bundle_path_detected() {
    let path = Path::new("/Applications/DBLocal.app/Contents/MacOS/dblocal");
    assert_eq!(
        mac_app_bundle_path(path),
        Some(PathBuf::from("/Applications/DBLocal.app"))
    );
}

#[test]
fn package_manager_hint_messages() {
    assert!(PackageManagerHint::Dpkg.message().contains("dpkg"));
    assert!(PackageManagerHint::Rpm.message().contains("rpm"));
    assert!(PackageManagerHint::Homebrew.message().contains("Homebrew"));
    assert!(
        PackageManagerHint::Other {
            manager: "nix".into()
        }
        .message()
        .contains("nix")
    );
}

#[test]
fn detect_from_path_linux_is_portable() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("dblocal");
    std::fs::write(&exe, b"x").unwrap();
    let kind = detect_from_path(&exe);
    assert!(matches!(kind, InstallKind::PortableTarball { .. }));
}

#[test]
fn detect_install_kind_with_ownership_fallback() {
    let kind =
        detect_install_kind_with(PathBuf::from("/opt/custom/dblocal"), &FixedOwnership(None))
            .unwrap();
    if cfg!(target_os = "linux") {
        assert!(matches!(kind, InstallKind::Unknown { .. }));
    }
}

#[test]
fn install_kind_path_accessors() {
    let p = PathBuf::from("/tmp/x");
    assert_eq!(
        InstallKind::PortableTarball {
            exe_path: p.clone()
        }
        .path(),
        p.as_path()
    );
    assert_eq!(
        InstallKind::WindowsZip {
            exe_path: p.clone()
        }
        .path(),
        p.as_path()
    );
    assert_eq!(
        InstallKind::WindowsMsi {
            exe_path: p.clone()
        }
        .path(),
        p.as_path()
    );
    assert_eq!(
        InstallKind::DistPackage {
            exe_path: p.clone(),
            hint: PackageManagerHint::Dpkg
        }
        .path(),
        p.as_path()
    );
    assert_eq!(
        InstallKind::Homebrew {
            exe_path: p.clone()
        }
        .path(),
        p.as_path()
    );
    assert_eq!(
        InstallKind::Unknown {
            exe_path: p.clone(),
            reason: "x".to_owned(),
        }
        .path(),
        p.as_path()
    );
}

#[test]
fn detect_from_path_windows_branch_via_direct_call() {
    // On non-Windows hosts, detect_windows still returns a deterministic kind.
    let kind = detect_windows(Path::new("C:\\Users\\me\\Downloads\\dblocal.exe"));
    assert!(matches!(kind, InstallKind::WindowsZip { .. }));
    let kind = detect_windows(Path::new("C:\\Program Files\\DBLocal\\dblocal.exe"));
    assert!(matches!(kind, InstallKind::WindowsMsi { .. }));
    let kind = detect_windows(Path::new(
        "C:\\Users\\me\\AppData\\Local\\DBLocal\\dblocal.exe",
    ));
    assert!(matches!(kind, InstallKind::WindowsMsi { .. }));
}

#[test]
fn detect_linux_deny_list_paths() {
    if !cfg!(target_os = "linux") {
        return;
    }
    for path in [
        "/home/u/.cargo/bin/dblocal",
        "/nix/store/abc-dblocal/bin/dblocal",
        "/home/u/src/dblocal/target/debug/dblocal",
        "/snap/dblocal/current/bin/dblocal",
    ] {
        assert!(
            linux_deny_reason(Path::new(path)).is_some(),
            "deny rule missing for {path}"
        );
        let kind = detect_from_path(Path::new(path));
        assert!(matches!(kind, InstallKind::Unknown { .. }), "path {path}");
    }
}

#[test]
fn each_managed_prefix_is_denied_at_the_start_of_the_path() {
    for (path, prefix) in [
        ("/usr/local/bin/dblocal", "/usr/"),
        ("/usr/bin/dblocal", "/usr/"),
        ("/opt/dblocal/dblocal", "/opt/"),
        ("/app/dblocal", "/app/"),
        ("/snap/dblocal/current/bin/dblocal", "/snap/"),
        ("/nix/store/abc-dblocal/bin/dblocal", "/nix/store/"),
    ] {
        assert_eq!(
            path_rule_deny_reason(Path::new(path)),
            Some(DenyReason::ManagedPrefix { prefix }),
            "prefix rule for {path}"
        );
    }
}

#[test]
fn each_build_tooling_suffix_is_denied_anywhere() {
    for (path, suffix) in [
        ("/home/u/.cargo/bin/dblocal", "/.cargo/bin/"),
        ("/home/u/src/dblocal/target/debug/dblocal", "/target/debug/"),
        (
            "/home/u/src/dblocal/target/release/dblocal",
            "/target/release/",
        ),
    ] {
        assert_eq!(
            path_rule_deny_reason(Path::new(path)),
            Some(DenyReason::BuildTooling { suffix }),
            "suffix rule for {path}"
        );
    }
}

#[test]
fn user_owned_prefixes_are_not_treated_as_managed_locations() {
    for path in ["/home/u/opt/dblocal/dblocal", "/home/u/work/app/dblocal"] {
        assert_eq!(
            path_rule_deny_reason(Path::new(path)),
            None,
            "{path} must not be denied by the prefix rules"
        );
    }
}

#[cfg(unix)]
#[test]
fn user_owned_prefix_trees_stay_portable_and_self_updatable() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for relative in ["opt/dblocal/dblocal", "work/app/dblocal"] {
        let exe = dir.path().join(relative);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"x").unwrap();
        assert_eq!(
            linux_deny_reason(&exe),
            None,
            "{} must not be denied",
            exe.display()
        );
        let kind = detect_from_path(&exe);
        assert!(
            matches!(kind, InstallKind::PortableTarball { .. }),
            "{} should be portable, got {kind:?}",
            exe.display()
        );
    }
}

#[cfg(unix)]
#[test]
fn unwritable_directory_is_denied() {
    use std::os::unix::fs::PermissionsExt;
    if !cfg!(target_os = "linux") {
        return;
    }
    if nix::unistd::Uid::effective().is_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("dblocal");
    std::fs::write(&exe, b"x").unwrap();
    let original = std::fs::metadata(dir.path()).unwrap().permissions();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let reason = linux_deny_reason(&exe);
    std::fs::set_permissions(dir.path(), original).unwrap();
    assert_eq!(reason, Some(DenyReason::NotWritable));
    assert!(
        reason.unwrap().message().contains("not writable"),
        "message must explain the writability refusal"
    );
}

#[test]
fn managed_prefix_reaches_the_unknown_refusal_message() {
    let kind = detect_from_path(Path::new("/usr/local/bin/dblocal"));
    if cfg!(target_os = "linux") {
        match kind {
            InstallKind::Unknown { reason, .. } => {
                assert!(reason.contains("/usr/"), "reason was {reason}");
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }
}

#[cfg(unix)]
#[test]
fn detection_does_not_write_into_the_install_directory() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("dblocal");
    std::fs::write(&exe, b"x").unwrap();
    let listing = |p: &Path| {
        let mut names: Vec<String> = std::fs::read_dir(p)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    };
    let before = listing(dir.path());
    let kind = detect_from_path(&exe);
    assert!(matches!(kind, InstallKind::PortableTarball { .. }));
    assert_eq!(listing(dir.path()), before, "detection wrote a probe file");
}

#[test]
fn detect_macos_unknown_when_not_in_bundle() {
    let kind = detect_macos(Path::new("/usr/local/bin/dblocal"));
    assert!(matches!(kind, InstallKind::Unknown { .. }));
}

#[test]
fn detect_package_manager_on_linux_path_does_not_panic() {
    let _ = detect_package_manager(Path::new("/definitely/not/owned/dblocal"));
}

#[test]
fn command_reports_ownership_false_for_missing_cmd() {
    assert!(!command_reports_ownership(
        "definitely-not-a-command",
        &["-S", "/bin/sh"]
    ));
}

#[test]
fn detect_from_env_with_appimage() {
    let dir = tempfile::tempdir().unwrap();
    let appdir = dir.path().join("appdir");
    std::fs::create_dir_all(appdir.join("usr/bin")).unwrap();
    let exe = appdir.join("usr/bin/dblocal");
    std::fs::write(&exe, b"x").unwrap();
    let appimage = dir.path().join("DBLocal.AppImage");
    std::fs::write(&appimage, b"app").unwrap();
    let kind = detect_from_env_with(&exe, |k| match k {
        "APPIMAGE" => Some(appimage.to_string_lossy().to_string()),
        "APPDIR" => Some(appdir.to_string_lossy().to_string()),
        _ => None,
    });
    assert!(matches!(kind, Some(InstallKind::AppImage { .. })));
}

#[test]
fn detect_from_env_rejects_foreign_appimage() {
    let dir = tempfile::tempdir().unwrap();
    let foreign = dir.path().join("Other.AppImage");
    std::fs::write(&foreign, b"other").unwrap();
    let kind = detect_from_env_with(Path::new("/usr/local/bin/dblocal"), |k| match k {
        "APPIMAGE" => Some(foreign.to_string_lossy().to_string()),
        _ => None,
    });
    assert!(kind.is_none());
}

#[test]
fn detect_from_env_with_homebrew() {
    let kind = detect_from_env_with(Path::new("/opt/homebrew/bin/dblocal"), |_| None);
    assert!(matches!(kind, Some(InstallKind::Homebrew { .. })));
}

#[test]
fn detect_from_env_with_none() {
    let kind = detect_from_env_with(Path::new("/tmp/dblocal"), |_| None);
    assert!(kind.is_none());
}

#[test]
fn is_per_user_windows_msi_handles_mixed_separators() {
    assert!(is_per_user_windows_msi(Path::new(
        r"C:\Users\runneradmin\AppData\Local\Temp\.tmp123\AppData/Local/DBLocal\dblocal.exe"
    )));
    assert!(is_per_user_windows_msi(Path::new(
        r"C:\Users\alice\AppData\Local\DBLocal\dblocal.exe"
    )));
    assert!(!is_per_user_windows_msi(Path::new(
        r"C:\Program Files\DBLocal\dblocal.exe"
    )));
}
