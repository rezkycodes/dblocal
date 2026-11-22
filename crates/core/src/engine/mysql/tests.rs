use super::*;
use crate::engine::Credential;
use crate::model::Port;

const SECRET: &str = "S3cret-Canary-Pw-MySql!9x";

fn init_ctx(port: u16, auth: AuthMode, password: Option<&str>) -> InitContext {
    InitContext {
        instance_name: "app".into(),
        runtime_dir: PathBuf::from("/opt/mysql"),
        data_dir: PathBuf::from("/data"),
        port: Port::new(port).expect("port"),
        superuser: "root".into(),
        auth,
        password: password.map(SecretString::from),
    }
}

fn run_ctx(port: u16, auth: AuthMode, password: Option<&str>) -> RunContext {
    RunContext {
        instance_name: "app".into(),
        runtime_dir: PathBuf::from("/opt/mysql"),
        data_dir: PathBuf::from("/data"),
        logs_dir: PathBuf::from("/logs"),
        run_dir: PathBuf::from("/run"),
        port: Port::new(port).expect("port"),
        superuser: "root".into(),
        auth,
        credential: password.map(|p| Credential::new("root", SecretString::from(p))),
    }
}

fn assert_no_secret_in_command(cmd: &Command, secret: &str) {
    let prog = cmd.get_program().to_string_lossy();
    assert!(!prog.contains(secret), "secret in program: {prog}");
    for arg in cmd.get_args() {
        let a = arg.to_string_lossy();
        assert!(!a.contains(secret), "secret in arg: {a}");
    }
    for (k, v) in cmd.get_envs() {
        let k = k.to_string_lossy();
        assert!(!k.contains(secret), "secret in env key: {k}");
        if k == "MYSQL_PWD" {
            continue;
        }
        if let Some(v) = v {
            let v = v.to_string_lossy();
            assert!(!v.contains(secret), "secret in env value {k}={v}");
        }
    }
}


#[test]
fn render_my_cnf_contains_required_security_settings() {
    let cnf = render_my_cnf(&MyCnfParams {
        port: 42345,
        data_dir: Path::new("/i/data"),
        run_dir: Path::new("/i/run"),
        logs_dir: Path::new("/i/logs"),
        runtime_dir: Path::new("/rt/mysql"),
        socket: Path::new("/i/run/mysqld-42345.sock"),
    });
    assert!(cnf.contains("bind-address=127.0.0.1\n"));
    assert!(cnf.contains("port=42345\n"));
    assert!(cnf.contains("datadir=/i/data\n"));
    assert!(cnf.contains("socket=/i/run/mysqld-42345.sock\n"));
    assert!(cnf.contains("pid-file=/i/run/mysqld.pid\n"));
    assert!(cnf.contains("log-error=/i/logs/error.log\n"));
    assert!(cnf.contains("mysqlx=OFF\n"));
    assert!(cnf.contains("skip-name-resolve\n"));
    assert!(cnf.contains("basedir=/rt/mysql\n"));
    assert!(!cnf.contains("0.0.0.0"));
}

#[test]
fn spawn_command_uses_defaults_file_and_no_password() {
    let ctx = run_ctx(42345, AuthMode::Password, Some(SECRET));
    let cmd = MysqlAdapter.spawn_command(&ctx);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args.len(), 1);
    assert!(args[0].starts_with("--defaults-file="));
    assert!(args[0].contains("/run/my.cnf"));
    assert!(cmd.get_program().to_string_lossy().ends_with("bin/mysqld"));
    assert_no_secret_in_command(&cmd, SECRET);
}

#[test]
fn connection_uri_password_and_no_password_shapes() {
    let ctx_pw = run_ctx(3306, AuthMode::Password, Some("p@:/?#[]% x"));
    let cred = Credential::new("root", SecretString::from("p@:/?#[]% x"));
    let uri = MysqlAdapter.connection_uri(&ctx_pw, &cred, UriFormat::Uri);
    assert!(uri.expose().starts_with("mysql://root:"));
    assert!(uri.expose().ends_with("@127.0.0.1:3306/"));
    assert!(uri.expose().contains("%40"));
    assert!(uri.expose().contains("%20"));
    let env = MysqlAdapter.connection_uri(&ctx_pw, &cred, UriFormat::DotEnv);
    assert!(env.expose().contains("DB_PASSWORD=p@:/?#[]% x\n"));
    let ctx_none = run_ctx(3306, AuthMode::None, None);
    let no_pw = MysqlAdapter.connection_uri_no_password(&ctx_none, UriFormat::Uri);
    assert_eq!(no_pw.expose(), "mysql://root@127.0.0.1:3306/");
    let no_env = MysqlAdapter.connection_uri_no_password(&ctx_none, UriFormat::DotEnv);
    assert!(!no_env.expose().contains("DB_PASSWORD"));
}

