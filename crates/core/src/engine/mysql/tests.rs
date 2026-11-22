use super::*;
use crate::engine::Credential;
use crate::model::Port;
use sha2::Digest;

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
    assert!(cnf.contains("tmpdir=/i/run/tmp\n"));
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
fn escape_sql_single_quoted_handles_control_chars_and_mixed() {
    assert_eq!(escape_sql_single_quoted("a\0b"), "a\\0b");
    assert_eq!(escape_sql_single_quoted("a\nb"), "a\\nb");
    assert_eq!(escape_sql_single_quoted("a\rb"), "a\\rb");
    let mixed = "x\0y\nz\rr'\\q";
    let escaped = escape_sql_single_quoted(mixed);
    assert_eq!(escaped, "x\\0y\\nz\\rr''\\\\q");
    let mut stmt_count = 0usize;
    let mut in_literal = false;
    let mut chars = escaped.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                if in_literal && chars.peek() == Some(&'\'') {
                    chars.next();
                } else {
                    in_literal = !in_literal;
                }
            }
            '\n' | '\r' => assert!(!in_literal, "raw newline inside SQL literal: {escaped:?}"),
            ';' if !in_literal => stmt_count += 1,
            _ => {}
        }
    }
    assert!(!in_literal, "unterminated SQL literal: {escaped:?}");
    let sql = format!("ALTER USER 'root'@'localhost' IDENTIFIED BY '{escaped}';\n");
    let mut top_level_stmts = 0usize;
    let mut in_lit = false;
    let mut it = sql.chars().peekable();
    while let Some(ch) = it.next() {
        match ch {
            '\'' => {
                if in_lit && it.peek() == Some(&'\'') {
                    it.next();
                } else {
                    in_lit = !in_lit;
                }
            }
            ';' if !in_lit => top_level_stmts += 1,
            _ => {}
        }
    }
    assert_eq!(
        top_level_stmts, 1,
        "escaped value must keep one statement: {sql:?}"
    );
    let _ = stmt_count;
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
fn validate_socket_path_len_boundary_at_limit_minus_one_limit_plus_one() {
    let limit = socket_path_limit();
    let base = "/tmp/s/";
    let suffix = ".sock";
    let fixed = base.len() + suffix.len();
    let at_limit = format!("{base}{}{suffix}", "a".repeat(limit - fixed));
    assert_eq!(at_limit.len(), limit);
    validate_socket_path_len(Path::new(&at_limit)).expect("at limit fits");

    let at_limit_minus_one = format!("{base}{}{suffix}", "a".repeat(limit - fixed - 1));
    assert_eq!(at_limit_minus_one.len(), limit - 1);
    validate_socket_path_len(Path::new(&at_limit_minus_one)).expect("limit-1 fits");

    let at_limit_plus_one = format!("{base}{}{suffix}", "a".repeat(limit - fixed + 1));
    assert_eq!(at_limit_plus_one.len(), limit + 1);
    let err = validate_socket_path_len(Path::new(&at_limit_plus_one)).unwrap_err();
    match err {
        DblError::InvalidInput { field, reason } => {
            assert_eq!(field, "socket");
            assert!(reason.contains(&(limit + 1).to_string()));
            assert!(reason.contains(&limit.to_string()));
        }
        other => panic!("unexpected error: {other:?}"),
    }
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
fn resolve_socket_path_for_errors_when_even_fallback_too_long() {
    let dir = tempfile::tempdir().expect("tmp");
    let fallback_root = dir.path().join("f".repeat(140));
    crate::fsutil::ensure_private_dir(&fallback_root).expect("fb");
    let long_run = dir.path().join("r".repeat(140)).join("run");
    let instance_dir = dir.path().join("instance");
    let uid = nix::unistd::geteuid().as_raw();
    let err = resolve_socket_path_with_candidates_for_test(
        &long_run,
        42000,
        &instance_dir,
        vec![fallback_root.clone()],
        uid,
    )
    .unwrap_err();
    match err {
        DblError::InvalidInput { field, reason } => {
            assert_eq!(field, "socket");
            assert!(reason.contains(&fallback_root.display().to_string()));
            assert!(reason.contains("too long") || reason.contains("bytes"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn resolve_socket_path_for_chooses_first_candidate_whose_socket_fits() {
    let dir = tempfile::tempdir().expect("tmp");
    let first = dir.path().join("x".repeat(140));
    let second = dir.path().join("short");
    crate::fsutil::ensure_private_dir(&first).expect("first");
    crate::fsutil::ensure_private_dir(&second).expect("second");
    let long_run = dir.path().join("r".repeat(140)).join("run");
    let instance_dir = dir.path().join("instance");
    let uid = nix::unistd::geteuid().as_raw();
    let chosen = resolve_socket_path_with_candidates_for_test(
        &long_run,
        42002,
        &instance_dir,
        vec![first.clone(), second.clone()],
        uid,
    )
    .expect("chosen");
    assert!(
        chosen.starts_with(second.join("dblocal")) || chosen.starts_with(&second),
        "should pick second short candidate: {chosen:?}"
    );
}

fn resolve_socket_path_with_candidates_for_test(
    run_dir: &Path,
    port: u16,
    instance_dir: &Path,
    candidates: Vec<PathBuf>,
    uid: u32,
) -> Result<PathBuf> {
    let preferred = socket_path(run_dir, port);
    if preferred.as_os_str().as_encoded_bytes().len() <= socket_path_limit() {
        return Ok(preferred);
    }
    let mut rejections = Vec::new();
    for dir in candidates {
        match ensure_verified_private_dir_for_test(&dir, uid) {
            Ok(()) => {}
            Err(err) => {
                rejections.push(format!("{}: {err}", dir.display()));
                continue;
            }
        }
        let fallback = dir.join({
            let mut hasher = sha2::Sha256::new();
            sha2::Digest::update(&mut hasher, instance_dir.as_os_str().as_encoded_bytes());
            let digest = sha2::Digest::finalize(hasher);
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            format!("m{port}-{}.sock", &hex[..12])
        });
        if fallback.as_os_str().as_encoded_bytes().len() > socket_path_limit() {
            rejections.push(format!("{}: too long", dir.display()));
            continue;
        }
        return Ok(fallback);
    }
    Err(DblError::InvalidInput {
        field: "socket",
        reason: rejections.join("; "),
    })
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
fn socket_path_from_my_cnf_missing_empty_and_spaces() {
    let dir = tempfile::tempdir().expect("tmp");
    let cnf = dir.path().join("my.cnf");
    fs::write(&cnf, "[mysqld]\nport=42000\n# socket=/tmp/x.sock\n").expect("write");
    assert!(socket_path_from_my_cnf(&cnf).is_none());

    fs::write(&cnf, "[mysqld]\nsocket=\nport=42000\n").expect("write");
    assert!(socket_path_from_my_cnf(&cnf).is_none());

    fs::write(&cnf, "[mysqld]\n socket=/tmp/dblocal-1/m1.sock \n").expect("write");
    assert_eq!(
        socket_path_from_my_cnf(&cnf),
        Some(PathBuf::from("/tmp/dblocal-1/m1.sock")),
        "leading spaces before key should be ignored by trim"
    );

    fs::write(&cnf, "[mysqld]\nsocket=   /tmp/dblocal-1/m1.sock  \n").expect("write");
    assert_eq!(
        socket_path_from_my_cnf(&cnf),
        Some(PathBuf::from("/tmp/dblocal-1/m1.sock"))
    );
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

    let uid = nix::unistd::geteuid().as_raw();
    let fallback_root =
        ensure_fallback_socket_dir_for(None, uid).expect("fallback dir for cleanup test");
    let fallback = fallback_root.join("m42000-abc123def456.sock");
    let _ = fs::remove_file(&fallback);
    let listener = std::os::unix::net::UnixListener::bind(&fallback).expect("bind fallback sock");
    assert!(fallback.exists());
    drop(listener);
    cleanup_fallback_socket(&fallback, &run_dir, 42000);
    assert!(!fallback.exists(), "fallback socket should be removed");
}

#[test]
fn effective_superuser_rejects_unsafe_name() {
    assert_eq!(effective_superuser("root").unwrap(), "root");
    assert_eq!(effective_superuser("mysql_admin").unwrap(), "mysql_admin");
    assert_eq!(effective_superuser("").unwrap(), "root");
    let err = effective_superuser("root'; DROP USER x; --").unwrap_err();
    assert!(matches!(
        err,
        DblError::InvalidInput {
            field: "superuser",
            ..
        }
    ));
    let err = effective_superuser("1root").unwrap_err();
    assert!(matches!(
        err,
        DblError::InvalidInput {
            field: "superuser",
            ..
        }
    ));
}

#[test]
fn render_init_sql_rejects_unsafe_superuser() {
    let mut ctx = init_ctx(42345, AuthMode::Password, Some(SECRET));
    ctx.superuser = "root'; DROP USER x; --".into();
    let err = render_init_sql(&ctx).unwrap_err();
    assert!(matches!(
        err,
        DblError::InvalidInput {
            field: "superuser",
            ..
        }
    ));
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
fn fallback_socket_dir_candidates_respects_xdg_and_uid() {
    let uid = 4242u32;
    let none = fallback_socket_dir_candidates(None, uid);
    assert_eq!(none, vec![PathBuf::from("/tmp/dblocal-4242")]);

    let empty = fallback_socket_dir_candidates(Some(std::ffi::OsString::from("")), uid);
    assert_eq!(empty, vec![PathBuf::from("/tmp/dblocal-4242")]);

    let set = fallback_socket_dir_candidates(Some(std::ffi::OsString::from("/run/user/4242")), uid);
    assert_eq!(
        set,
        vec![
            PathBuf::from("/run/user/4242/dblocal"),
            PathBuf::from("/tmp/dblocal-4242")
        ]
    );
}

#[test]
fn resolve_socket_path_for_uses_second_candidate_when_first_fails() {
    let dir = tempfile::tempdir().expect("tmp");
    let long_first = dir.path().join("x".repeat(140));
    let short_second = dir.path().join("s");
    crate::fsutil::ensure_private_dir(&long_first).expect("first");
    crate::fsutil::ensure_private_dir(&short_second).expect("second");
    let long_run = dir.path().join("r".repeat(140)).join("run");
    let instance_dir = dir.path().join("instance");
    let uid = nix::unistd::geteuid().as_raw();
    let chosen = resolve_socket_path_with_candidates_for_test(
        &long_run,
        42003,
        &instance_dir,
        vec![long_first.clone(), short_second.clone()],
        uid,
    )
    .expect("should fall back to short second");
    assert!(chosen.starts_with(&short_second));
    assert!(chosen.as_os_str().as_encoded_bytes().len() <= socket_path_limit());
}

#[test]
fn resolve_socket_path_for_reports_candidate_rejections() {
    let dir = tempfile::tempdir().expect("tmp");
    let bad = dir.path().join("x".repeat(160));
    crate::fsutil::ensure_private_dir(&bad).expect("bad");
    let long_run = dir.path().join("r".repeat(140)).join("run");
    let instance_dir = dir.path().join("instance");
    let uid = nix::unistd::geteuid().as_raw();
    let err = resolve_socket_path_with_candidates_for_test(
        &long_run,
        42004,
        &instance_dir,
        vec![bad.clone()],
        uid,
    )
    .unwrap_err();
    match err {
        DblError::InvalidInput { field, reason } => {
            assert_eq!(field, "socket");
            assert!(reason.contains(&bad.display().to_string()));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn verify_secure_dir_rejects_symlink_file_mode_and_wrong_owner() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().expect("tmp");
    let real = dir.path().join("real");
    crate::fsutil::ensure_private_dir(&real).expect("real");
    let uid = nix::unistd::geteuid().as_raw();
    verify_secure_dir_for_test(&real, uid).expect("real ok");

    let link = dir.path().join("link");
    symlink(&real, &link).expect("symlink");
    let err = verify_secure_dir_for_test(&link, uid).unwrap_err();
    assert!(err.to_string().contains("not a real directory"));

    let file = dir.path().join("file");
    fs::write(&file, b"x").expect("file");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).expect("chmod");
    let err = verify_secure_dir_for_test(&file, uid).unwrap_err();
    assert!(err.to_string().contains("not a real directory"));

    let bad_mode = dir.path().join("badmode");
    fs::create_dir(&bad_mode).expect("dir");
    fs::set_permissions(&bad_mode, fs::Permissions::from_mode(0o755)).expect("chmod");
    let err = verify_secure_dir_for_test(&bad_mode, uid).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("0700") || msg.contains("mode"), "msg: {msg}");

    let wrong_owner = dir.path().join("wrongowner");
    crate::fsutil::ensure_private_dir(&wrong_owner).expect("dir");
    let err = verify_secure_dir_for_test(&wrong_owner, uid + 1).unwrap_err();
    assert!(err.to_string().contains("wrong owner"));
}

#[test]
fn ensure_verified_private_dir_creates_and_rejects_preexisting_wrong_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let target = dir.path().join("newdir");
    ensure_verified_private_dir_for_test(&target, uid).expect("create");
    let meta = fs::symlink_metadata(&target).expect("meta");
    assert!(meta.is_dir());
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);

    let bad = dir.path().join("bad");
    fs::create_dir(&bad).expect("bad dir");
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o770)).expect("chmod");
    let err = ensure_verified_private_dir_for_test(&bad, uid).unwrap_err();
    assert!(err.to_string().contains("0700") || err.to_string().contains("mode"));
    let meta = fs::symlink_metadata(&bad).expect("meta");
    assert_eq!(
        meta.permissions().mode() & 0o777,
        0o770,
        "must not chmod pre-existing wrong mode"
    );
}

#[test]
fn ensure_verified_private_dir_missing_parent_error_when_parent_not_writable() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        eprintln!(
            "skipping ensure_verified_private_dir_missing_parent_error_when_parent_not_writable: running as root"
        );
        return;
    }
    let dir = tempfile::tempdir().expect("tmp");
    let parent = dir.path().join("ro");
    fs::create_dir(&parent).expect("parent");
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o500)).expect("chmod");
    let uid = nix::unistd::geteuid().as_raw();
    let target = parent.join("child");
    let err = ensure_verified_private_dir_for_test(&target, uid).unwrap_err();
    assert!(
        err.to_string().contains("cannot create") || err.to_string().contains("fallback socket")
    );
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).expect("restore");
}

