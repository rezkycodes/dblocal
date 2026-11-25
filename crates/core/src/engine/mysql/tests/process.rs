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
#[cfg(unix)]
#[test]
fn shutdown_happy_path_passes_defaults_file_and_cleans_up() {
    let dir = tempfile::tempdir().expect("tmp");
    let runtime = dir.path().join("rt");
    let data = dir.path().join("inst/data");
    let run = dir.path().join("inst/run");
    let logs = dir.path().join("inst/logs");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&run).expect("run");
    fs::create_dir_all(&logs).expect("logs");

    let capture = dir.path().join("captured.txt");
    let script = format!(
        "#!/bin/sh\n\
         echo \"ARGS:$*\" > \"{}\"\n\
         echo \"MYSQL_PWD:${{MYSQL_PWD:-UNSET}}\" >> \"{}\"\n\
         echo \"MYSQL_TCP_PORT:${{MYSQL_TCP_PORT:-UNSET}}\" >> \"{}\"\n\
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
        capture.display(),
    );
    let _admin = write_fake_bin(&runtime, "mysqladmin", &script);

    let ctx = RunContext {
        instance_name: "m1".into(),
        runtime_dir: runtime,
        data_dir: data,
        logs_dir: logs,
        run_dir: run.clone(),
        port: Port::new(42345).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::Password,
        credential: Some(Credential::new("root", SecretString::from(SECRET))),
    };

    MysqlAdapter
        .shutdown(&ctx)
        .expect("mysqladmin shutdown should succeed");

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
        first_line.contains("-h 127.0.0.1 -P 42345 --protocol=TCP shutdown"),
        "unexpected args: {first_line}"
    );
    assert!(
        !first_line.contains(SECRET),
        "password must not appear in argv: {first_line}"
    );
    assert!(
        captured.contains("MYSQL_PWD:UNSET"),
        "MYSQL_PWD must not be set: {captured}"
    );
    assert!(
        captured.contains("MYSQL_TCP_PORT:UNSET"),
        "MYSQL_TCP_PORT must not be set: {captured}"
    );
    assert!(
        captured.contains(&format!("password=\"{SECRET}\"")),
        "defaults file must carry password: {captured}"
    );
    let leftover: Vec<_> = fs::read_dir(&run)
        .expect("read run")
        .filter_map(std::result::Result::ok)
        .collect();
    assert!(leftover.is_empty(), "temp files leaked: {leftover:?}");
}

#[cfg(unix)]
#[test]
fn shutdown_missing_credential_or_failed_client_returns_err_and_cleans_up() {
    let dir = tempfile::tempdir().expect("tmp");
    let runtime = dir.path().join("rt");
    let data = dir.path().join("inst/data");
    let run = dir.path().join("inst/run");
    let logs = dir.path().join("inst/logs");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&run).expect("run");
    fs::create_dir_all(&logs).expect("logs");

    let _admin = write_fake_bin(
        &runtime,
        "mysqladmin",
        &format!("#!/bin/sh\necho 'Access denied {SECRET}' >&2\nexit 1\n"),
    );

    let no_cred_ctx = RunContext {
        instance_name: "m2".into(),
        runtime_dir: runtime,
        data_dir: data,
        logs_dir: logs,
        run_dir: run.clone(),
        port: Port::new(42346).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::Password,
        credential: None,
    };
    let err = MysqlAdapter
        .shutdown(&no_cred_ctx)
        .expect_err("missing credential must fail");
    assert!(err.to_string().contains("credential"), "{err}");

    let fail_ctx = RunContext {
        credential: Some(Credential::new("root", SecretString::from(SECRET))),
        ..no_cred_ctx
    };
    let err = MysqlAdapter
        .shutdown(&fail_ctx)
        .expect_err("non-zero mysqladmin exit must fail");
    let msg = err.to_string();
    assert!(msg.contains("Access denied"), "{msg}");
    assert!(!msg.contains(SECRET), "secret leaked in error: {msg}");

    let leftover: Vec<_> = fs::read_dir(&run)
        .expect("read run")
        .filter_map(std::result::Result::ok)
        .collect();
    assert!(
        leftover.is_empty(),
        "temp files leaked after failure: {leftover:?}"
    );
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
    let cmd = bootstrap_mysqld_command_for(
        &ctx,
        Path::new("/run/my.cnf"),
        Path::new("/run/dblocal-init.sql"),
        false,
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

/// `mysqld.exe` aborts on `--daemonize` ("unknown variable"), so the Windows
/// bootstrap must not pass it; the rest of the command is unchanged.
#[test]
fn windows_bootstrap_command_has_no_daemonize_option() {
    let ctx = init_ctx(42345, AuthMode::Password, Some(SECRET));
    let cmd = bootstrap_mysqld_command_for(
        &ctx,
        Path::new("/run/my.cnf"),
        Path::new("/run/dblocal-init.sql"),
        true,
    );
    assert_no_secret_in_command(&cmd, SECRET);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        !args.iter().any(|a| a.starts_with("--daemonize")),
        "{args:?}"
    );
    assert!(args.iter().any(|a| a.starts_with("--defaults-file=")));
    assert!(args.iter().any(|a| a.starts_with("--init-file=")));
}

