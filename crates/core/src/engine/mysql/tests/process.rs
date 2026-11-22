use super::*;

#[test]
fn cleanup_init_file_removes_one_shot_sql() {
    let dir = tempfile::tempdir().expect("tmp");
    let p = dir.path().join("dblocal-init.sql");
    fs::write(&p, "ALTER USER root@localhost IDENTIFIED BY 'x';").expect("write");
    assert!(p.exists());
    cleanup_init_file(&p);
    assert!(!p.exists(), "init file must be removed");
}
#[test]
fn shutdown_command_uses_mysql_pwd_env_not_argv_password_mode() {
    let ctx = run_ctx(42345, AuthMode::Password, Some(SECRET));
    let cmd = shutdown_command(&ctx);
    assert!(cmd.get_program().to_string_lossy().ends_with("mysqladmin"));
    assert_no_secret_in_command(&cmd, SECRET);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(args.iter().any(|a| a == "shutdown"));
    assert!(args.iter().any(|a| a == "--protocol=TCP"));
    let mut saw_mysql_pwd = false;
    for (k, v) in cmd.get_envs() {
        if k.to_string_lossy() == "MYSQL_PWD" {
            let v = v.expect("MYSQL_PWD value").to_string_lossy();
            assert_eq!(v, SECRET);
            saw_mysql_pwd = true;
        }
    }
    assert!(saw_mysql_pwd, "MYSQL_PWD must be set for password auth");
}

#[test]
fn shutdown_command_omits_mysql_pwd_for_no_password_mode() {
    let ctx = run_ctx(42345, AuthMode::None, None);
    let cmd = shutdown_command(&ctx);
    let has_mysql_pwd = cmd
        .get_envs()
        .any(|(k, _)| k.to_string_lossy() == "MYSQL_PWD");
    assert!(
        !has_mysql_pwd,
        "MYSQL_PWD must not be set for AuthMode::None"
    );
}

#[test]
fn bootstrap_shutdown_command_has_no_password_in_argv() {
    let cmd = bootstrap_shutdown_command(
        Path::new("/opt/mysql"),
        42345,
        "root",
        AuthMode::Password,
        Some(&SecretString::from(SECRET)),
    );
    assert_no_secret_in_command(&cmd, SECRET);
    let has_mysql_pwd = cmd.get_envs().any(|(k, v)| {
        k.to_string_lossy() == "MYSQL_PWD"
            && v.map(|v| v.to_string_lossy() == SECRET).unwrap_or(false)
    });
    assert!(has_mysql_pwd);
}

#[test]
fn initialize_insecure_command_has_no_password() {
    let ctx = init_ctx(42345, AuthMode::Password, Some(SECRET));
    let cmd = initialize_insecure_command(&ctx, Path::new("/logs/init.err"));
    assert_no_secret_in_command(&cmd, SECRET);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(args.iter().any(|a| a == "--initialize-insecure"));
    assert!(!args.iter().any(|a| a.contains("--password")));
}

#[test]
fn bootstrap_mysqld_command_has_no_password() {
    let ctx = init_ctx(42345, AuthMode::Password, Some(SECRET));
    let cmd = bootstrap_mysqld_command(
        &ctx,
        Path::new("/run/my.cnf"),
        Path::new("/run/dblocal-init.sql"),
    );
    assert_no_secret_in_command(&cmd, SECRET);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(args.iter().any(|a| a.starts_with("--defaults-file=")));
    assert!(args.iter().any(|a| a.starts_with("--init-file=")));
    assert!(args.iter().any(|a| a == "--daemonize=OFF"));
}

#[test]
fn initialize_insecure_command_failure_cleans_up_and_reports_tail() {
    let dir = tempfile::tempdir().expect("tmp");
    let runtime = dir.path().join("rt");
    let _mysqld = write_fake_bin(
        &runtime,
        "mysqld",
        "#!/bin/sh\necho 'init boom' >&2\nexit 7\n",
    );
    let data = dir.path().join("inst/data");
    let logs = dir.path().join("inst/logs");
    let run = dir.path().join("inst/run");
    fs::create_dir_all(&logs).expect("logs");
    fs::create_dir_all(&run).expect("run");
    let ctx = InitContext {
        instance_name: "app".into(),
        runtime_dir: runtime,
        data_dir: data.clone(),
        port: Port::new(42100).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        password: None,
    };
    let err = retry_etxtbsy(|| MysqlAdapter.initialize(&ctx)).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("init boom") || msg.contains("code"),
        "msg: {msg}"
    );
    assert!(!data.exists(), "data dir should be cleaned on init failure");
    assert!(!run.join("dblocal-init.sql").exists());
}