#[test]
fn init_sql_password_mode_sets_both_hosts_and_flushes() {
    let ctx = init_ctx(42345, AuthMode::Password, Some(SECRET));
    let s = render_init_sql(&ctx).expect("sql").expose().to_owned();
    assert!(s.contains("ALTER USER 'root'@'localhost' IDENTIFIED BY 'S3cret-Canary-Pw-MySql!9x';"));
    assert!(s.contains(
        "CREATE USER IF NOT EXISTS 'root'@'127.0.0.1' IDENTIFIED BY 'S3cret-Canary-Pw-MySql!9x';"
    ));
    assert!(s.contains("GRANT ALL PRIVILEGES ON *.* TO 'root'@'127.0.0.1' WITH GRANT OPTION;"));
    assert!(s.contains("FLUSH PRIVILEGES;"));
}

#[test]
fn init_sql_no_password_mode_adds_loopback_user() {
    let ctx = init_ctx(42345, AuthMode::None, None);
    let s = render_init_sql(&ctx).expect("sql").expose().to_owned();
    assert!(s.contains("CREATE USER IF NOT EXISTS 'root'@'127.0.0.1' IDENTIFIED BY '';"));
    assert!(!s.contains("ALTER USER 'root'@'localhost' IDENTIFIED BY"));
    assert!(s.contains("FLUSH PRIVILEGES;"));
}

#[test]
fn init_sql_escapes_single_quote_and_backslash() {
    let ctx = init_ctx(42345, AuthMode::Password, Some("p'w\\rd"));
    let s = render_init_sql(&ctx).expect("sql").expose().to_owned();
    assert!(s.contains("IDENTIFIED BY 'p''w\\\\rd';"));
}

#[test]
fn init_sql_debug_redacted() {
    let ctx = init_ctx(42345, AuthMode::Password, Some(SECRET));
    let sql = render_init_sql(&ctx).expect("sql");
    assert!(!format!("{sql:?}").contains(SECRET));
    assert!(!format!("{sql}").contains(SECRET));
}

#[test]
fn validate_socket_path_len_rejects_long_path() {
    let long = PathBuf::from(format!("/tmp/{}/mysqld-3306.sock", "x".repeat(120)));
    let err = validate_socket_path_len(&long).unwrap_err();
    assert!(matches!(
        err,
        DblError::InvalidInput {
            field: "socket",
            ..
        }
    ));
}

#[test]
fn parse_handshake_protocol_10_ready() {
    let mut payload = vec![10u8];
    payload.extend_from_slice(b"8.4.11\0");
    payload.extend_from_slice(&[0u8; 60]);
    assert_eq!(parse_handshake_payload(&payload).unwrap(), Handshake::Ready);
}

#[test]
fn parse_handshake_real_greeting_packet_fixture() {
    // Captured from mysqld 8.4.11 minimal, 127.0.0.1:41108, 2026-11-22.
    // Full packet: 4-byte header + 74-byte payload (protocol 10, 8.4.11, caching_sha2_password).
    let pkt = hex_to_bytes(
        "4a0000000a382e342e3131000a0000002b452a0f663f0b7e00ffffff0200ffdf15000000000000000000002f793f2347727f7a046f1b720063616368696e675f736861325f70617373776f726400",
    );
    let payload = &pkt[4..];
    assert_eq!(payload[0], 10);
    assert_eq!(parse_handshake_payload(payload).unwrap(), Handshake::Ready);
}

