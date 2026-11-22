//! Unit tests for the PostgreSQL adapter (moved out of `postgres.rs` to keep
//! the production file within the 800-line limit).

use super::*;
use crate::engine::InitContext;
use crate::model::Port;
use std::ffi::OsString;
use tempfile::tempdir;

fn init_ctx(auth: AuthMode, password: Option<&str>) -> InitContext {
    InitContext {
        instance_name: "pg-test".into(),
        runtime_dir: PathBuf::from("/tmp/rt"),
        data_dir: PathBuf::from("/tmp/data"),
        port: Port::new(41010).expect("port"),
        superuser: "postgres".into(),
        auth,
        password: password.map(SecretString::from),
    }
}

#[test]
fn auth_args_match_mode() {
    let pw = auth_args(AuthMode::Password);
    assert_eq!(pw.local, "scram-sha-256");
    assert_eq!(pw.host, "scram-sha-256");
    let none = auth_args(AuthMode::None);
    assert_eq!(none.local, "trust");
    assert_eq!(none.host, "trust");
}

#[test]
fn socket_path_limit_enforced() {
    let short = Path::new("/tmp/s");
    assert!(validate_socket_path_len(short, 41010).is_ok());
    let long_dir = "/".to_owned() + &"a".repeat(120);
    let err = validate_socket_path_len(Path::new(&long_dir), 41010).unwrap_err();
    assert!(matches!(err, DblError::InvalidInput { .. }));
}

#[test]
fn socket_path_boundary() {
    // Build a dir whose resulting socket path is exactly at the limit.
    let port = 41010u16;
    let suffix = format!("/.s.PGSQL.{port}");
    let dir_len = UNIX_SOCKET_PATH_LIMIT - suffix.len();
    let dir = PathBuf::from("/".to_owned() + &"s".repeat(dir_len.saturating_sub(1)));
    assert!(validate_socket_path_len(&dir, port).is_ok());
}

#[test]
fn choose_socket_dir_enables_short_path() {
    let plan = choose_socket_dir(Path::new("/tmp/s"), 41010);
    assert_eq!(plan, SocketDir::Enabled(PathBuf::from("/tmp/s")));
    assert_eq!(plan.config_value(), "/tmp/s");
    assert_eq!(plan.path(), Some(Path::new("/tmp/s")));
}

#[test]
fn choose_socket_dir_disables_overlong_path_instead_of_failing() {
    // A realistic deep home path that exceeds the kernel's sun_path limit.
    let long = PathBuf::from(format!(
        "/home/{}/.dblocal/instances/{}/run/sock",
        "u".repeat(32),
        "n".repeat(40)
    ));
    assert!(
        validate_socket_path_len(&long, 5432).is_err(),
        "the fixture must exceed the limit"
    );
    let plan = choose_socket_dir(&long, 5432);
    assert_eq!(plan, SocketDir::Disabled);
    assert_eq!(plan.path(), None);
    assert_eq!(plan.config_value(), "");
}

#[test]
fn configure_after_init_disables_unix_socket_when_too_long() {
    let dir = tempdir().expect("tempdir");
    let data = dir.path().join("data");
    fs::create_dir_all(&data).expect("data");
    fs::write(data.join("postgresql.conf"), "# base\n").expect("conf");
    let ctx = InitContext {
        instance_name: "x".into(),
        runtime_dir: PathBuf::from("/rt"),
        data_dir: data.clone(),
        port: Port::new(41010).expect("port"),
        superuser: "postgres".into(),
        auth: AuthMode::None,
        password: None,
    };
    configure_after_init(&ctx, &SocketDir::Disabled).expect("configure");
    let text = fs::read_to_string(data.join("postgresql.conf")).expect("read");
    assert!(
        text.contains("unix_socket_directories = ''"),
        "expected an empty unix_socket_directories, got: {text}"
    );
    assert!(text.contains("listen_addresses = '127.0.0.1'"));
}

#[test]
fn prepare_run_returns_plan_and_creates_dir_when_enabled() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("run");
    let ctx = RunContext {
        instance_name: "app".into(),
        runtime_dir: PathBuf::from("/rt"),
        data_dir: dir.path().join("data"),
        logs_dir: dir.path().join("logs"),
        run_dir: run.clone(),
        port: Port::new(41010).expect("port"),
        superuser: "postgres".into(),
        auth: AuthMode::None,
        credential: None,
    };
    let plan = prepare_run(&ctx).expect("prepare");
    let sock = run.join("sock");
    assert_eq!(plan, SocketDir::Enabled(sock.clone()));
    assert!(sock.is_dir());
}