#[test]
fn initialize_reports_init_err_log_tail() {
    let dir = tempfile::tempdir().expect("tmp");
    let runtime = dir.path().join("rt");
    let _mysqld = write_fake_bin(
        &runtime,
        "mysqld",
        "#!/bin/sh\nlog=\nfor a in \"$@\"; do case \"$a\" in --log-error=*) log=\"${a#--log-error=}\" ;; esac; done\necho 'stderr line' >&2\nif [ -n \"$log\" ]; then echo 'log detail' > \"$log\"; fi\nexit 3\n",
    );
    let _ = &_mysqld;
    let data = dir.path().join("inst/data");
    let logs = dir.path().join("inst/logs");
    let run = dir.path().join("inst/run");
    fs::create_dir_all(&logs).expect("logs");
    fs::create_dir_all(&run).expect("run");
    let ctx = InitContext {
        instance_name: "app".into(),
        runtime_dir: runtime,
        data_dir: data.clone(),
        port: Port::new(42101).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        password: None,
    };
    let err = retry_etxtbsy(|| MysqlAdapter.initialize(&ctx)).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("log detail"), "msg: {msg}");
}

#[test]
fn health_check_maps_connection_refused_to_error() {
    let dir = tempfile::tempdir().expect("tmp");
    let ctx = RunContext {
        instance_name: "app".into(),
        runtime_dir: dir.path().join("rt"),
        data_dir: dir.path().join("data"),
        logs_dir: dir.path().join("logs"),
        run_dir: dir.path().join("run"),
        port: Port::new(42103).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        credential: None,
    };
    let err = MysqlAdapter.health_check(&ctx).unwrap_err();
    assert!(err.to_string().contains("cannot connect") || err.to_string().contains("mysql"));
}

#[test]
fn command_builders_set_tmpdir_env() {
    let ctx_i = init_ctx(42345, AuthMode::Password, Some(SECRET));
    let init_cmd = initialize_insecure_command(&ctx_i, Path::new("/logs/init.err"));
    let tmp = init_cmd
        .get_envs()
        .find(|(k, _)| *k == "TMPDIR")
        .and_then(|(_, v)| v)
        .expect("TMPDIR");
    assert_eq!(tmp.to_string_lossy(), "/run/tmp");

    let boot =
        bootstrap_mysqld_command(&ctx_i, Path::new("/run/my.cnf"), Path::new("/run/init.sql"));
    let tmp = boot
        .get_envs()
        .find(|(k, _)| *k == "TMPDIR")
        .and_then(|(_, v)| v)
        .expect("TMPDIR");
    assert_eq!(tmp.to_string_lossy(), "/run/tmp");

    let ctx_r = run_ctx(42345, AuthMode::Password, Some(SECRET));
    let spawn = MysqlAdapter.spawn_command(&ctx_r);
    let tmp = spawn
        .get_envs()
        .find(|(k, _)| *k == "TMPDIR")
        .and_then(|(_, v)| v)
        .expect("TMPDIR");
    assert_eq!(tmp.to_string_lossy(), "/run/tmp");
}

#[test]
fn health_check_maps_not_ready_banner() {
    use std::io::Write;
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let t = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().expect("accept");
        let payload = b"garbage";
        let mut hdr = [0u8; 4];
        hdr[0] = payload.len() as u8;
        s.write_all(&hdr).expect("hdr");
        s.write_all(payload).expect("payload");
    });
    let ctx = run_ctx(port, AuthMode::None, None);
    let health = MysqlAdapter.health_check(&ctx).expect("health");
    assert!(matches!(health, Health::NotReady(_)), "got {health:?}");
    t.join().expect("join");
}

