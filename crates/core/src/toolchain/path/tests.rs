//! PATH block, doctor and Windows path-logic tests.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use tempfile::tempdir;

use crate::platform::Os;
use crate::toolchain::consts;
use crate::toolchain::fsops::ToolchainPaths;
use crate::toolchain::path::{
    Finding, apply_posix, block_shims_dir, diagnose, has_block, path_contains, prepend_windows_path,
    remove_block, remove_from_windows_path, remove_posix, render_block,
};

#[test]
fn block_has_markers_and_is_idempotent() {
    let block = render_block(Path::new("/h/bin"));
    assert!(block.contains(consts::PATH_BLOCK_BEGIN));
    assert!(block.contains(consts::PATH_BLOCK_END));
    assert!(block.contains("/h/bin"), "{block}");

    let original = "# my bashrc\nexport FOO=1\n";
    let once = crate::toolchain::path::apply_block(original, Path::new("/h/bin"));
    let twice = crate::toolchain::path::apply_block(&once, Path::new("/h/bin"));
    assert_eq!(once, twice, "applying twice must yield one block");
    assert_eq!(once.matches(consts::PATH_BLOCK_BEGIN).count(), 1);
    assert!(once.starts_with("# my bashrc\n"), "{once}");
}

#[test]
fn remove_restores_the_original_bytes() {
    let original = "# my zshrc\nexport FOO=1\nalias ll='ls -l'\n";
    let applied = crate::toolchain::path::apply_block(original, Path::new("/h/bin"));
    let removed = remove_block(&applied);
    assert_eq!(removed, original, "remove must restore exactly");
}

#[test]
fn empty_file_round_trips() {
    let applied = crate::toolchain::path::apply_block("", Path::new("/h/bin"));
    assert!(has_block(&applied));
    let removed = remove_block(&applied);
    assert_eq!(removed, "");
}

#[test]
fn apply_edits_every_existing_rc_file_and_writes_a_backup() {
    let home = tempdir().expect("tmp");
    let home_path = home.path();
    std::fs::write(home_path.join(".bashrc"), "# bashrc\n").expect("bashrc");
    std::fs::write(home_path.join(".profile"), "# profile\n").expect("profile");
    // No .zshrc: it must not be created.

    let report = apply_posix(home_path, &home_path.join("bin"));
    assert_eq!(report.changed.len(), 2);
    assert_eq!(report.failures.len(), 0);
    assert!(report.backups.len() == 2);
    assert!(home_path.join(".bashrc.dblocal-backup").exists());
    let bashrc = std::fs::read_to_string(home_path.join(".bashrc")).expect("read");
    assert!(has_block(&bashrc));
    assert!(!home_path.join(".zshrc").exists(), "must not create a missing rc file");

    // Remove restores both byte for byte.
    let report = remove_posix(home_path, &home_path.join("bin"));
    assert_eq!(report.changed.len(), 2);
    assert_eq!(std::fs::read_to_string(home_path.join(".bashrc")).unwrap(), "# bashrc\n");
    assert_eq!(std::fs::read_to_string(home_path.join(".profile")).unwrap(), "# profile\n");
}

#[cfg(unix)]
#[test]
fn a_read_only_rc_file_is_reported_not_clobbered() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempdir().expect("tmp");
    let home_path = home.path();
    let rc = home_path.join(".bashrc");
    std::fs::write(&rc, "# locked\n").expect("write");
    let mut perms = std::fs::metadata(&rc).unwrap().permissions();
    perms.set_mode(0o444);
    std::fs::set_permissions(&rc, perms).expect("chmod");

    let report = apply_posix(home_path, &home_path.join("bin"));
    assert_eq!(report.changed.len(), 0);
    assert_eq!(report.failures.len(), 1, "a read-only file is reported");
    // Restore so the tempdir can clean up.
    let mut perms = std::fs::metadata(&rc).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&rc, perms).expect("restore");
}

#[test]
fn block_shims_dir_is_read_back() {
    let home = tempdir().expect("tmp");
    let home_path = home.path();
    std::fs::write(home_path.join(".profile"), "# p\n").expect("profile");
    apply_posix(home_path, &home_path.join("bin"));
    let dir = block_shims_dir(home_path).expect("dir");
    assert!(dir.ends_with("bin"));
}

#[test]
fn doctor_reports_missing_not_first_shadow_and_dangling() {
    let shims = Path::new("/home/u/.dblocal/bin");
    // Missing.
    let findings = diagnose("/usr/bin:/bin", shims, Os::Linux, &[], None);
    assert!(findings.iter().any(|f| matches!(f, Finding::ShimsDirMissing { .. })));

    // First: clean.
    let findings = diagnose("/home/u/.dblocal/bin:/usr/bin", shims, Os::Linux, &[], None);
    assert!(findings.is_empty(), "clean PATH must have no findings: {findings:?}");

    // Not first.
    let findings = diagnose("/usr/bin:/home/u/.dblocal/bin", shims, Os::Linux, &[], None);
    assert!(findings.iter().any(|f| matches!(f, Finding::ShimsDirNotFirst { .. })));
}

#[test]
fn doctor_names_a_shadowing_binary_when_it_exists() {
    let dir = tempdir().expect("tmp");
    let earlier = dir.path().join("earlier");
    std::fs::create_dir_all(&earlier).expect("mkdir");
    // Create a file named `php` earlier on PATH.
    std::fs::write(earlier.join("php"), "#!/bin/sh\n").expect("php");
    let shims = dir.path().join("shims");
    let path_var = format!("{}:/usr/bin", earlier.display());
    let findings = diagnose(&path_var, &shims, Os::Linux, &[], None);
    let shadow = findings
        .iter()
        .find(|f| matches!(f, Finding::ShadowedBinary { name, .. } if name == "php"));
    assert!(shadow.is_some(), "must name the shadowing php: {findings:?}");
}