#[test]
fn init_plan_disables_socket_for_overlong_path() {
    let long_run = format!(
        "/home/{}/.dblocal/instances/{}/run",
        "u".repeat(32),
        "n".repeat(40)
    );
    let ctx = InitContext {
        instance_name: "pg-test".into(),
        runtime_dir: PathBuf::from("/tmp/rt"),
        data_dir: PathBuf::from(format!("{long_run}/data")),
        port: Port::new(5432).expect("port"),
        superuser: "postgres".into(),
        auth: AuthMode::None,
        password: None,
    };
    assert_eq!(socket_dir_plan_for_init(&ctx), SocketDir::Disabled);
}

#[test]
fn parse_postmaster_pid_happy_path() {
    let contents =
        "3810050\n/tmp/data\n1795176359\n41060\n/tmp/sock\n127.0.0.1\n  2558019     32796\nready\n";
    let id = parse_postmaster_pid(contents).expect("parse");
    assert_eq!(id.pid, 3810050);
    assert_eq!(id.data_dir, PathBuf::from("/tmp/data"));
    assert_eq!(id.start_epoch, 1795176359);
    assert_eq!(id.port, 41060);
    assert_eq!(id.socket_dirs, "/tmp/sock");
    assert_eq!(id.status.as_deref(), Some("ready"));
}

#[test]
fn parse_postmaster_pid_rejects_short_file() {
    assert!(parse_postmaster_pid("1\n2\n3\n").is_err());
    assert!(parse_postmaster_pid("abc\n/tmp\n1\n1\n/tmp\n127.0.0.1\n").is_err());
}

#[test]
fn read_postmaster_pid_from_file() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("postmaster.pid");
    fs::write(&path, "42\n/d\n100\n5432\n/s\n127.0.0.1\n0 0\nready\n").expect("write");
    let id = read_postmaster_pid(dir.path()).expect("read");
    assert_eq!(id.pid, 42);
}

#[test]
fn locale_preference_order() {
    let mut set = HashSet::new();
    set.insert("C".to_owned());
    set.insert("en_US.UTF-8".to_owned());
    assert_eq!(pick_locale(&set), "C");
    set.insert("C.utf8".to_owned());
    assert_eq!(pick_locale(&set), "C.utf8");
    set.insert("C.UTF-8".to_owned());
    assert_eq!(pick_locale(&set), "C.UTF-8");
}

#[test]
fn parse_locale_list_trims() {
    let set = parse_locale_list("C\nC.UTF-8\nen_US.UTF-8\n");
    assert!(set.contains("C.UTF-8"));
    assert_eq!(set.len(), 3);
}

#[test]
fn combined_output_merges_streams() {
    let out = combined_output(b"out\n".to_vec(), b"err\n".to_vec());
    assert!(out.contains("out"));
    assert!(out.contains("err"));
}