#[test]
fn read_log_tail_bounds_and_missing_file() {
    let dir = tempfile::tempdir().expect("tmp");
    let p = dir.path().join("mysql.log");
    assert_eq!(read_log_tail(&p, 5), "");
    fs::write(&p, "a\nb\nc\n").expect("write");
    assert_eq!(read_log_tail(&p, 2), "b\nc");
    assert_eq!(read_log_tail(&p, 10).lines().count(), 3);
}

#[test]
fn shutdown_bootstrap_reports_spawn_and_exit_failure() {
    let dir = tempfile::tempdir().expect("tmp");
    let missing = dir.path().join("rt");
    fs::create_dir_all(&missing).expect("rt");
    let err = retry_etxtbsy(|| shutdown_bootstrap(&missing, 42200, "root", AuthMode::None, None))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("failed to shutdown bootstrap mysqld"),
        "{err}"
    );

    let _admin = write_fake_bin(
        &missing,
        "mysqladmin",
        "#!/bin/sh\necho 'boot shutdown bad' >&2\nexit 5\n",
    );
    let err = retry_etxtbsy(|| shutdown_bootstrap(&missing, 42201, "root", AuthMode::None, None))
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("bootstrap shutdown failed"), "{msg}");
    assert!(msg.contains("boot shutdown bad"), "{msg}");
    assert!(msg.contains("code Some(5)"), "{msg}");
}

#[test]
fn run_initialize_insecure_missing_binary_and_early_exit() {
    let dir = tempfile::tempdir().expect("tmp");
    let logs = dir.path().join("logs");
    fs::create_dir_all(&logs).expect("logs");
    let runtime = dir.path().join("rt");
    fs::create_dir_all(&runtime).expect("rt");
    let mut ctx = init_ctx(42202, AuthMode::None, None);
    ctx.runtime_dir = runtime.clone();
    ctx.data_dir = dir.path().join("data");
    let err = retry_etxtbsy(|| run_initialize_insecure(&ctx, &logs)).unwrap_err();
    assert!(
        err.to_string()
            .contains("failed to execute mysqld --initialize-insecure"),
        "{err}"
    );

    let _mysqld = write_fake_bin(
        &runtime,
        "mysqld",
        "#!/bin/sh\necho 'init failed hard' >&2\nexit 4\n",
    );
    let err = retry_etxtbsy(|| run_initialize_insecure(&ctx, &logs)).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("--initialize-insecure failed"), "{msg}");
    assert!(msg.contains("init failed hard"), "{msg}");
}

#[test]
fn bootstrap_credentials_reports_unwritable_init_file_and_early_exit() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tmp");
    let logs = dir.path().join("logs");
    fs::create_dir_all(&logs).expect("logs");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let runtime = dir.path().join("rt");
    let bin = runtime.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let mysqld = bin.join("mysqld");
    fs::write(
        &mysqld,
        "#!/bin/sh\necho 'bootstrap early' >&2\necho 'logline' >/dev/null\nexit 2\n",
    )
    .expect("script");
    fs::set_permissions(&mysqld, fs::Permissions::from_mode(0o755)).expect("chmod");

    let mut ctx = init_ctx(42203, AuthMode::Password, Some(SECRET));
    ctx.runtime_dir = runtime;
    ctx.data_dir = dir.path().join("inst/data");
    fs::create_dir_all(ctx.data_dir.parent().expect("parent")).expect("inst");

    let before = fs::symlink_metadata(&run_dir).expect("meta");
    fs::set_permissions(&run_dir, fs::Permissions::from_mode(0o500)).expect("chmod");
    let err = retry_etxtbsy(|| bootstrap_credentials(&ctx, &run_dir, &logs)).unwrap_err();
    assert!(err.to_string().contains("cannot write init SQL"), "{err}");
    fs::set_permissions(
        &run_dir,
        fs::Permissions::from_mode(before.permissions().mode()),
    )
    .expect("restore");

    let err = retry_etxtbsy(|| bootstrap_credentials(&ctx, &run_dir, &logs)).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("mysqld bootstrap exited early"), "{msg}");
    assert!(msg.contains("bootstrap early"), "{msg}");
}