#[test]
fn doctor_reports_a_dangling_shim() {
    let dir = tempdir().expect("tmp");
    let shim = dir.path().join("bin/php");
    std::fs::create_dir_all(shim.parent().unwrap()).expect("mkdir");
    std::fs::write(
        &shim,
        "#!/bin/sh\n# DBLocal shim\nexec '/does/not/exist/bin/php' \"$@\"\n",
    )
    .expect("shim");
    let shims = vec![("php".to_owned(), shim)];
    let findings = diagnose(
        &format!("{}:/usr/bin", dir.path().join("bin").display()),
        &dir.path().join("bin"),
        Os::Linux,
        &shims,
        None,
    );
    assert!(
        findings.iter().any(|f| matches!(f, Finding::DanglingShim { .. })),
        "{findings:?}"
    );
}

#[test]
fn doctor_reports_a_stale_block_dir() {
    let shims = Path::new("/home/u/.dblocal/bin");
    let findings = diagnose(
        "/home/u/.dblocal/bin:/usr/bin",
        shims,
        Os::Linux,
        &[],
        Some(Path::new("/old/bin")),
    );
    assert!(findings.iter().any(|f| matches!(f, Finding::StaleBlock { .. })));
}

#[test]
fn finding_codes_and_messages_are_present() {
    let findings = vec![
        Finding::ShimsDirMissing { dir: "/h/bin".into() },
        Finding::ShimsDirNotFirst { dir: "/h/bin".into(), before: "/usr/bin".into() },
        Finding::ShadowedBinary { name: "php".into(), earlier: "/usr/bin/php".into() },
        Finding::DanglingShim { shim: "/h/bin/php".into(), target: "/gone/php".into() },
        Finding::StaleBlock { dir: "/old".into() },
    ];
    for finding in findings {
        assert!(!finding.code().is_empty());
        assert!(!finding.message().is_empty());
    }
}

#[test]
fn windows_path_round_trips() {
    let shims = Path::new(r"C:\Users\u\.dblocal\bin");
    let original = r"C:\Windows;C:\Windows\System32";
    let prepended = prepend_windows_path(original, shims);
    assert!(prepended.starts_with(r"C:\Users\u\.dblocal\bin;"));
    // Idempotent.
    assert_eq!(prepend_windows_path(&prepended, shims), prepended);
    // Removed exactly.
    let removed = remove_from_windows_path(&prepended, shims);
    assert_eq!(removed, original);
}

#[test]
fn windows_prepend_moves_an_existing_entry_to_the_front() {
    let shims = Path::new(r"C:\Users\u\.dblocal\bin");
    let current = r"C:\Windows;C:\Users\u\.dblocal\bin;C:\Windows\System32";
    let result = prepend_windows_path(current, shims);
    assert!(result.starts_with(r"C:\Users\u\.dblocal\bin;"), "{result}");
    assert_eq!(result.matches("dblocal").count(), 1, "no duplicate: {result}");
}

#[test]
fn path_contains_is_case_and_separator_insensitive_on_windows() {
    let shims = Path::new(r"C:\Users\u\.dblocal\bin");
    assert!(path_contains(
        r"c:\windows;c:\users\u\.dblocal\BIN",
        shims,
        Os::Windows
    ));
    assert!(!path_contains(r"C:\Windows", shims, Os::Windows));
}

#[test]
fn doctor_wires_up_from_a_toolchain_root() {
    let dir = tempdir().expect("tmp");
    let paths = ToolchainPaths::with_root(dir.path());
    // Nothing installed, empty PATH: still reports the missing dir, never panics.
    let findings = crate::toolchain::path::doctor(&paths, dir.path(), "/usr/bin", Os::Linux);
    assert!(findings.iter().any(|f| matches!(f, Finding::ShimsDirMissing { .. })));
    let _: PathBuf = paths.shims_dir();
}

#[test]
fn windows_env_trait_applies_and_removes_idempotently() {
    struct FakeWinEnv(std::cell::RefCell<Option<String>>);
    impl crate::toolchain::path::WindowsEnv for FakeWinEnv {
        fn get_user_path(&self) -> crate::Result<Option<String>> {
            Ok(self.0.borrow().clone())
        }
        fn set_user_path(&self, value: &str) -> crate::Result<()> {
            *self.0.borrow_mut() = Some(value.to_owned());
            Ok(())
        }
    }

    let env = FakeWinEnv(std::cell::RefCell::new(Some(r"C:\Windows".to_owned())));
    let shims = Path::new(r"C:\Users\u\.dblocal\bin");

    let first = crate::toolchain::path::apply_windows_with(&env, shims).expect("apply 1");
    assert_eq!(first.changed.len(), 1);
    assert_eq!(
        env.0.borrow().as_deref(),
        Some(r"C:\Users\u\.dblocal\bin;C:\Windows")
    );

    let second = crate::toolchain::path::apply_windows_with(&env, shims).expect("apply 2");
    assert!(second.changed.is_empty());
    assert_eq!(second.unchanged.len(), 1);

    let removed = crate::toolchain::path::remove_windows_with(&env, shims).expect("remove");
    assert_eq!(removed.changed.len(), 1);
    assert_eq!(env.0.borrow().as_deref(), Some(r"C:\Windows"));
}