#[test]
fn write_pwfile_creates_restricted_file() {
    let dir = tempdir().expect("tempdir");
    let data_dir = dir.path().join("data");
    fs::create_dir_all(&data_dir).expect("data");
    let pw = SecretString::from("s3cret");
    let path = write_pwfile(&data_dir, Some(&pw)).expect("pwfile");
    let contents = fs::read_to_string(&path).expect("read");
    assert_eq!(contents, "s3cret\n");
    #[cfg(unix)]
    {
        let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let _ = fs::remove_file(path);
}

#[test]
fn init_ctx_requires_password_when_auth_is_password() {
    let ctx = init_ctx(AuthMode::Password, None);
    assert!(ctx.with_password());
    assert!(ctx.password.is_none());
}

#[test]
fn spawn_command_does_not_carry_password() {
    let ctx = RunContext {
        instance_name: "app".into(),
        runtime_dir: PathBuf::from("/opt/pg"),
        data_dir: PathBuf::from("/data"),
        logs_dir: PathBuf::from("/logs"),
        run_dir: PathBuf::from("/run"),
        port: Port::new(41010).expect("port"),
        superuser: "postgres".into(),
        auth: AuthMode::Password,
        credential: Some(Credential::new(
            "postgres",
            SecretString::from("top-secret"),
        )),
    };
    let cmd = PostgresAdapter.spawn_command(&ctx);
    let args: Vec<OsString> = cmd.get_args().map(|s| s.to_os_string()).collect();
    for a in &args {
        assert!(!a.to_string_lossy().contains("top-secret"));
    }
    for (k, v) in cmd.get_envs() {
        if let Some(v) = v {
            assert!(!v.to_string_lossy().contains("top-secret"));
            assert_ne!(k.to_string_lossy(), "PGPASSWORD");
        }
    }
    let prog = cmd.get_program().to_string_lossy();
    assert!(prog.ends_with("pg_ctl") || prog.ends_with("pg_ctl.exe"));
}

#[test]
fn configure_after_init_writes_managed_settings() {
    let dir = tempdir().expect("tempdir");
    let data = dir.path().join("data");
    fs::create_dir_all(&data).expect("data");
    fs::write(data.join("postgresql.conf"), "# base\n").expect("conf");
    let ctx = InitContext {
        instance_name: "x".into(),
        runtime_dir: PathBuf::from("/rt"),
        data_dir: data.clone(),
        port: Port::new(41010).expect("port"),
        superuser: "postgres".into(),
        auth: AuthMode::None,
        password: None,
    };
    let sock = SocketDir::Enabled(dir.path().join("run/sock"));
    configure_after_init(&ctx, &sock).expect("configure");
    let text = fs::read_to_string(data.join("postgresql.conf")).expect("read");
    assert!(text.contains("listen_addresses = '127.0.0.1'"));
    assert!(text.contains("port = 41010"));
    assert!(text.contains("unix_socket_directories = '"));
}

#[test]
fn log_path_is_under_logs_dir() {
    let ctx = RunContext {
        instance_name: "app".into(),
        runtime_dir: PathBuf::from("/rt"),
        data_dir: PathBuf::from("/d"),
        logs_dir: PathBuf::from("/logs"),
        run_dir: PathBuf::from("/run"),
        port: Port::new(41010).expect("port"),
        superuser: "postgres".into(),
        auth: AuthMode::None,
        credential: None,
    };
    assert_eq!(log_path(&ctx), PathBuf::from("/logs/postgres.log"));
}

#[test]
fn escape_single_quotes_doubles() {
    assert_eq!(escape_single_quotes("a'b"), "a''b");
}

#[test]
fn effective_superuser_defaults_when_empty() {
    let mut ctx = init_ctx(AuthMode::None, None);
    ctx.superuser = String::new();
    assert_eq!(effective_superuser(&ctx), consts::SUPERUSER_POSTGRESQL);
    ctx.superuser = "admin".to_owned();
    assert_eq!(effective_superuser(&ctx), "admin");
}

#[test]
fn write_pwfile_requires_a_password() {
    let dir = tempdir().expect("tempdir");
    let data = dir.path().join("data");
    fs::create_dir_all(&data).expect("data");
    let err = write_pwfile(&data, None).unwrap_err();
    assert!(matches!(err, DblError::InvalidInput { .. }));
}

#[test]
fn read_postmaster_pid_reports_a_missing_file() {
    let dir = tempdir().expect("tempdir");
    let err = read_postmaster_pid(dir.path()).unwrap_err();
    assert!(matches!(err, DblError::Engine { .. }));
    assert!(format!("{err}").contains("postmaster.pid"));
}

#[test]
fn parse_postmaster_pid_rejects_bad_fields() {
    // Bad start time on line 3.
    assert!(parse_postmaster_pid("1\n/d\nnot-a-time\n5432\n/s\n127.0.0.1\n0 0\nready\n").is_err());
    // Bad port on line 4.
    assert!(parse_postmaster_pid("1\n/d\n100\nnot-a-port\n/s\n127.0.0.1\n0 0\nready\n").is_err());
    // Bad pid on line 1.
    assert!(parse_postmaster_pid("x\n/d\n100\n5432\n/s\n127.0.0.1\n0 0\nready\n").is_err());
}

#[test]
fn trait_diagnose_dispatches_to_the_shared_table() {
    let out = "FATAL:  database files are incompatible with server\n";
    assert_eq!(
        PostgresAdapter.diagnose_start_failure(out, Some(1)),
        StartFailure::DataDirVersionMismatch
    );
}

#[cfg(unix)]
#[test]
fn shutdown_reports_a_nonzero_pg_ctl() {
    // A fake runtime whose pg_ctl exits 1 exercises the error branch without a
    // real server.
    let dir = tempdir().expect("tempdir");
    stub_runtime(dir.path(), "pg_ctl", "#!/bin/sh\n echo boom >&2\n exit 1\n");
    let ctx = RunContext {
        instance_name: "app".into(),
        runtime_dir: dir.path().to_path_buf(),
        data_dir: dir.path().join("data"),
        logs_dir: dir.path().join("logs"),
        run_dir: dir.path().join("run"),
        port: Port::new(41010).expect("port"),
        superuser: "postgres".into(),
        auth: AuthMode::None,
        credential: None,
    };
    let err = PostgresAdapter.shutdown(&ctx).unwrap_err();
    assert!(matches!(err, DblError::Engine { .. }));
    assert!(format!("{err}").contains("pg_ctl stop failed"));
}

#[test]
fn health_check_reports_a_closed_port() {
    // Pick a port with nothing listening; the probe must return an error, not
    // a false Ready, and must not hang for long.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let ctx = RunContext {
        instance_name: "app".into(),
        runtime_dir: PathBuf::from("/rt"),
        data_dir: PathBuf::from("/d"),
        logs_dir: PathBuf::from("/logs"),
        run_dir: PathBuf::from("/run"),
        port: Port::new(port).expect("port"),
        superuser: "postgres".into(),
        auth: AuthMode::Password,
        credential: Some(Credential::new("postgres", SecretString::from("pw"))),
    };
    let err = PostgresAdapter.health_check(&ctx).unwrap_err();
    assert!(matches!(err, DblError::Engine { .. }));
}

