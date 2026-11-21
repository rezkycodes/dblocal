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