#[test]
fn parse_handshake_real_err_1130_host_not_allowed_is_ready() {
    // Captured from mysqld 8.4.11 minimal before root@127.0.0.1 existed.
    let pkt = hex_to_bytes(
        "42000000ff6a04486f737420273132372e302e302e3127206973206e6f7420616c6c6f77656420746f20636f6e6e65637420746f2074686973204d7953514c20736572766572",
    );
    let payload = &pkt[4..];
    assert_eq!(payload[0], 0xFF);
    assert_eq!(parse_handshake_payload(payload).unwrap(), Handshake::Ready);
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

#[test]
fn parse_handshake_auth_challenges_are_ready() {
    let mut p1045 = vec![0xFF, 0x15, 0x04, b'#'];
    p1045.extend_from_slice(b"28000Access denied for user 'root'@'127.0.0.1'");
    assert_eq!(parse_handshake_payload(&p1045).unwrap(), Handshake::Ready);
    let mut p1130 = vec![0xFF, 0x6A, 0x04, b'#'];
    p1130
        .extend_from_slice(b"HY000Host '127.0.0.1' is not allowed to connect to this MySQL server");
    assert_eq!(parse_handshake_payload(&p1130).unwrap(), Handshake::Ready);
}

#[test]
fn parse_handshake_other_err_empty_unknown_no_panic() {
    let mut other = vec![0xFF, 0x10, 0x04, b'#'];
    other.extend_from_slice(b"HY000Too many connections");
    assert!(matches!(
        parse_handshake_payload(&other).unwrap(),
        Handshake::NotReady(_)
    ));
    assert!(matches!(
        parse_handshake_payload(&[]).unwrap(),
        Handshake::NotReady(_)
    ));
    assert!(matches!(
        parse_handshake_payload(&[0xFE, 0, 0]).unwrap(),
        Handshake::NotReady(_)
    ));
    assert!(matches!(
        parse_handshake_payload(&[0xFF]).unwrap(),
        Handshake::NotReady(_) | Handshake::Ready
    ));
    let big = vec![10u8; 1024 * 1024];
    assert_eq!(parse_handshake_payload(&big).unwrap(), Handshake::Ready);
}

fn diag_cases() -> Vec<(&'static str, StartFailure)> {
    vec![
        (
            "2026-11-21T23:56:30.686109Z 0 [ERROR] [MY-010262] [Server] Can't start server: Bind on TCP/IP port: Address already in use\n2026-11-21T23:56:30.686124Z 0 [ERROR] [MY-010257] [Server] Do you already have another mysqld server running on port: 41102 ?\n",
            StartFailure::PortInUse,
        ),
        (
            "Can't create test file /tmp/x/ro/mysqld_tmp_file_case_insensitive_test.lower-test\nmysqld: File './binlog.index' not found (OS errno 13 - Permission denied)\n",
            StartFailure::PermissionDenied,
        ),
        // P4: four distinct cases that keep Unknown but deserve pinned fixtures.
        (
            "2026-11-21T23:56:34.517296Z 0 [ERROR] [MY-010267] [Server] The socket file path is too long (> 107): /tmp/very/long/path/mysqld.sock\n",
            StartFailure::Unknown,
        ),
        (
            "2026-11-21T23:56:33.765723Z 0 [ERROR] [MY-000058] [Server] Unknown suffix 'n' used for variable 'port' (value 'not-a-number').\n2026-11-21T23:56:33.765729Z 0 [ERROR] [MY-000077] [Server] /tmp/.../bin/mysqld: Error while setting value 'not-a-number' to 'port'.\n2026-11-21T23:56:33.765762Z 0 [ERROR] [MY-010119] [Server] Aborting\n",
            StartFailure::Unknown,
        ),
        (
            "2026-11-21T23:56:32.516055Z 1 [ERROR] [MY-012646] [InnoDB] File ./ibdata1: 'open' returned OS error 71. Cannot continue operation\n2026-11-21T23:56:32.516068Z 1 [ERROR] [MY-012981] [InnoDB] Cannot continue operation.\n",
            StartFailure::Unknown,
        ),
        (
            "2026-11-21T23:56:32.767385Z 1 [ERROR] [MY-012263] [InnoDB] The Auto-extending innodb_system data file './ibdata1' is of a different size 0 pages\n2026-11-21T23:56:33.268000Z 0 [ERROR] [MY-010020] [Server] Data Dictionary initialization failed.\n",
            StartFailure::Unknown,
        ),
        (
            "mysqld: error while loading shared libraries: libfoo.so.1: cannot open shared object file: No such file or directory\n",
            StartFailure::BinaryMissing,
        ),
        (
            "bash: /opt/mysql/bin/mysqld: No such file or directory\n",
            StartFailure::BinaryMissing,
        ),
    ]
}

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
fn diagnose_real_spike_texts() {
    for (text, expected) in diag_cases() {
        assert_eq!(diagnose_failure_text(text), expected, "text: {text:?}");
    }
}

#[test]
fn diagnose_unknown_for_unrelated_text() {
    assert_eq!(
        diagnose_failure_text("2026-11-21T00:00:00Z 0 [Note] [MY-000000] [Server] some note\n"),
        StartFailure::Unknown
    );
}

#[test]
fn init_sql_requires_password_when_auth_password() {
    let ctx = init_ctx(3306, AuthMode::Password, None);
    let err = render_init_sql(&ctx).unwrap_err();
    assert!(matches!(
        err,
        DblError::InvalidInput {
            field: "password",
            ..
        }
    ));
}

#[test]
fn socket_path_limit_is_platform_specific() {
    let limit = socket_path_limit();
    #[cfg(target_os = "linux")]
    assert_eq!(limit, 107);
    #[cfg(not(target_os = "linux"))]
    assert_eq!(limit, 103);
    assert!(limit <= 107);
}

#[test]
fn resolve_socket_path_uses_run_dir_when_short() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let instance_dir = dir.path().join("inst");
    let sock = resolve_socket_path(&run_dir, 42000, &instance_dir).expect("sock");
    assert_eq!(sock, run_dir.join("mysqld-42000.sock"));
}