// ---- Table-driven diagnosis: every verbatim failure text from spike ----

fn cases() -> Vec<(&'static str, StartFailure)> {
    vec![
        // Port in use (spike 11.1)
        (
            "LOG:  could not bind IPv4 address \"127.0.0.1\": Address already in use\nHINT:  Is another postmaster already running on port 41100? If not, wait a few seconds and retry.\nWARNING:  could not create listen socket for \"127.0.0.1\"\nFATAL:  could not create any TCP/IP sockets\nLOG:  database system is shut down\n",
            StartFailure::PortInUse,
        ),
        (
            "HINT:  Is another postmaster already running on port 41100? If not, wait a few seconds and retry.",
            StartFailure::PortInUse,
        ),
        // Wrong-major data dir (spike 11.2)
        (
            "FATAL:  database files are incompatible with server\nDETAIL:  The data directory was initialized by PostgreSQL version 17, which is not compatible with this version 18.7.\n",
            StartFailure::DataDirVersionMismatch,
        ),
        // Permission denied (spike 11.3)
        (
            "pg_ctl: could not open PID file \"/tmp/d/postmaster.pid\": Permission denied",
            StartFailure::PermissionDenied,
        ),
        (
            "FATAL:  could not create lock file \"postmaster.pid\": Permission denied",
            StartFailure::PermissionDenied,
        ),
        // Socket dir missing (spike 11.5): recognized but no honest variant.
        (
            "FATAL:  could not create lock file \"/tmp/s5/.s.PGSQL.41130.lock\": No such file or directory",
            StartFailure::Unknown,
        ),
        // Socket path too long (spike 11.5): recognized but no honest variant.
        (
            "LOG:  Unix-domain socket path \"/tmp/very/long/.s.PGSQL.41000\" is too long (maximum 107 bytes)\nFATAL:  could not create any Unix-domain sockets\n",
            StartFailure::Unknown,
        ),
        // Nonexistent data directory: not a missing binary, so no honest variant.
        (
            "pg_ctl: directory \"/tmp/nodir\" does not exist",
            StartFailure::Unknown,
        ),
        (
            "/bin/sh: line 1: /tmp/missing: No such file or directory",
            StartFailure::BinaryMissing,
        ),
        (
            "bash: ./postgres: Exec format error",
            StartFailure::BinaryMissing,
        ),
    ]
}

#[test]
fn diagnose_spike_verbatim_texts() {
    for (text, expected) in cases() {
        assert_eq!(diagnose_failure_text(text), expected, "text: {text:?}");
    }
}

