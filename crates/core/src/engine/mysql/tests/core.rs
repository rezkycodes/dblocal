use super::*;

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