#[test]
fn resolve_socket_path_falls_back_for_long_run_dir() {
    let dir = tempfile::tempdir().expect("tmp");
    let fallback_root = dir.path().join("fb");
    crate::fsutil::ensure_private_dir(&fallback_root).expect("fb");
    let long_run = dir.path().join("r".repeat(140)).join("run");
    fs::create_dir_all(&long_run).expect("long run");
    let instance_dir = dir.path().join("instance-very-long-name-for-hash");
    let sock =
        resolve_socket_path_with_fallback_dir(&long_run, 42000, &instance_dir, &fallback_root)
            .expect("fallback sock");
    assert_ne!(sock, socket_path(&long_run, 42000));
    assert!(sock.starts_with(&fallback_root));
    assert!(sock.as_os_str().as_encoded_bytes().len() <= socket_path_limit());
    let name = sock.file_name().unwrap().to_string_lossy();
    assert!(name.starts_with("m42000-"));
    assert!(name.ends_with(".sock"));
}

#[test]
fn resolve_socket_path_fallback_is_deterministic_for_same_instance() {
    let dir = tempfile::tempdir().expect("tmp");
    let fallback_root = dir.path().join("fb");
    crate::fsutil::ensure_private_dir(&fallback_root).expect("fb");
    let long_run = dir.path().join("x".repeat(140));
    let instance_dir = dir.path().join("instance-a");
    let a = resolve_socket_path_with_fallback_dir(&long_run, 42001, &instance_dir, &fallback_root)
        .expect("a");
    let b = resolve_socket_path_with_fallback_dir(&long_run, 42001, &instance_dir, &fallback_root)
        .expect("b");
    assert_eq!(a, b);
    let other = resolve_socket_path_with_fallback_dir(
        &long_run,
        42001,
        &dir.path().join("instance-b"),
        &fallback_root,
    )
    .expect("other");
    assert_ne!(a, other);
}

#[test]
fn socket_path_from_my_cnf_reads_socket_value() {
    let dir = tempfile::tempdir().expect("tmp");
    let cnf = dir.path().join("my.cnf");
    fs::write(
        &cnf,
        "[mysqld]\nsocket=/tmp/dblocal-1000/m42000-abcd.sock\nport=42000\n",
    )
    .expect("write");
    let sock = socket_path_from_my_cnf(&cnf).expect("sock");
    assert_eq!(sock, PathBuf::from("/tmp/dblocal-1000/m42000-abcd.sock"));
}

#[test]
fn cleanup_fallback_socket_removes_only_fallback() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let preferred = socket_path(&run_dir, 42000);
    fs::write(&preferred, b"").expect("preferred");
    cleanup_fallback_socket(&preferred, &run_dir, 42000);
    assert!(preferred.exists(), "preferred socket must not be removed");

    let fallback = dir.path().join("fb").join("m42000-abc.sock");
    fs::create_dir_all(fallback.parent().unwrap()).expect("fb");
    fs::write(&fallback, b"").expect("fallback");
    cleanup_fallback_socket(&fallback, &run_dir, 42000);
    assert!(!fallback.exists(), "fallback socket should be removed");
}

#[test]
fn effective_superuser_rejects_unsafe_name_in_debug() {
    // debug_assert only fires in debug builds; test the validator directly.
    assert!(is_safe_sql_identifier("root"));
    assert!(is_safe_sql_identifier("mysql_admin"));
    assert!(!is_safe_sql_identifier("root';DROP TABLE x;--"));
    assert!(!is_safe_sql_identifier(""));
    assert!(!is_safe_sql_identifier("1root"));
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