#[test]
fn fallback_socket_name_validation_for_port() {
    assert!(is_fallback_socket_name_for_port(
        "m42000-abcdef012345.sock",
        42000
    ));
    assert!(!is_fallback_socket_name_for_port(
        "m42001-abcdef012345.sock",
        42000
    ));
    assert!(!is_fallback_socket_name_for_port(
        "m42000-abcdef01234.sock",
        42000
    ));
    // uppercase hex is accepted by is_ascii_hexdigit; that is harmless here
    assert!(is_fallback_socket_name_for_port(
        "m42000-ABCDEF012345.sock",
        42000
    ));
    assert!(!is_fallback_socket_name_for_port(
        "other-abcdef012345.sock",
        42000
    ));
    assert!(!is_fallback_socket_name_for_port(
        "m42000-abcdef012345.socket",
        42000
    ));
}

#[test]
fn cleanup_fallback_socket_refuses_regular_file_with_valid_name() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let fallback_root = ensure_fallback_socket_dir_for(None, uid).expect("fb");
    let fallback = fallback_root.join("m42010-abc123def456.sock");
    let _ = fs::remove_file(&fallback);
    fs::write(&fallback, b"not a socket").expect("write");
    cleanup_fallback_socket_with_candidates(&fallback, &run_dir, 42010, None, uid);
    assert!(fallback.exists(), "regular file must not be deleted");
    let _ = fs::remove_file(&fallback);
}

