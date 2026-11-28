//! Shim generation, refresh and safe-removal tests.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use tempfile::tempdir;

use crate::platform::Os;
use crate::toolchain::catalog::ToolId;
use crate::toolchain::fsops::ToolchainPaths;
use crate::toolchain::registry::{ActiveVersion, Registry};
use crate::toolchain::shim::{ShimTool, refresh, remove_shim_if_ours, render, shim_target};

fn paths(dir: &Path) -> ToolchainPaths {
    ToolchainPaths::with_root(dir)
}

#[test]
fn php_shim_sets_ini_scan_dir_and_execs_the_target() {
    let install = Path::new("/h/toolchains/php/8.4.15-common");
    let ini = Path::new("/h/toolchains/php/conf.d");
    let php_shim = Path::new("/h/bin/php");
    let text = render(ShimTool::Php, Os::Linux, install, ini, php_shim);
    assert!(text.starts_with("#!/bin/sh"));
    assert!(text.contains("PHP_INI_SCAN_DIR='/h/toolchains/php/conf.d'"));
    assert!(text.contains("exec '/h/toolchains/php/8.4.15-common/php' \"$@\""));
    assert!(text.contains("# DBLocal shim"));
}

#[test]
fn node_shims_exec_their_binaries() {
    let install = Path::new("/h/toolchains/node/24.11.0");
    let ini = Path::new("/h/toolchains/php/conf.d");
    let php_shim = Path::new("/h/bin/php");
    for shim in [ShimTool::Node, ShimTool::Npm, ShimTool::Npx] {
        let text = render(shim, Os::Linux, install, ini, php_shim);
        assert!(
            text.contains(&format!(
                "exec '/h/toolchains/node/24.11.0/bin/{}' \"$@\"",
                shim.name()
            )),
            "{shim:?}: {text}"
        );
    }
}

#[test]
fn composer_shim_delegates_to_sibling_php_and_errors_without_it() {
    let install = Path::new("/h/toolchains/composer/2.8.4");
    let ini = Path::new("/h/toolchains/php/conf.d");
    let php_shim = Path::new("/h/bin/php");
    let text = render(ShimTool::Composer, Os::Linux, install, ini, php_shim);
    assert!(text.contains("no active PHP"), "{text}");
    assert!(
        text.contains("exec '/h/bin/php' '/h/toolchains/composer/2.8.4/composer.phar' \"$@\""),
        "{text}"
    );
}

#[test]
fn shell_quote_handles_single_quotes() {
    let install = Path::new("/h/it's here/php");
    let text = render(
        ShimTool::Php,
        Os::Linux,
        install,
        Path::new("/h/conf"),
        Path::new("/h/bin/php"),
    );
    // The embedded quote must be escaped, not terminate the string.
    assert!(text.contains("it'\\''s here"), "{text}");
}

