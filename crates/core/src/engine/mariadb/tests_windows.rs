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

#[test]
fn admin_program_uses_exe_on_windows_and_bare_on_unix() {
    let rt = Path::new("/rt");
    assert_eq!(
        admin_program(rt, true),
        Path::new("/rt").join("bin").join("mariadb-admin.exe")
    );
    assert_eq!(
        admin_program(rt, false),
        Path::new("/rt").join("bin").join("mariadb-admin")
    );
}

#[test]
fn render_admin_defaults_file_escapes_password_and_sets_user() {
    let secret = SecretString::new("p@ss\"w\\rd".to_owned());
    let content = render_admin_defaults("root", AuthMode::Password, Some(&secret));
    let text = content.expose();
    assert!(text.contains("[client]\n"));
    assert!(text.contains("user=root\n"));
    assert!(text.contains("password=\"p@ss\\\"w\\\\rd\"\n"));
}

#[test]
fn render_admin_defaults_file_omits_password_for_auth_none() {
    let content = render_admin_defaults("root", AuthMode::None, None);
    let text = content.expose();
    assert!(text.contains("[client]\n"));
    assert!(text.contains("user=root\n"));
    assert!(!text.contains("password="));
}

#[cfg(unix)]
#[test]
fn admin_shutdown_happy_path_passes_defaults_file_and_cleans_up() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let rt = tmp.path().join("rt");
    let data = tmp.path().join("inst").join("data");
    let run = tmp.path().join("inst").join("run");
    let logs = tmp.path().join("inst").join("logs");
    fs::create_dir_all(rt.join("bin")).expect("bin");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&run).expect("run");
    fs::create_dir_all(&logs).expect("logs");

    let capture = tmp.path().join("captured.txt");
    let admin = rt.join("bin").join("mariadb-admin");
    let script = format!(
        "#!/bin/sh\n\
         echo \"ARGS:$*\" > \"{}\"\n\
         echo \"MYSQL_PWD:${{MYSQL_PWD:-UNSET}}\" >> \"{}\"\n\
         for a in \"$@\"; do\n\
           case \"$a\" in\n\
             --defaults-file=*)\n\
               f=\"${{a#--defaults-file=}}\"\n\
               cat \"$f\" >> \"{}\"\n\
               ;;\n\
           esac\n\
         done\n\
         exit 0\n",
        capture.display(),
        capture.display(),
        capture.display(),
    );
    fs::write(&admin, script).expect("write admin");
    fs::set_permissions(&admin, fs::Permissions::from_mode(0o755)).expect("chmod");

    let pw = SecretString::new("s3cr3t-pw-99".to_owned());
    let run_ctx = RunContext {
        instance_name: "m1".into(),
        runtime_dir: rt.clone(),
        data_dir: data,
        run_dir: run.clone(),
        logs_dir: logs,
        port: Port::new(43306).expect("port"),
        extra_port: None,
        superuser: "root".into(),
        auth: AuthMode::Password,
        credential: Some(Credential {
            username: "root".into(),
            password: pw,
        }),
    };

    shutdown_via_admin_with(&run_ctx, 12345, PidIdentity::Match, &admin)
        .expect("admin shutdown should succeed");

    let captured = fs::read_to_string(&capture).expect("captured");
    let first_line = captured.lines().next().unwrap_or("");
    assert!(
        first_line.starts_with("ARGS:--defaults-file="),
        "first arg must be --defaults-file: {first_line}"
    );
    assert!(
        first_line.contains("--connect-timeout="),
        "must pass --connect-timeout: {first_line}"
    );
    assert!(
        !first_line.contains("s3cr3t-pw-99"),
        "password must not appear in argv: {first_line}"
    );
    assert!(
        captured.contains("MYSQL_PWD:UNSET"),
        "MYSQL_PWD must not be set: {captured}"
    );
    assert!(
        captured.contains("password=\"s3cr3t-pw-99\""),
        "defaults file must carry password: {captured}"
    );
    let leftover: Vec<_> = fs::read_dir(&run)
        .expect("read run")
        .filter_map(std::result::Result::ok)
        .collect();
    assert!(
        leftover.is_empty(),
        "temp defaults file leaked: {leftover:?}"
    );
}