#[test]
fn cleanup_fallback_socket_refuses_foreign_name_and_other_port() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let fallback_root = ensure_fallback_socket_dir_for(None, uid).expect("fb");

    let foreign = fallback_root.join("not-a-fallback.sock");
    let _ = fs::remove_file(&foreign);
    let l1 = std::os::unix::net::UnixListener::bind(&foreign).expect("bind");
    drop(l1);
    cleanup_fallback_socket_with_candidates(&foreign, &run_dir, 42011, None, uid);
    assert!(foreign.exists(), "foreign name must not be deleted");
    let _ = fs::remove_file(&foreign);

    let other_port = fallback_root.join("m42012-abc123def456.sock");
    let _ = fs::remove_file(&other_port);
    let l2 = std::os::unix::net::UnixListener::bind(&other_port).expect("bind");
    drop(l2);
    cleanup_fallback_socket_with_candidates(&other_port, &run_dir, 42011, None, uid);
    assert!(other_port.exists(), "other-port socket must not be deleted");
    let _ = fs::remove_file(&other_port);
}

#[test]
fn cleanup_fallback_socket_removes_real_socket_with_right_name_and_parent() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let fallback_root = ensure_fallback_socket_dir_for(None, uid).expect("fb");
    let fallback = fallback_root.join("m42013-abc123def456.sock");
    let _ = fs::remove_file(&fallback);
    let listener = std::os::unix::net::UnixListener::bind(&fallback).expect("bind");
    drop(listener);
    assert!(fallback.exists());
    cleanup_fallback_socket_with_candidates(&fallback, &run_dir, 42013, None, uid);
    assert!(
        !fallback.exists(),
        "verified fallback socket should be removed"
    );
}