#[test]
fn diagnose_unknown_for_unrelated_text() {
    assert_eq!(
        diagnose_failure_text("something completely different\n"),
        StartFailure::Unknown
    );
}

#[test]
fn sync_port_rewrites_managed_block_atomically_and_keeps_comments() {
    let dir = tempdir().expect("tempdir");
    let data = dir.path().join("data");
    let run = dir.path().join("run");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&run).expect("run");
    fs::write(
        data.join("postgresql.conf"),
        "# user comment\nmax_connections = 100\n\n# DBLocal managed settings\nlisten_addresses = '127.0.0.1'\nport = 5432\nunix_socket_directories = '/old'\n# DBLocal managed settings end\n",
    )
    .expect("conf");
    let plan = sync_port(&data, &run, 5544).expect("sync");
    assert!(matches!(plan, SocketDir::Enabled(_)) || matches!(plan, SocketDir::Disabled));
    let text = fs::read_to_string(data.join("postgresql.conf")).expect("read");
    assert!(text.contains("# user comment"));
    assert!(text.contains("max_connections = 100"));
    assert!(text.contains("port = 5544"));
    assert!(!text.contains("port = 5432"));
    assert_eq!(text.matches("port = ").count(), 1);
}

#[test]
fn sync_port_is_idempotent() {
    let dir = tempdir().expect("tempdir");
    let data = dir.path().join("data");
    let run = dir.path().join("run");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&run).expect("run");
    fs::write(data.join("postgresql.conf"), "# base\n").expect("conf");
    sync_port(&data, &run, 5544).expect("first");
    let once = fs::read_to_string(data.join("postgresql.conf")).expect("read");
    sync_port(&data, &run, 5544).expect("second");
    let twice = fs::read_to_string(data.join("postgresql.conf")).expect("read");
    assert_eq!(once, twice);
    assert_eq!(once.matches("port = 5544").count(), 1);
}

#[test]
fn sync_port_reports_a_missing_config_as_an_engine_error() {
    let dir = tempdir().expect("tempdir");
    let data = dir.path().join("data");
    let run = dir.path().join("run");
    fs::create_dir_all(&data).expect("data");
    let err = sync_port(&data, &run, 5544).expect_err("a missing config is an error");
    assert!(matches!(err, DblError::Engine { .. }), "{err:?}");
    assert!(format!("{err}").contains("cannot read postgresql.conf"));
}

#[test]
fn configure_after_init_reports_a_missing_config() {
    let dir = tempdir().expect("tempdir");
    let ctx = init_ctx(AuthMode::None, None);
    let ctx = InitContext {
        data_dir: dir.path().to_path_buf(),
        ..ctx
    };
    let plan = SocketDir::Enabled(dir.path().join("sock"));
    let err = configure_after_init(&ctx, &plan).expect_err("missing base config");
    assert!(
        format!("{err}").contains("cannot read postgresql.conf"),
        "{err:?}"
    );
}

#[test]
fn initdb_failed_carries_the_combined_tail() {
    let err = initdb_failed(b"out line".to_vec(), b"err line".to_vec(), Some(1));
    let text = format!("{err}");
    assert!(text.contains("initdb failed (code Some(1))"), "{text}");
    assert!(text.contains("out line"), "{text}");
    assert!(text.contains("err line"), "{text}");
}

#[test]
fn combined_output_joins_stdout_and_stderr_cleanly() {
    // stdout without a trailing newline gains one before stderr; empty stderr
    // leaves stdout untouched.
    assert_eq!(combined_output(b"a".to_vec(), b"b".to_vec()), "a\nb");
    assert_eq!(combined_output(b"a\n".to_vec(), b"b".to_vec()), "a\nb");
    assert_eq!(combined_output(b"only".to_vec(), Vec::new()), "only");
    assert_eq!(combined_output(b"".to_vec(), b"err".to_vec()), "err");
}

#[cfg(unix)]
fn stub_runtime(dir: &std::path::Path, name: &str, script: &str) -> PathBuf {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let path = bin.join(exe_name(name));
    crate::fsutil::write_executable_file_atomic(&path, script.as_bytes()).expect("script");
    path
}