/// A fake `mysqld` is a `#!/bin/sh` script, which Windows cannot exec, so this
/// failure round-trip is proved on POSIX hosts only.
#[cfg(unix)]
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

/// A fake `mysqld` is a `#!/bin/sh` script, which Windows cannot exec, so the
/// log-tail round-trip is proved on POSIX hosts only.
#[cfg(unix)]
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
    // The expected value uses the host separator (`run\tmp` on Windows), which
    // is what `tmpdir_path` produces there; a POSIX literal failed on Windows.
    let expected = tmpdir_path(Path::new("/run"));
    let init_cmd = initialize_insecure_command(&ctx_i, Path::new("/logs/init.err"));
    let tmp = init_cmd
        .get_envs()
        .find(|(k, _)| *k == "TMPDIR")
        .and_then(|(_, v)| v)
        .expect("TMPDIR");
    assert_eq!(Path::new(tmp), expected);

    let boot =
        bootstrap_mysqld_command(&ctx_i, Path::new("/run/my.cnf"), Path::new("/run/init.sql"));
    let tmp = boot
        .get_envs()
        .find(|(k, _)| *k == "TMPDIR")
        .and_then(|(_, v)| v)
        .expect("TMPDIR");
    assert_eq!(Path::new(tmp), expected);

    let ctx_r = run_ctx(42345, AuthMode::Password, Some(SECRET));
    let spawn = MysqlAdapter.spawn_command(&ctx_r);
    let tmp = spawn
        .get_envs()
        .find(|(k, _)| *k == "TMPDIR")
        .and_then(|(_, v)| v)
        .expect("TMPDIR");
    assert_eq!(Path::new(tmp), expected);
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

/// The second half runs a fake `mysqladmin` that is a `#!/bin/sh` script, which
/// Windows cannot exec, so the whole round-trip is proved on POSIX hosts only.
#[cfg(unix)]
#[test]
fn shutdown_bootstrap_reports_spawn_and_exit_failure() {
    let dir = tempfile::tempdir().expect("tmp");
    let missing = dir.path().join("rt");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&missing).expect("rt");
    fs::create_dir_all(&run_dir).expect("run");
    let err = retry_etxtbsy(|| {
        shutdown_bootstrap(&missing, &run_dir, 42200, "root", AuthMode::None, None)
    })
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("failed to execute mysqladmin shutdown"),
        "{err}"
    );

    let _admin = write_fake_bin(
        &missing,
        "mysqladmin",
        "#!/bin/sh\necho 'boot shutdown bad' >&2\nexit 5\n",
    );
    let err = retry_etxtbsy(|| {
        shutdown_bootstrap(&missing, &run_dir, 42201, "root", AuthMode::None, None)
    })
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("mysqladmin shutdown failed"), "{msg}");
    assert!(msg.contains("boot shutdown bad"), "{msg}");
    assert!(msg.contains("code Some(5)"), "{msg}");
}

/// The early-exit half runs a fake `mysqld` that is a `#!/bin/sh` script, which
/// Windows cannot exec, so the whole round-trip is proved on POSIX hosts only.
#[cfg(unix)]
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

/// The unwritable-parent half of this test uses POSIX mode bits; Windows would
/// need an ACL to express it, so the case is POSIX-only.
#[cfg(unix)]
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

#[cfg(unix)]
#[test]
fn shutdown_kills_hanging_client_on_timeout_and_cleans_up() {
    let dir = tempfile::tempdir().expect("tmp");
    let runtime = dir.path().join("rt");
    let data = dir.path().join("inst/data");
    let run = dir.path().join("inst/run");
    let logs = dir.path().join("inst/logs");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&run).expect("run");
    fs::create_dir_all(&logs).expect("logs");
    let _admin = write_fake_bin(&runtime, "mysqladmin", "#!/bin/sh\nsleep 10\nexit 0\n");

    let ctx = RunContext {
        instance_name: "m-hang".into(),
        runtime_dir: runtime,
        data_dir: data,
        logs_dir: logs,
        run_dir: run.clone(),
        port: Port::new(43319).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        credential: None,
    };

    let (tx, rx) = std::sync::mpsc::channel();
    let ctx_thread = ctx.clone();
    let started = std::time::Instant::now();
    std::thread::spawn(move || {
        let _ = tx.send(shutdown_with_timeout(
            &ctx_thread,
            Duration::from_millis(200),
        ));
    });

    let result = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("mysqladmin shutdown hung past test bound");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "timeout took too long: {:?}",
        started.elapsed()
    );
    let err = result.expect_err("hanging client must time out");
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