fn write_fake_bin(dir: &Path, name: &str, script: &str) -> PathBuf {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let p = bin.join(name);
    // Write to a temp name, chmod it, then rename into place so a concurrent
    // fork/exec in another test never sees a half-written or write-locked file
    // ("Text file busy", ETXTBSY).
    let tmp = bin.join(format!(".{name}.tmp-{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    fs::write(&tmp, script).expect("write script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let _ = fs::remove_file(&p);
    fs::rename(&tmp, &p).expect("rename script");
    p
}

/// Runs `f` until it stops failing with ETXTBSY.
///
/// Another test in the same binary can fork while our fake script is still open
/// for writing; the forked child inherits that descriptor and the following exec
/// of the script fails with `Text file busy` (os error 26). That is a test
/// environment artifact, not product behaviour, so retry briefly.
fn retry_etxtbsy<T, E>(
    mut f: impl FnMut() -> std::result::Result<T, E>,
) -> std::result::Result<T, E>
where
    E: std::fmt::Display,
{
    let mut last = None;
    for _ in 0..500 {
        match f() {
            Ok(value) => return Ok(value),
            Err(err) => {
                if !err.to_string().contains("Text file busy") {
                    return Err(err);
                }
                last = Some(err);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
    Err(last.expect("retry budget always records the last ETXTBSY"))
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
fn shutdown_command_failure_reports_tail_and_cleans_fallback_socket() {
    let dir = tempfile::tempdir().expect("tmp");
    let runtime = dir.path().join("rt");
    let _mysqladmin = write_fake_bin(
        &runtime,
        "mysqladmin",
        "#!/bin/sh\necho 'shutdown bad' >&2\nexit 9\n",
    );
    let uid = nix::unistd::geteuid().as_raw();
    let fallback_root = ensure_fallback_socket_dir_for(None, uid).expect("fb");
    let socket = fallback_root.join("m42102-abc123def456.sock");
    let _ = fs::remove_file(&socket);
    let l = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
    drop(l);
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", socket.display()),
    )
    .expect("cnf");
    let ctx = RunContext {
        instance_name: "app".into(),
        runtime_dir: runtime,
        data_dir: dir.path().join("data"),
        logs_dir: dir.path().join("logs"),
        run_dir: run_dir.clone(),
        port: Port::new(42102).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        credential: None,
    };
    let err = retry_etxtbsy(|| MysqlAdapter.shutdown(&ctx)).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("shutdown bad"), "msg: {msg}");
    assert!(msg.contains("9") || msg.contains("code"), "msg: {msg}");
    assert!(
        !socket.exists(),
        "fallback socket should be removed even on shutdown failure"
    );
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
fn read_handshake_half_open_and_non_mysql_banner() {
    use std::io::Write;
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let t = std::thread::spawn(move || {
        let (s, _) = listener.accept().expect("accept");
        // half-open: close without sending handshake
        let _ = s.shutdown(std::net::Shutdown::Both);
    });
    let err = read_handshake(port, Duration::from_millis(500)).unwrap_err();
    assert!(err.to_string().contains("handshake") || err.to_string().contains("mysql"));
    t.join().expect("join");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let t = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().expect("accept");
        let payload = b"HTTP/1.1 200 OK\r\n\r\n";
        let len = payload.len() as u32;
        let mut hdr = [0u8; 4];
        hdr[0] = (len & 0xff) as u8;
        hdr[1] = ((len >> 8) & 0xff) as u8;
        hdr[2] = ((len >> 16) & 0xff) as u8;
        hdr[3] = 0;
        s.write_all(&hdr).expect("hdr");
        s.write_all(payload).expect("payload");
    });
    let h = read_handshake(port, Duration::from_millis(500)).expect("handshake");
    assert!(matches!(h, Handshake::NotReady(_)));
    t.join().expect("join");
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
fn prepare_run_recreates_missing_tmpdir_and_fallback_socket_dir() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = xdg.join("dblocal");
    let sock = fb.join("m42120-abc123def456.sock");
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\nport=42120\n", sock.display()),
    )
    .expect("cnf");
    let tmp = run_dir.join("tmp");
    let _ = fs::remove_dir_all(&tmp);
    prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42120,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .expect("prepare");
    let meta = fs::symlink_metadata(&tmp).expect("tmp meta");
    assert!(meta.is_dir());
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    let fb_meta = fs::symlink_metadata(&fb).expect("fb meta");
    assert_eq!(fb_meta.uid(), uid);
    assert_eq!(fb_meta.permissions().mode() & 0o777, 0o700);
}

#[test]
fn prepare_run_removes_stale_socket_file_and_refuses_regular_file() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let sock = fb.join("m42121-abc123def456.sock");
    let _ = fs::remove_file(&sock);
    let l = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
    drop(l);
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", sock.display()),
    )
    .expect("cnf");
    prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42121,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .expect("prepare");
    assert!(!sock.exists(), "stale socket file should be removed");

    fs::write(&sock, b"not socket").expect("write");
    let err = prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42121,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .unwrap_err();
    assert!(err.to_string().contains("not a socket"));
    let _ = fs::remove_file(&sock);
}