#[cfg(unix)]
#[test]
fn admin_shutdown_refuses_when_pid_is_not_ours_or_missing_or_unverifiable() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let rt = tmp.path().join("rt");
    let data = tmp.path().join("inst").join("data");
    let run = tmp.path().join("inst").join("run");
    let logs = tmp.path().join("inst").join("logs");
    fs::create_dir_all(rt.join("bin")).expect("bin");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&run).expect("run");
    fs::create_dir_all(&logs).expect("logs");

    let marker = tmp.path().join("invoked");
    let admin = rt.join("bin").join("mariadb-admin");
    fs::write(
        &admin,
        format!("#!/bin/sh\ntouch \"{}\"\nexit 0\n", marker.display()),
    )
    .expect("write");
    fs::set_permissions(&admin, fs::Permissions::from_mode(0o755)).expect("chmod");

    let run_ctx = RunContext {
        instance_name: "m2".into(),
        runtime_dir: rt,
        data_dir: data,
        run_dir: run,
        logs_dir: logs,
        port: Port::new(43307).expect("port"),
        extra_port: None,
        superuser: "root".into(),
        auth: AuthMode::None,
        credential: None,
    };

    shutdown_via_admin_with(
        &run_ctx,
        99999,
        PidIdentity::NotOurs("different exe".into()),
        &admin,
    )
    .expect("NotOurs returns Ok");
    assert!(!marker.exists(), "must not invoke admin for NotOurs");

    shutdown_via_admin_with(&run_ctx, 99999, PidIdentity::NoSuchProcess, &admin)
        .expect("NoSuchProcess returns Ok");
    assert!(!marker.exists(), "must not invoke admin for NoSuchProcess");

    let err = shutdown_via_admin_with(
        &run_ctx,
        99999,
        PidIdentity::Unverifiable("cannot inspect".into()),
        &admin,
    )
    .expect_err("Unverifiable returns Err");
    assert!(!marker.exists(), "must not invoke admin for Unverifiable");
    assert!(err.to_string().contains("unverifiable"), "{err}");
}

#[cfg(unix)]
#[test]
fn admin_shutdown_missing_credential_or_failed_client_returns_err_and_cleans_up() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let rt = tmp.path().join("rt");
    let data = tmp.path().join("inst").join("data");
    let run = tmp.path().join("inst").join("run");
    let logs = tmp.path().join("inst").join("logs");
    fs::create_dir_all(rt.join("bin")).expect("bin");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&run).expect("run");
    fs::create_dir_all(&logs).expect("logs");

    let admin = rt.join("bin").join("mariadb-admin");
    fs::write(&admin, "#!/bin/sh\necho 'Access denied' >&2\nexit 1\n").expect("write");
    fs::set_permissions(&admin, fs::Permissions::from_mode(0o755)).expect("chmod");

    let no_cred_ctx = RunContext {
        instance_name: "m3".into(),
        runtime_dir: rt.clone(),
        data_dir: data.clone(),
        run_dir: run.clone(),
        logs_dir: logs.clone(),
        port: Port::new(43308).expect("port"),
        extra_port: None,
        superuser: "root".into(),
        auth: AuthMode::Password,
        credential: None,
    };
    let err = shutdown_via_admin_with(&no_cred_ctx, 12345, PidIdentity::Match, &admin)
        .expect_err("missing credential must fail");
    assert!(err.to_string().contains("credential"), "{err}");

    let pw = SecretString::new("top-secret-42".to_owned());
    let fail_ctx = RunContext {
        credential: Some(Credential {
            username: "root".into(),
            password: pw,
        }),
        ..no_cred_ctx
    };
    let err = shutdown_via_admin_with(&fail_ctx, 12345, PidIdentity::Match, &admin)
        .expect_err("non-zero admin exit must fail");
    assert!(
        !err.to_string().contains("top-secret-42"),
        "secret leaked: {err}"
    );
    let leftover: Vec<_> = fs::read_dir(&run)
        .expect("read run")
        .filter_map(std::result::Result::ok)
        .collect();
    assert!(
        leftover.is_empty(),
        "temp defaults file leaked after failure: {leftover:?}"
    );
}

#[cfg(unix)]
#[test]
fn admin_shutdown_kills_hanging_client_on_timeout_and_cleans_up() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let rt = tmp.path().join("rt");
    let data = tmp.path().join("inst").join("data");
    let run = tmp.path().join("inst").join("run");
    let logs = tmp.path().join("inst").join("logs");
    fs::create_dir_all(rt.join("bin")).expect("bin");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&run).expect("run");
    fs::create_dir_all(&logs).expect("logs");

    let admin = rt.join("bin").join("mariadb-admin");
    fs::write(&admin, "#!/bin/sh\nsleep 10\nexit 0\n").expect("write");
    fs::set_permissions(&admin, fs::Permissions::from_mode(0o755)).expect("chmod");

    let run_ctx = RunContext {
        instance_name: "m-hang".into(),
        runtime_dir: rt,
        data_dir: data,
        run_dir: run.clone(),
        logs_dir: logs,
        port: Port::new(43309).expect("port"),
        extra_port: None,
        superuser: "root".into(),
        auth: AuthMode::None,
        credential: None,
    };

    let started = Instant::now();
    let err = shutdown_via_admin_with_timeout(
        &run_ctx,
        12345,
        PidIdentity::Match,
        &admin,
        Duration::from_millis(200),
    )
    .expect_err("hanging client must time out");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "timeout took too long: {:?}",
        started.elapsed()
    );
    assert!(err.to_string().contains("timed out"), "{err}");

    let leftover: Vec<_> = fs::read_dir(&run)
        .expect("read run")
        .filter_map(std::result::Result::ok)
        .collect();
    assert!(
        leftover.is_empty(),
        "temp defaults file leaked after timeout: {leftover:?}"
    );
}