#[test]
fn windows_php_shim_is_a_cmd_that_propagates_exit_code() {
    let install = Path::new(r"C:\h\toolchains\php\8.4.15");
    let ini = Path::new(r"C:\h\conf");
    let php_shim = Path::new(r"C:\h\bin\php.cmd");
    let text = render(ShimTool::Php, Os::Windows, install, ini, php_shim);
    assert!(text.starts_with("@echo off"));
    assert!(text.contains("rem DBLocal managed shim"));
    assert!(text.contains(r#"set "PHP_INI_SCAN_DIR=C:\h\conf""#));
    // The target path is built with the platform separator of the host the test
    // runs on; assert on the stable tail rather than the separators.
    assert!(text.contains("php.exe"), "{text}");
    assert!(text.contains("%*"), "{text}");
    assert!(text.contains("exit /b %ERRORLEVEL%"));
}

#[test]
fn windows_composer_shim_uses_call_and_errors() {
    let install = Path::new(r"C:\h\toolchains\composer\2.8.4");
    let text = render(
        ShimTool::Composer,
        Os::Windows,
        install,
        Path::new(r"C:\h\conf"),
        Path::new(r"C:\h\bin\php.cmd"),
    );
    assert!(text.contains("no active PHP"), "{text}");
    assert!(text.contains("call "), "{text}");
    assert!(text.contains("exit /b %ERRORLEVEL%"), "{text}");
}

#[test]
fn refresh_writes_current_shims_and_removes_stale_ones() {
    let dir = tempdir().expect("tmp");
    let paths = paths(dir.path());
    let registry = Registry::empty()
        .with_active(ActiveVersion::new(
            ToolId::Php,
            "8.4.15",
            Some("common".into()),
        ))
        .with_active(ActiveVersion::new(ToolId::Node, "24.11.0", None));

    refresh(&paths, Os::Linux, &registry).expect("refresh");
    let php = paths.shim_path("php", Os::Linux);
    assert!(php.is_file());
    assert!(paths.shim_path("node", Os::Linux).is_file());
    assert!(paths.shim_path("npm", Os::Linux).is_file());
    // Composer is not active, so no composer shim.
    assert!(!paths.shim_path("composer", Os::Linux).exists());

    // Shims are executable on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&php).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    // Refresh is idempotent.
    refresh(&paths, Os::Linux, &registry).expect("refresh again");
    assert!(php.is_file());

    // Clearing node removes its shims.
    let only_php = registry.without(ToolId::Node);
    refresh(&paths, Os::Linux, &only_php).expect("refresh");
    assert!(!paths.shim_path("node", Os::Linux).exists());
    assert!(php.is_file());
}

#[test]
fn composer_refreshes_even_without_active_php() {
    let dir = tempdir().expect("tmp");
    let paths = paths(dir.path());
    let registry =
        Registry::empty().with_active(ActiveVersion::new(ToolId::Composer, "2.8.4", None));
    refresh(&paths, Os::Linux, &registry).expect("composer shim writes even without php");
    assert!(paths.shim_path("composer", Os::Linux).is_file());
}

#[test]
fn composer_refreshes_when_php_is_active() {
    let dir = tempdir().expect("tmp");
    let paths = paths(dir.path());
    let registry = Registry::empty()
        .with_active(ActiveVersion::new(
            ToolId::Php,
            "8.4.15",
            Some("common".into()),
        ))
        .with_active(ActiveVersion::new(ToolId::Composer, "2.8.4", None));
    refresh(&paths, Os::Linux, &registry).expect("refresh");
    assert!(paths.shim_path("composer", Os::Linux).is_file());
}

#[test]
fn removal_never_touches_a_file_dblocal_did_not_write() {
    let dir = tempdir().expect("tmp");
    let shims = dir.path().join("bin");
    std::fs::create_dir_all(&shims).expect("mkdir");
    let user_file = shims.join("php");
    std::fs::write(&user_file, "#!/bin/sh\necho user's own php\n").expect("write");

    // Not ours: no marker, so it must survive.
    assert!(
        !remove_shim_if_ours(&user_file),
        "must not remove a user file"
    );
    assert!(user_file.exists());

    // Ours: marker present, so it is removed.
    let ours = shims.join("node");
    std::fs::write(&ours, "#!/bin/sh\n# DBLocal shim\nexec /x \"$@\"\n").expect("write");
    assert!(remove_shim_if_ours(&ours));
    assert!(!ours.exists());
}

#[test]
fn shim_target_reads_the_baked_in_path() {
    let dir = tempdir().expect("tmp");
    let paths = paths(dir.path());
    let registry = Registry::empty().with_active(ActiveVersion::new(
        ToolId::Php,
        "8.4.15",
        Some("common".into()),
    ));
    refresh(&paths, Os::Linux, &registry).expect("refresh");
    let target = shim_target(&paths.shim_path("php", Os::Linux)).expect("target");
    assert_eq!(
        target,
        paths
            .install_dir(ToolId::Php, "8.4.15", Some("common"))
            .join("php")
    );
}

#[cfg(unix)]
#[test]
fn npm_shim_prepends_node_bin_dir_over_system_node() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempdir().expect("tmp");
    let sys_bin = dir.path().join("sysbin");
    std::fs::create_dir_all(&sys_bin).expect("sysbin");
    let sys_node = sys_bin.join("node");
    std::fs::write(&sys_node, "#!/bin/sh\necho SYSTEM_NODE\n").expect("sys node");
    std::fs::set_permissions(&sys_node, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let paths = paths(dir.path());
    let install_dir = paths.install_dir(ToolId::Node, "24.11.0", None);
    let node_bin = install_dir.join("bin");
    std::fs::create_dir_all(&node_bin).expect("node bin");
    let dbl_node = node_bin.join("node");
    std::fs::write(&dbl_node, "#!/bin/sh\necho DBLOCAL_NODE\n").expect("dbl node");
    std::fs::set_permissions(&dbl_node, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let dbl_npm = node_bin.join("npm");
    std::fs::write(&dbl_npm, "#!/bin/sh\nnode\n").expect("dbl npm");
    std::fs::set_permissions(&dbl_npm, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let registry =
        Registry::empty().with_active(ActiveVersion::new(ToolId::Node, "24.11.0", None));
    refresh(&paths, Os::Linux, &registry).expect("refresh");

    let npm_shim = paths.shim_path("npm", Os::Linux);
    let out = std::process::Command::new(&npm_shim)
        .env("PATH", format!("{}:/usr/bin:/bin", sys_bin.display()))
        .output()
        .expect("run npm shim");
    assert!(out.status.success(), "{:?}", out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim(), "DBLOCAL_NODE", "wrong node ran: {stdout}");

    let win_npm = render(
        ShimTool::Npm,
        Os::Windows,
        Path::new(r"C:\h\toolchains\node\24.11.0"),
        Path::new(r"C:\h\conf"),
        Path::new(r"C:\h\bin\php.cmd"),
    );
    assert!(
        win_npm.contains(r#"set "PATH=C:\h\toolchains\node\24.11.0;%PATH%""#),
        "{win_npm}"
    );
}