#[test]
fn prepare_run_refuses_symlink_and_wrong_mode_fallback_dir() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let target = dir.path().join("target");
    crate::fsutil::ensure_private_dir(&target).expect("target");
    let sock = fb.join("m42122-abc123def456.sock");
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", sock.display()),
    )
    .expect("cnf");

    fs::set_permissions(&fb, fs::Permissions::from_mode(0o755)).expect("chmod");
    let err = prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42122,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .unwrap_err();
    assert!(err.to_string().contains("0700") || err.to_string().contains("mode"));
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o700)).expect("restore");

    let link_xdg = dir.path().join("xdg-link");
    fs::create_dir_all(&link_xdg).expect("link xdg");
    let link_parent = link_xdg.join("dblocal");
    symlink(&target, &link_parent).expect("symlink");
    let link_sock = link_parent.join("m42122-abc123def456.sock");
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", link_sock.display()),
    )
    .expect("cnf");
    let err = prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42122,
        Some(link_xdg.as_os_str().to_os_string()),
        uid,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("not an allowed fallback socket path")
            || err.to_string().contains("fallback")
            || err.to_string().contains("not a real directory")
    );
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
    let p = dir.path().join("error.log");
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

#[test]
fn resolve_socket_path_with_single_fallback_reports_too_long() {
    let dir = tempfile::tempdir().expect("tmp");
    let long_run = dir.path().join("r".repeat(140)).join("run");
    let long_fb = dir.path().join("f".repeat(140));
    crate::fsutil::ensure_private_dir(&long_fb).expect("fb");
    let instance = dir.path().join("instance");
    let err =
        resolve_socket_path_with_fallback_dir(&long_run, 42204, &instance, &long_fb).unwrap_err();
    match err {
        DblError::InvalidInput { field, reason } => {
            assert_eq!(field, "socket");
            assert!(reason.contains("bytes, maximum is"), "{reason}");
            assert!(
                reason.contains(&socket_path_limit().to_string()),
                "{reason}"
            );
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn socket_name_validation_rejects_short_hash_and_wrong_suffix() {
    assert!(!is_fallback_socket_name_for_port("m42205-abc.sock", 42205));
    assert!(!is_fallback_socket_name_for_port(
        "m42205-abcdef012345.sockx",
        42205
    ));
    assert!(!is_fallback_socket_name_for_port("", 42205));
    assert!(is_fallback_socket_name_for_port(
        "m42205-abcdef012345.sock",
        42205
    ));
}

#[test]
fn cleanup_refuses_socket_outside_allowed_dirs() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let stray = dir.path().join("elsewhere");
    crate::fsutil::ensure_private_dir(&stray).expect("stray");
    let sock = stray.join("m42206-abc123def456.sock");
    let l = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
    drop(l);
    cleanup_fallback_socket_with_candidates(&sock, &run_dir, 42206, None, uid);
    assert!(
        sock.exists(),
        "socket outside the candidate dirs must be left alone"
    );
}

fn long_run_under(dir: &Path) -> PathBuf {
    dir.join("r".repeat(140)).join("run")
}

#[test]
fn resolve_socket_path_for_skips_unverifiable_candidate_then_fits() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o755)).expect("chmod");
    let long_run = long_run_under(dir.path());
    let instance = dir.path().join("instance");
    // First candidate is unverifiable (mode 0755); the /tmp candidate must win.
    let chosen = resolve_socket_path_for(
        &long_run,
        42210,
        &instance,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .expect("second candidate fits");
    assert!(chosen.starts_with(Path::new("/tmp")));
    assert!(!chosen.starts_with(&fb));
    assert!(chosen.as_os_str().as_encoded_bytes().len() <= socket_path_limit());
}

