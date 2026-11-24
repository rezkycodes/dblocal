//! Windows spelling of the MariaDB bootstrap: another tool, other arguments and
//! option-file paths without backslashes. Every test passes the platform in, so
//! they run on every host.

use super::*;
use crate::engine::InitContext;
use crate::model::{AuthMode, Port};

fn ctx() -> InitContext {
    InitContext {
        instance_name: "w".into(),
        runtime_dir: PathBuf::from(r"C:\dbl\home\runtimes\mariadb\11.8.10\x86_64"),
        data_dir: PathBuf::from(r"C:\dbl\home\instances\w\data"),
        port: Port::new(13306).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        password: None,
    }
}

fn args_of(cmd: &Command) -> Vec<String> {
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn windows_uses_the_exe_in_bin_and_unix_the_script() {
    let rt = Path::new("/rt");
    assert_eq!(
        install_db_program(rt, true),
        Path::new("/rt").join("bin").join("mariadb-install-db.exe")
    );
    assert_eq!(
        install_db_program(rt, false),
        Path::new("/rt").join("scripts").join("mariadb-install-db")
    );
}

#[test]
fn windows_tool_gets_only_the_arguments_it_knows() {
    let args = args_of(&build_install_db_command_for(&ctx(), true));
    assert!(args.iter().any(|a| a.starts_with("--datadir=")), "{args:?}");
    assert!(args.iter().any(|a| a == "--silent"), "{args:?}");
    for unknown in ["--basedir", "--auth-root", "--skip-test-db"] {
        assert!(
            !args.iter().any(|a| a.starts_with(unknown)),
            "the Windows tool rejects {unknown}: {args:?}"
        );
    }
}

#[test]
fn unix_script_keeps_its_arguments() {
    let args = args_of(&build_install_db_command_for(&ctx(), false));
    assert!(args.iter().any(|a| a.starts_with("--basedir=")), "{args:?}");
    assert!(
        args.iter()
            .any(|a| a == "--auth-root-authentication-method=normal"),
        "{args:?}"
    );
    assert!(args.iter().any(|a| a == "--skip-test-db"), "{args:?}");
}

#[test]
fn windows_option_file_has_no_backslash() {
    let ctx = ctx();
    let socket = ctx
        .data_dir
        .join("..")
        .join("run")
        .join("mariadb-13306.sock");
    let conf = render_config_for(&config_params(&ctx, &socket).expect("params"), true);
    let text = conf.expose();
    assert!(
        !text.contains('\\'),
        "no backslash may reach my.cnf:\n{text}"
    );
    assert!(text.contains("basedir=C:/dbl/home/runtimes/mariadb/11.8.10/x86_64\n"));
    assert!(text.contains("datadir=C:/dbl/home/instances/w/data\n"));
    assert!(text.contains("bind-address=127.0.0.1\n"));
}

#[test]
fn unix_option_file_is_unchanged() {
    let ctx = InitContext {
        runtime_dir: PathBuf::from("/opt/mariadb"),
        data_dir: PathBuf::from("/h/instances/m/data"),
        ..ctx()
    };
    let socket = PathBuf::from("/h/instances/m/run/mariadb-13306.sock");
    let conf = render_config_for(&config_params(&ctx, &socket).expect("params"), false);
    assert!(conf.expose().contains("datadir=/h/instances/m/data\n"));
    assert!(
        conf.expose()
            .contains("socket=/h/instances/m/run/mariadb-13306.sock\n")
    );
}

/// Windows exits 1 after the init-file `SHUTDOWN` although the change was
/// applied; Unix exits 0. A non-zero Unix exit stays a failure.
#[test]
fn bootstrap_exit_policy_per_platform() {
    assert!(bootstrap_exit_ok(true, false));
    assert!(
        !bootstrap_exit_ok(false, false),
        "unix failure must stay one"
    );
    assert!(bootstrap_exit_ok(false, true), "windows exits 1 on success");
    assert!(bootstrap_exit_ok(true, true));
}

#[test]
fn unix_bootstrap_still_skips_networking_and_windows_does_not() {
    let init_file = PathBuf::from("/tmp/init.sql");
    let args = args_of(&build_bootstrap_command(&ctx(), &init_file));
    let skips = args.iter().any(|a| a == "--skip-networking");
    assert_eq!(skips, !cfg!(windows), "{args:?}");
}