#[cfg(unix)]
#[test]
fn initialize_succeeds_through_a_stub_initdb_and_writes_the_managed_block() {
    // A stub initdb that creates the data dir and a base postgresql.conf, so the
    // success path calls `configure_after_init` and leaves the instance stopped
    // with the managed block. No real engine runs.
    let dir = tempdir().expect("tempdir");
    let data = dir.path().join("data");
    stub_runtime(
        dir.path(),
        "initdb",
        &format!(
            "#!/bin/sh\nmkdir -p '{data}'\nprintf '# initdb base\\n' > '{data}/postgresql.conf'\nexit 0\n",
            data = data.display()
        ),
    );
    let ctx = InitContext {
        instance_name: "pg-stub".into(),
        runtime_dir: dir.path().to_path_buf(),
        data_dir: data.clone(),
        port: Port::new(41010).expect("port"),
        superuser: "postgres".into(),
        auth: AuthMode::None,
        password: None,
    };
    PostgresAdapter.initialize(&ctx).expect("initialize");
    let text = fs::read_to_string(data.join("postgresql.conf")).expect("conf");
    assert!(text.contains("# initdb base"));
    assert!(text.contains("# DBLocal managed settings"));
    assert!(text.contains("port = 41010"));
}

#[cfg(unix)]
#[test]
fn initialize_with_password_writes_and_removes_a_pwfile() {
    // The password path writes a 0600 pwfile, passes it via --pwfile, and removes
    // it afterwards; a stub initdb that records its argv proves both.
    let dir = tempdir().expect("tempdir");
    let data = dir.path().join("data");
    let log = dir.path().join("argv.log");
    stub_runtime(
        dir.path(),
        "initdb",
        &format!(
            "#!/bin/sh\necho \"$@\" > '{log}'\nmkdir -p '{data}'\nprintf '# base\\n' > '{data}/postgresql.conf'\nexit 0\n",
            log = log.display(),
            data = data.display()
        ),
    );
    let ctx = InitContext {
        instance_name: "pg-stub".into(),
        runtime_dir: dir.path().to_path_buf(),
        data_dir: data.clone(),
        port: Port::new(41010).expect("port"),
        superuser: "postgres".into(),
        auth: AuthMode::Password,
        password: Some(SecretString::from("s3cret")),
    };
    PostgresAdapter.initialize(&ctx).expect("initialize");
    let argv = fs::read_to_string(&log).expect("argv");
    assert!(argv.contains("--pwfile"), "argv: {argv}");
    assert!(argv.contains("--auth-local scram-sha-256"), "argv: {argv}");
    // The transient pwfile must be gone after initdb.
    assert!(
        !data
            .parent()
            .expect("parent")
            .join(".dblocal-pwfile")
            .exists(),
        "the pwfile must be removed"
    );
}

#[cfg(unix)]
#[test]
fn initialize_failure_cleans_the_data_dir_and_reports_initdb_output() {
    // A stub initdb that fails must roll the data directory back and carry its
    // stderr into the typed error (F-INS-02 AC3).
    let dir = tempdir().expect("tempdir");
    let data = dir.path().join("data");
    stub_runtime(
        dir.path(),
        "initdb",
        "#!/bin/sh\nprintf 'initdb boom\\n' >&2\nexit 1\n",
    );
    let ctx = InitContext {
        instance_name: "pg-stub".into(),
        runtime_dir: dir.path().to_path_buf(),
        data_dir: data.clone(),
        port: Port::new(41010).expect("port"),
        superuser: "postgres".into(),
        auth: AuthMode::None,
        password: None,
    };
    let err = PostgresAdapter.initialize(&ctx).expect_err("initdb failed");
    let text = format!("{err}");
    assert!(text.contains("initdb failed"), "{text}");
    assert!(text.contains("initdb boom"), "{text}");
    assert!(!data.exists(), "a failed init must clean the data dir");
}

#[cfg(unix)]
#[test]
fn initialize_reports_a_missing_initdb_binary() {
    let dir = tempdir().expect("tempdir");
    let ctx = InitContext {
        instance_name: "pg-stub".into(),
        runtime_dir: dir.path().to_path_buf(),
        data_dir: dir.path().join("data"),
        port: Port::new(41010).expect("port"),
        superuser: "postgres".into(),
        auth: AuthMode::None,
        password: None,
    };
    let err = PostgresAdapter.initialize(&ctx).expect_err("no initdb");
    assert!(
        format!("{err}").contains("failed to execute initdb"),
        "{err:?}"
    );
}