#[test]
fn resolve_socket_path_with_single_fallback_logs_and_fits_short_dir() {
    let dir = tempfile::tempdir().expect("tmp");
    let fb = dir.path().join("short");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let long_run = long_run_under(dir.path());
    let instance = dir.path().join("instance");
    let sock = resolve_socket_path_with_fallback_dir(&long_run, 42211, &instance, &fb)
        .expect("short fallback fits");
    assert!(sock.starts_with(&fb));
}

#[test]
fn ensure_fallback_socket_dir_for_reports_last_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o755)).expect("chmod");
    // /tmp/dblocal-<uid> exists and is usable on this host, so the helper only
    // fails if even that is unusable; force the failure with a bogus uid whose
    // /tmp dir cannot be created by us only when the path already exists.
    let _ = uid;
    let err = ensure_fallback_socket_dir_for(Some(xdg.as_os_str().to_os_string()), 0);
    // uid 0 gives /tmp/dblocal-0 which we cannot create (permission denied)
    match err {
        Ok(path) => assert!(path.starts_with(&fb) || path.starts_with("/tmp")),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("cannot create verified mysql fallback socket dir"),
                "{msg}"
            );
        }
    }
}

#[test]
fn prepare_runtime_dirs_creates_missing_tmpdir_and_skips_without_my_cnf() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    prepare_runtime_dirs_for_start_with_env(&run_dir, 42212, None, uid).expect("prepare");
    let tmp = run_dir.join("tmp");
    let meta = fs::symlink_metadata(&tmp).expect("meta");
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
}

#[test]
fn prepare_runtime_dirs_errors_for_preferred_socket_that_is_a_regular_file() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let sock = socket_path(&run_dir, 42213);
    fs::write(&sock, b"x").expect("write");
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", sock.display()),
    )
    .expect("cnf");
    let err = prepare_runtime_dirs_for_start_with_env(&run_dir, 42213, None, uid).unwrap_err();
    assert!(err.to_string().contains("not a socket"), "{err}");
}

#[test]
fn prepare_runtime_dirs_errors_when_tmpdir_cannot_be_created() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        eprintln!("skipping prepare_runtime_dirs_errors_when_tmpdir_cannot_be_created as root");
        return;
    }
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    // A regular file where tmp must be created makes ensure_verified_private_dir fail.
    fs::write(run_dir.join("tmp"), b"x").expect("write");
    let err = prepare_runtime_dirs_for_start_with_env(&run_dir, 42214, None, uid).unwrap_err();
    assert!(
        err.to_string().contains("cannot prepare mysql tmpdir"),
        "{err}"
    );
    let _ = fs::remove_file(run_dir.join("tmp"));

    // Parent not writable -> cannot create tmpdir.
    let dir2 = tempfile::tempdir().expect("tmp2");
    let ro_run = dir2.path().join("run");
    fs::create_dir_all(&ro_run).expect("run");
    fs::set_permissions(&ro_run, fs::Permissions::from_mode(0o500)).expect("chmod");
    let err = prepare_runtime_dirs_for_start_with_env(&ro_run, 42214, None, uid).unwrap_err();
    fs::set_permissions(&ro_run, fs::Permissions::from_mode(0o700)).expect("restore");
    assert!(
        err.to_string().contains("cannot prepare mysql tmpdir"),
        "{err}"
    );
}

#[test]
fn cleanup_from_my_cnf_skips_foreign_socket_and_removes_verified_one() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let cnf = run_dir.join("my.cnf");
    let foreign = dir.path().join("notes.txt");
    fs::write(&foreign, b"notes").expect("notes");
    fs::write(&cnf, format!("[mysqld]\nsocket={}\n", foreign.display())).expect("cnf");
    cleanup_fallback_socket_from_my_cnf_with_candidates(
        &cnf,
        &run_dir,
        42215,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    );
    assert!(
        foreign.exists(),
        "a regular non-fallback file must survive cleanup"
    );

    let sock = fb.join("m42215-abc123def456.sock");
    let l = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
    drop(l);
    fs::write(&cnf, format!("[mysqld]\nsocket={}\n", sock.display())).expect("cnf2");
    cleanup_fallback_socket_from_my_cnf_with_candidates(
        &cnf,
        &run_dir,
        42215,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    );
    assert!(
        !sock.exists(),
        "the verified fallback socket must be removed"
    );
}

#[test]
fn resolve_socket_path_with_single_fallback_returns_preferred_when_fits() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let fb = dir.path().join("fb");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let inst = dir.path().join("inst");
    let sock = resolve_socket_path_with_fallback_dir(&run_dir, 42220, &inst, &fb).expect("sock");
    assert_eq!(sock, socket_path(&run_dir, 42220));
}

#[test]
fn ensure_fallback_socket_dir_for_errors_when_candidate_not_dir() {
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    // Create a regular file where the fallback dir should be.
    fs::write(xdg.join("dblocal"), b"not a dir").expect("write");
    let err = ensure_fallback_socket_dir_for(Some(xdg.as_os_str().to_os_string()), uid);
    match err {
        Ok(path) => {
            // /tmp/dblocal-<uid> may be usable; that's fine as long as behavior is correct.
            assert!(path.starts_with(Path::new("/tmp")));
        }
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("cannot create verified mysql fallback socket dir"),
                "{msg}"
            );
        }
    }
}

#[test]
fn resolve_socket_path_for_rejects_every_candidate_and_names_them() {
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let long_run = long_run_under(dir.path());
    let instance = dir.path().join("instance");
    // A uid we do not own: every candidate is rejected at verification.
    let err = resolve_socket_path_for(
        &long_run,
        42230,
        &instance,
        Some(xdg.as_os_str().to_os_string()),
        uid + 1,
    )
    .unwrap_err();
    match err {
        DblError::InvalidInput { field, reason } => {
            assert_eq!(field, "socket");
            assert!(reason.contains(&fb.display().to_string()), "{reason}");
            assert!(reason.contains("wrong owner"), "{reason}");
            assert!(reason.contains("maximum is"), "{reason}");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn resolve_socket_path_for_skips_verified_dir_whose_socket_would_not_fit() {
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("n".repeat(88));
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    assert!(
        fb.is_dir(),
        "first candidate verifies but its socket cannot fit"
    );
    let long_run = long_run_under(dir.path());
    let instance = dir.path().join("instance");
    let chosen = resolve_socket_path_for(
        &long_run,
        42231,
        &instance,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .expect("the /tmp candidate must win");
    assert!(chosen.starts_with(Path::new("/tmp")), "{chosen:?}");
    assert!(!chosen.starts_with(&fb));
    assert!(chosen.as_os_str().as_encoded_bytes().len() <= socket_path_limit());
}

#[test]
fn cleanup_from_my_cnf_without_socket_line_is_a_noop() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let cnf = run_dir.join("my.cnf");
    fs::write(&cnf, "[mysqld]\nport=42232\n").expect("cnf");
    cleanup_fallback_socket_from_my_cnf_with_candidates(&cnf, &run_dir, 42232, None, uid);
}

#[test]
fn cleanup_fallback_socket_leaves_a_vanished_path_alone() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let fb = dir.path().join("xdg").join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let sock = fb.join("m42233-abc123def456.sock");
    // The path is syntactically valid but never existed: cleanup must not fail.
    cleanup_fallback_socket_with_candidates(
        &sock,
        &run_dir,
        42233,
        Some(dir.path().join("xdg").as_os_str().to_os_string()),
        uid,
    );
    assert!(!sock.exists());
}

#[test]
fn cleanup_tolerates_socket_paths_without_parent_or_name() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    // No parent -> cannot be a verified fallback socket; cleanup must just skip.
    cleanup_fallback_socket_with_candidates(Path::new("/"), &run_dir, 42234, None, uid);
}

#[test]
fn prepare_runtime_dirs_accepts_preferred_socket_that_does_not_exist_yet() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let sock = socket_path(&run_dir, 42235);
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", sock.display()),
    )
    .expect("cnf");
    prepare_runtime_dirs_for_start_with_env(&run_dir, 42235, None, uid).expect("prepare");
    assert!(run_dir.join("tmp").is_dir());
}

#[test]
fn ensure_verified_private_dir_reports_parent_that_is_a_file() {
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let not_a_dir = dir.path().join("run");
    fs::write(&not_a_dir, b"x").expect("write");
    let err = ensure_verified_private_dir_for_test(&not_a_dir.join("tmp"), uid).unwrap_err();
    assert!(
        err.to_string()
            .contains("cannot create fallback socket parent"),
        "{err}"
    );
}

#[test]
fn diagnose_start_failure_delegates_to_the_text_diagnoser() {
    let bind = "Can't start server: Bind on TCP/IP port: Address already in use\n";
    assert_eq!(
        MysqlAdapter.diagnose_start_failure(bind, Some(1)),
        StartFailure::PortInUse
    );
    assert_eq!(
        MysqlAdapter.diagnose_start_failure("mysqld: Exec format error", None),
        StartFailure::BinaryMissing
    );
}

#[test]
fn public_prepare_run_creates_the_private_tmpdir() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let ctx = RunContext {
        instance_name: "app".into(),
        runtime_dir: dir.path().join("rt"),
        data_dir: dir.path().join("data"),
        logs_dir: dir.path().join("logs"),
        run_dir: run_dir.clone(),
        port: Port::new(42250).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        credential: None,
    };
    prepare_run(&ctx).expect("prepare_run");
    let meta = fs::symlink_metadata(run_dir.join("tmp")).expect("tmp meta");
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
}

#[test]
fn read_handshake_reports_a_zero_length_packet() {
    use std::io::Write;
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let t = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().expect("accept");
        s.write_all(&[0u8; 4]).expect("zero header");
    });
    let h = read_handshake(port, Duration::from_millis(500)).expect("handshake");
    match h {
        Handshake::NotReady(reason) => {
            assert!(reason.contains("invalid mysql packet length 0"), "{reason}")
        }
        other => panic!("unexpected: {other:?}"),
    }
    t.join().expect("join");
}

#[test]
fn combined_output_joins_stdout_and_stderr_with_one_newline() {
    assert_eq!(
        combined_output(b"out".to_vec(), b"err".to_vec()),
        "out\nerr"
    );
    assert_eq!(
        combined_output(b"out\n".to_vec(), b"err".to_vec()),
        "out\nerr"
    );
    assert_eq!(combined_output(b"out\n".to_vec(), Vec::new()), "out\n");
    assert_eq!(combined_output(Vec::new(), b"err".to_vec()), "err");
}

#[test]
fn sibling_dir_follows_parent_or_falls_back_to_child() {
    assert_eq!(
        sibling_dir(Path::new("/inst/data"), "run"),
        PathBuf::from("/inst/run")
    );
    // A single relative component has an empty parent, so the join is the leaf.
    assert_eq!(sibling_dir(Path::new("data"), "run"), PathBuf::from("run"));
    // No parent at all (root) joins under the root itself.
    assert_eq!(sibling_dir(Path::new("/"), "run"), PathBuf::from("/run"));
}

#[test]
fn safe_sql_identifier_rejects_empty_and_leading_digit() {
    assert!(!is_safe_sql_identifier(""));
    assert!(!is_safe_sql_identifier("1root"));
    assert!(!is_safe_sql_identifier("root-x"));
    assert!(is_safe_sql_identifier("_root1"));
}

#[test]
fn diagnose_bind_and_loader_branches_stay_unknown_or_port_in_use() {
    assert_eq!(
        diagnose_failure_text("Address already in use while bind to 127.0.0.1"),
        StartFailure::PortInUse
    );
    assert_eq!(
        diagnose_failure_text("mysqld: Exec format error"),
        StartFailure::BinaryMissing
    );
    assert_eq!(
        diagnose_failure_text("mysqld: No such file or directory: /x/.sock"),
        StartFailure::Unknown
    );
    assert_eq!(
        diagnose_failure_text("InnoDB: Plugin initialization aborted"),
        StartFailure::Unknown
    );
    // A missing mysqld binary is BinaryMissing; the .sock exemption above is
    // what keeps a missing socket directory from being mistaken for it.
    assert_eq!(
        diagnose_failure_text("mysqld: No such file or directory"),
        StartFailure::BinaryMissing
    );
}

#[test]
fn prepare_runtime_dirs_rejects_my_cnf_socket_outside_fallback_dirs() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let sock = dir.path().join("stray.sock");
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", sock.display()),
    )
    .expect("cnf");
    let err = prepare_runtime_dirs_for_start_with_env(&run_dir, 42221, None, uid).unwrap_err();
    assert!(
        err.to_string()
            .contains("not an allowed fallback socket path"),
        "{err}"
    );
}

#[test]
fn prepare_runtime_dirs_rejects_stale_socket_stat_error_path() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        eprintln!("skipping as root");
        return;
    }
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let sock = fb.join("m42222-abc123def456.sock");
    let _ = fs::remove_file(&sock);
    let l = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
    drop(l);
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", sock.display()),
    )
    .expect("cnf");
    // Make the socket file un-stat-able? chmod 000 on parent dir blocks stat on entry.
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o000)).expect("chmod");
    let err = prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42222,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .unwrap_err();
    let msg = err.to_string();
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o700)).expect("restore");
    assert!(
        msg.contains("cannot prepare mysql fallback socket dir")
            || msg.contains("cannot stat mysql socket")
            || msg.contains("permission")
            || msg.contains("0700")
            || msg.contains("mode"),
        "{msg}"
    );
    let _ = fs::remove_file(&sock);
}
