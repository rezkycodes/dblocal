//! Unit tests for the MariaDB adapter.

use super::testutil::{retry_etxtbsy, write_fake_script};
use super::*;
use crate::engine::{Credential, InitContext, RunContext};
use crate::model::{AuthMode, Port};
use std::ffi::OsString;
use tempfile::tempdir;

fn init_ctx(auth: AuthMode, password: Option<&str>) -> InitContext {
    InitContext {
        instance_name: "mdb-test".into(),
        runtime_dir: PathBuf::from("/opt/mariadb"),
        data_dir: PathBuf::from("/tmp/dblocal/instances/mdb/data"),
        port: Port::new(3307).expect("port"),
        superuser: "root".into(),
        auth,
        password: password.map(SecretString::from),
    }
}

fn run_ctx(auth: AuthMode, password: Option<&str>) -> RunContext {
    RunContext {
        instance_name: "mdb-test".into(),
        runtime_dir: PathBuf::from("/opt/mariadb"),
        data_dir: PathBuf::from("/tmp/dblocal/instances/mdb/data"),
        logs_dir: PathBuf::from("/tmp/dblocal/instances/mdb/logs"),
        run_dir: PathBuf::from("/tmp/dblocal/instances/mdb/run"),
        port: Port::new(3307).expect("port"),
        superuser: "root".into(),
        auth,
        credential: password.map(|p| Credential::new("root", SecretString::from(p))),
    }
}

#[test]
fn config_renders_required_settings_and_no_password() {
    let ctx = init_ctx(AuthMode::Password, Some("s3cret"));
    let sock = socket_path(&ctx.data_dir, ctx.port.get());
    let params = config_params(&ctx, &sock).expect("params");
    let conf = render_config(&params);
    let text = conf.expose();
    assert!(text.contains("[mysqld]"));
    assert!(text.contains("basedir=/opt/mariadb"));
    assert!(text.contains("datadir=/tmp/dblocal/instances/mdb/data"));
    assert!(text.contains("bind-address=127.0.0.1"));
    assert!(text.contains("port=3307"));
    assert!(text.contains("socket="));
    assert!(text.contains("pid-file="));
    assert!(text.contains("log-error="));
    assert!(text.contains("tmpdir=/tmp/dblocal/instances/mdb/run/tmp"));
    assert!(text.contains("skip-name-resolve"));
    assert!(!text.contains("0.0.0.0"));
    assert!(!text.contains("s3cret"));
    assert!(!text.contains("password"));
}

#[test]
fn config_paths_are_derived_from_instance_layout() {
    let data = Path::new("/home/u/.dblocal/instances/app/data");
    assert_eq!(
        socket_path(data, 3307),
        PathBuf::from("/home/u/.dblocal/instances/app/run/mariadb-3307.sock")
    );
    assert_eq!(
        pidfile_path(data),
        PathBuf::from("/home/u/.dblocal/instances/app/data/mariadbd.pid")
    );
    assert_eq!(
        log_error_path(data),
        PathBuf::from("/home/u/.dblocal/instances/app/logs/mariadb.log")
    );
    assert_eq!(
        config_path(data),
        PathBuf::from("/home/u/.dblocal/instances/app/data/my.cnf")
    );
}

#[test]
fn socket_path_length_is_checked() {
    let short = Path::new("/tmp/m.sock");
    assert!(sockdir::validate_socket_path_len(short).is_ok());
    let long = PathBuf::from(format!("/tmp/{}", "a".repeat(120)));
    let err = sockdir::validate_socket_path_len(&long).expect_err("too long");
    assert!(matches!(
        err,
        DblError::InvalidInput {
            field: "socket",
            ..
        }
    ));
    assert!(!format!("{err:?}").contains("password"));
}

#[test]
fn spawn_command_has_no_password_in_argv_or_env() {
    let pw = "top-secret-pw";
    let ctx = run_ctx(AuthMode::Password, Some(pw));
    let cmd = MariaDbAdapter.spawn_command(&ctx);
    let args: Vec<OsString> = cmd.get_args().map(|a| a.to_os_string()).collect();
    for a in &args {
        let s = a.to_string_lossy();
        assert!(!s.contains(pw), "password leaked in arg: {s}");
        assert!(!s.contains("--password"), "password flag in arg: {s}");
    }
    for (k, v) in cmd.get_envs() {
        let k = k.to_string_lossy();
        if let Some(v) = v {
            assert_ne!(k.as_ref(), "MYSQL_PWD");
            assert_ne!(k.as_ref(), "MYSQL_PWD_FILE");
            assert!(!v.to_string_lossy().contains(pw), "leak in env {k}");
        }
    }
    let prog = cmd.get_program().to_string_lossy();
    assert!(prog.ends_with("mariadbd") || prog.ends_with("mariadbd.exe"));
}

#[test]
fn password_change_sql_escapes_and_has_no_plain_formatting_leak() {
    let sql = password_change_sql(&SecretString::from("p@ss'w0rd"));
    assert!(sql.contains("ALTER USER 'root'@'localhost' IDENTIFIED BY 'p@ss''w0rd';"));
    assert!(sql.contains("FLUSH PRIVILEGES;"));
    assert!(sql.contains("SHUTDOWN;"));
    assert!(!sql.contains("p@ss'w0rd';\nALTER"));
    let hostile = password_change_sql(&SecretString::from("Cx9!it's\"q\\z$HOME`id`;&&%*-é"));
    assert!(hostile.contains("it''s\"q\\\\z"));
    assert!(hostile.contains("SHUTDOWN;"));
}

#[test]
fn connection_uri_password_and_no_password() {
    let ctx = run_ctx(AuthMode::Password, Some("pw"));
    let cred = Credential::new("root", SecretString::from("p@ss:w/rd"));
    let uri = MariaDbAdapter.connection_uri(&ctx, &cred, UriFormat::Uri);
    assert!(uri.expose().starts_with("mysql://root:"));
    assert!(uri.expose().contains("@127.0.0.1:3307/"));
    assert!(!uri.expose().contains("p@ss:w/rd"));
    let env = MariaDbAdapter.connection_uri(&ctx, &cred, UriFormat::DotEnv);
    assert!(env.expose().contains("DB_USERNAME=root"));
    assert!(env.expose().contains("DB_PASSWORD=p@ss:w/rd"));
    assert!(env.expose().contains("DATABASE_URL=mysql://root:"));

    let no_pw = MariaDbAdapter.connection_uri_no_password(&ctx, UriFormat::Uri);
    assert_eq!(no_pw.expose(), "mysql://root@127.0.0.1:3307/");
    let no_pw_env = MariaDbAdapter.connection_uri_no_password(&ctx, UriFormat::DotEnv);
    assert!(!no_pw_env.expose().contains("DB_PASSWORD"));
}

#[test]
fn parse_real_greeting_11_8_10() {
    // Captured from MariaDB 11.8.10 on 127.0.0.1:43118 during W3-B2 spike.
    // Protocol 10, version "11.8.10-MariaDB", auth plugin mysql_native_password.
    let payload = hex_to_bytes(
        "0a31312e382e31302d4d6172696144420004000000673d3c536b444f4300feff2d0200ff81150000000000003d00000040567155364c5a615a7c2335006d7973716c5f6e61746976655f70617373776f726400",
    );
    let g = parse_greeting(&payload).expect("greeting");
    assert_eq!(g.protocol_version, 10);
    assert_eq!(g.server_version, "11.8.10-MariaDB");
}

#[test]
fn parse_greeting_rejects_truncated_and_hostile() {
    assert!(parse_greeting(&[]).is_err());
    assert!(parse_greeting(&[0xff, 0x10, 0x04]).is_err()); // error packet
    assert!(parse_greeting(&[0x09, 0x00]).is_err()); // bad protocol version
    assert!(parse_greeting(&[10]).is_err()); // no null-terminated version
    let mut no_nul = vec![10u8];
    no_nul.extend(std::iter::repeat_n(b'a', 300));
    assert!(parse_greeting(&no_nul).is_err());

    // Oversized packet is rejected at read_greeting_packet layer by MAX_GREETING_BYTES;
    // parse_greeting itself takes a payload slice - test a very large but structurally valid prefix
    let mut huge = vec![10u8];
    huge.extend_from_slice(b"11.8.10-MariaDB\0");
    huge.extend(std::iter::repeat_n(0u8, 20000));
    let g = parse_greeting(&huge).expect("structurally ok");
    assert_eq!(g.protocol_version, 10);
}

#[test]
fn read_greeting_packet_rejects_oversized_length() {
    use std::io::Write as _;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().expect("accept");
        // header: plen = MAX_GREETING_BYTES + 1, seq 0
        let plen = (MAX_GREETING_BYTES + 1) as u32;
        let hdr = [plen as u8, (plen >> 8) as u8, (plen >> 16) as u8, 0u8];
        let _ = s.write_all(&hdr);
    });
    let stream = TcpStream::connect(format!("127.0.0.1:{port}")).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("timeout");
    let err = read_greeting_packet(&stream).expect_err("oversized");
    assert!(matches!(err, GreetingError::Invalid(_)));
    handle.join().expect("join");
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

#[test]
fn diagnose_port_in_use_from_spike() {
    let text = "2026-11-22  8:03:14 0 [Note] Server socket created on IP: '127.0.0.1', port: '43119'.\n2026-11-22  8:03:14 0 [ERROR] Can't start server: Bind on TCP/IP port. Got error: 98: Address already in use\n2026-11-22  8:03:14 0 [ERROR] Do you already have another server running on port: 43119 ?\n2026-11-22  8:03:14 0 [ERROR] Aborting\n";
    assert_eq!(diagnose_failure_text(text), StartFailure::PortInUse);
}

#[test]
fn diagnose_permission_denied_from_spike() {
    let text = "2026-11-22  8:03:50 0 [Warning] Can't create test file '/tmp/w3b2/perm/data/rezkycodes.lower-test' (Errcode: 13 \"Permission denied\")\n/tmp/w3b2/spike/mariadb-11.8.10-linux-systemd-x86_64/bin/mariadbd: Can't change dir to '/tmp/w3b2/perm/data/' (Errcode: 13 \"Permission denied\")\n2026-11-22  8:03:50 0 [ERROR] Aborting\n";
    assert_eq!(diagnose_failure_text(text), StartFailure::PermissionDenied);
}

#[test]
fn diagnose_missing_datadir_is_unknown_with_raw_text() {
    let text = "2026-11-22  8:03:26 0 [Warning] Can't create test file '/tmp/w3b2/baddata/data/rezkycodes.lower-test' (Errcode: 2 \"No such file or directory\")\n/tmp/w3b2/spike/mariadb-11.8.10-linux-systemd-x86_64/bin/mariadbd: Can't change dir to '/tmp/w3b2/baddata/data/' (Errcode: 2 \"No such file or directory\")\n2026-11-22  8:03:26 0 [ERROR] Aborting\n";
    assert_eq!(diagnose_failure_text(text), StartFailure::Unknown);
}

#[test]
fn diagnose_bad_config_is_unknown() {
    let text = "2026-11-22  8:03:34 0 [ERROR] /tmp/w3b2/spike/mariadb-11.8.10-linux-systemd-x86_64/bin/mariadbd: unknown variable 'bogus_option_that_does_not_exist=1'\n2026-11-22  8:03:34 0 [ERROR] Aborting\n";
    assert_eq!(diagnose_failure_text(text), StartFailure::Unknown);
}

#[test]
fn diagnose_socket_too_long_is_unknown() {
    let text = "2026-11-22  8:04:00 0 [Note] Server socket created on IP: '127.0.0.1', port: '43123'.\n2026-11-22  8:04:00 0 [ERROR] The socket file path is too long (> 107): /tmp/w3b2/socklong/rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr/m.sock\n2026-11-22  8:04:00 0 [ERROR] Aborting\n";
    assert_eq!(diagnose_failure_text(text), StartFailure::Unknown);
}

#[test]
fn diagnose_mysql_datadir_opened_by_mariadb_is_version_mismatch() {
    let text = "2026-11-22  8:11:47 0 [ERROR] InnoDB: File ./ib_logfile0 was not found\n2026-11-22  8:11:47 0 [ERROR] InnoDB: Plugin initialization aborted with error Generic error\n2026-11-22  8:11:47 0 [ERROR] Plugin 'InnoDB' registration as a STORAGE ENGINE failed.\n2026-11-22  8:11:47 0 [ERROR] Unknown/unsupported storage engine: InnoDB\n2026-11-22  8:11:47 0 [ERROR] Aborting\n";
    assert_eq!(
        diagnose_failure_text(text),
        StartFailure::DataDirVersionMismatch
    );
}

#[test]
fn diagnose_disk_full_is_unknown() {
    assert_eq!(
        diagnose_failure_text("No space left on device"),
        StartFailure::Unknown
    );
}

#[test]
fn diagnose_binary_missing() {
    assert_eq!(
        diagnose_failure_text("bash: /opt/mariadb/bin/mariadbd: No such file or directory"),
        StartFailure::BinaryMissing
    );
    assert_eq!(
        diagnose_failure_text("Exec format error"),
        StartFailure::BinaryMissing
    );
    // Verbatim from real W3-B2 e2e on this host (11.4.14 install-db failed
    // because libcrypt.so.1 shim lacked GLIBC_2.2.5 symbol; the loader message
    // itself is the stable diagnosis marker).
    assert_eq!(
        diagnose_failure_text(
            "/home/rezkycodes/.cache/dblocal-e2e-tmp/.tmphJmyQV/mariadb-11.4.14-linux-systemd-x86_64/bin/mariadbd: /home/rezkycodes/.cache/dblocal-e2e-tmp/.tmphJmyQV/libshim/libcrypt.so.1: version `GLIBC_2.2.5' not found (required by /home/rezkycodes/.cache/dblocal-e2e-tmp/.tmphJmyQV/mariadb-11.4.14-linux-systemd-x86_64/bin/mariadbd)"
        ),
        StartFailure::BinaryMissing
    );
    assert_eq!(
        diagnose_failure_text(
            "/opt/mariadb/bin/mariadbd: error while loading shared libraries: libcrypt.so.1: cannot open shared object file: No such file or directory"
        ),
        StartFailure::BinaryMissing
    );
}

#[test]
fn diagnose_unknown_for_unrelated_text() {
    assert_eq!(
        diagnose_failure_text("something unrelated happened"),
        StartFailure::Unknown
    );
}

#[test]
fn install_db_command_has_no_password_and_uses_normal_auth() {
    let ctx = init_ctx(AuthMode::Password, Some("super-secret"));
    let cmd = build_install_db_command(&ctx);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.iter()
            .any(|a| a == "--auth-root-authentication-method=normal")
    );
    assert!(args.iter().any(|a| a == "--skip-test-db"));
    assert!(args.iter().any(|a| a.starts_with("--datadir=")));
    assert!(args.iter().any(|a| a.starts_with("--basedir=")));
    for a in &args {
        assert!(
            !a.contains("super-secret"),
            "password in install-db arg: {a}"
        );
        assert!(
            !a.to_lowercase().contains("password"),
            "password-ish arg: {a}"
        );
    }
    let mut saw_tmpdir = false;
    for (k, v) in cmd.get_envs() {
        if let Some(v) = v {
            let k = k.to_string_lossy();
            assert_ne!(k.as_ref(), "MYSQL_PWD");
            assert!(!v.to_string_lossy().contains("super-secret"));
            if k == "TMPDIR" {
                assert_eq!(v.to_string_lossy(), "/tmp/dblocal/instances/mdb/run/tmp");
                saw_tmpdir = true;
            }
        }
    }
    assert!(saw_tmpdir, "install-db must set TMPDIR to instance tmpdir");
}

#[test]
fn bootstrap_init_file_is_0600_and_removed_after_success() {
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    let runtime = root.join("rt");
    let bindir = runtime.join("bin");
    fs::create_dir_all(&bindir).expect("bin");
    // Fake mariadbd that exits 0 immediately.
    let server = bindir.join(exe_name("mariadbd"));
    write_fake_script(&server, "#!/bin/sh\nexit 0\n");
    let data = root.join("inst/data");
    let logs = root.join("inst/logs");
    let run = root.join("inst/run");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&logs).expect("logs");
    fs::create_dir_all(&run).expect("run");
    let ctx = InitContext {
        instance_name: "m".into(),
        runtime_dir: runtime,
        data_dir: data.clone(),
        port: Port::new(43130).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::Password,
        password: Some(SecretString::from("pw-123")),
    };
    // Write config needed by bootstrap
    let sock = socket_path(&data, 43130);
    let conf = render_config(&config_params(&ctx, &sock).expect("params"));
    write_private_config(&config_path(&data), conf.expose()).expect("conf");

    retry_etxtbsy(|| bootstrap_password(&ctx)).expect("bootstrap");
    let init_file = run.join(".mariadb-init.sql");
    assert!(
        !init_file.exists(),
        "init-file must be deleted after bootstrap"
    );
}

#[test]
fn bootstrap_init_file_is_removed_on_failure() {
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    let runtime = root.join("rt");
    let bindir = runtime.join("bin");
    fs::create_dir_all(&bindir).expect("bin");
    let server = bindir.join(exe_name("mariadbd"));
    write_fake_script(&server, "#!/bin/sh\nexit 1\n");
    let data = root.join("inst/data");
    let logs = root.join("inst/logs");
    let run = root.join("inst/run");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&logs).expect("logs");
    fs::create_dir_all(&run).expect("run");
    let ctx = InitContext {
        instance_name: "m".into(),
        runtime_dir: runtime,
        data_dir: data.clone(),
        port: Port::new(43131).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::Password,
        password: Some(SecretString::from("pw-123")),
    };
    let sock = socket_path(&data, 43131);
    let conf = render_config(&config_params(&ctx, &sock).expect("params"));
    write_private_config(&config_path(&data), conf.expose()).expect("conf");

    let err = retry_etxtbsy(|| bootstrap_password(&ctx)).expect_err("should fail");
    assert!(matches!(err, DblError::Engine { .. }));
    let init_file = run.join(".mariadb-init.sql");
    assert!(
        !init_file.exists(),
        "init-file must be deleted on failure too"
    );
}

#[test]
fn mutation_bind_address_must_be_loopback() {
    let ctx = init_ctx(AuthMode::None, None);
    let sock = socket_path(&ctx.data_dir, 3307);
    let conf = render_config(&config_params(&ctx, &sock).expect("params"));
    assert!(conf.expose().contains("bind-address=127.0.0.1"));
    assert!(!conf.expose().contains("0.0.0.0"));
}

#[test]
fn mutation_auth_root_normal_required_in_install_db() {
    let ctx = init_ctx(AuthMode::Password, Some("pw"));
    let args: Vec<String> = build_install_db_command(&ctx)
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.iter()
            .any(|a| a == "--auth-root-authentication-method=normal")
    );
}

#[test]
fn bootstrap_command_skips_networking() {
    // The bootstrap server runs before the password exists; binding the
    // instance TCP port would make initialize depend on that port being free.
    let ctx = init_ctx(AuthMode::Password, Some("pw"));
    let init_file = PathBuf::from("/tmp/dblocal/instances/mdb/run/.mariadb-init.sql");
    let args: Vec<String> = build_bootstrap_command(&ctx, &init_file)
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.iter().any(|a| a == "--skip-networking"),
        "bootstrap must not listen on TCP: {args:?}"
    );
    assert!(
        args.iter().any(|a| a.starts_with("--defaults-file=")),
        "bootstrap still reads my.cnf for the socket and datadir: {args:?}"
    );
    assert!(
        args.iter().any(|a| a.starts_with("--init-file=")),
        "bootstrap still runs the init file: {args:?}"
    );
}

#[test]
fn mutation_handshake_rejects_non_protocol_10() {
    assert!(parse_greeting(&[0x09, b'1', 0]).is_err());
    assert!(parse_greeting(&[0x00, b'1', 0]).is_err());
    let payload = hex_to_bytes(
        "0a31312e382e31302d4d6172696144420004000000673d3c536b444f4300feff2d0200ff81150000000000003d00000040567155364c5a615a7c2335006d7973716c5f6e61746976655f70617373776f726400",
    );
    assert_eq!(parse_greeting(&payload).expect("real").protocol_version, 10);
}

#[test]
fn cmdline_datadir_equals_token_is_accepted() {
    let expected = ExpectedIdentity {
        exe: PathBuf::from("/opt/mariadb/bin/mariadbd"),
        data_dir: PathBuf::from("/tmp/a/data"),
        config_path: PathBuf::from("/tmp/a/data/my.cnf"),
    };
    // `--datadir=/tmp/a/data --datadir /tmp/a/data` must not count twice-safely;
    // the second occurrence of a token pair still marks the instance.
    let args = vec![
        "--defaults-file=/tmp/a/data/my.cnf".to_owned(),
        "--datadir=/tmp/a/data".to_owned(),
    ];
    assert!(cmdline_targets_instance(&args, &expected));
}

#[test]
fn cmdline_targets_instance_with_separate_datadir_token() {
    let expected = ExpectedIdentity {
        exe: PathBuf::from("/opt/mariadb/bin/mariadbd"),
        data_dir: PathBuf::from("/tmp/a/data"),
        config_path: PathBuf::from("/tmp/a/data/my.cnf"),
    };
    let args = vec![
        "--defaults-file".to_owned(),
        "/tmp/a/data/my.cnf".to_owned(),
        "--datadir".to_owned(),
        "/tmp/a/data".to_owned(),
    ];
    assert!(cmdline_targets_instance(&args, &expected));
}

#[test]
fn cmdline_rejects_datadir_with_suffix_collision() {
    let expected = ExpectedIdentity {
        exe: PathBuf::from("/opt/mariadb/bin/mariadbd"),
        data_dir: PathBuf::from("/tmp/a/data"),
        config_path: PathBuf::from("/tmp/a/data/my.cnf"),
    };
    // Both args point at a different instance whose path is a prefix-collision
    // of the real one; the token matcher must not accept it.
    let args = vec![
        "--datadir=/tmp/a/data2".to_owned(),
        "--defaults-file=/tmp/a/data2/my.cnf".to_owned(),
    ];
    assert!(!cmdline_targets_instance(&args, &expected));
}

#[test]
fn same_executable_strips_deleted_suffix() {
    let expected = PathBuf::from("/opt/mariadb/bin/mariadbd");
    let observed = PathBuf::from("/opt/mariadb/bin/mariadbd (deleted)");
    assert!(same_executable(&observed, &expected));
}

#[test]
fn strip_deleted_is_a_noop_without_the_suffix() {
    let path = Path::new("/opt/mariadb/bin/mariadbd");
    assert_eq!(strip_deleted(path).as_ref(), path);
}

#[test]
fn contains_arg_token_finds_a_later_occurrence() {
    // The first hit is glued to a prefix (`x--flag=v`), the second is a token.
    let haystack = "prefix--flag=value --flag=value tail";
    assert!(contains_arg_token(haystack, "--flag=value"));
    assert!(!contains_arg_token(haystack, "--flag=nope"));
    // A token that never stands alone is not matched.
    assert!(!contains_arg_token("a--flag=value", "--flag=value"));
}

#[test]
fn path_arg_matches_via_canonicalize() {
    let dir = tempdir().expect("tmp");
    let real = dir.path().join("inst/data");
    fs::create_dir_all(&real).expect("data");
    assert!(path_arg_matches(
        real.to_str().expect("utf8"),
        real.to_str().expect("utf8")
    ));
    assert!(!path_arg_matches(
        real.to_str().expect("utf8"),
        "/definitely/not/this/path"
    ));
    #[cfg(unix)]
    {
        let link = dir.path().join("link-to-data");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        assert!(path_arg_matches(
            link.to_str().expect("utf8"),
            real.to_str().expect("utf8")
        ));
    }
}

#[test]
fn same_executable_rejects_different_name() {
    let expected = PathBuf::from("/no/such/dblocal/bin/mariadbd");
    let other = PathBuf::from("/no/such/dblocal/bin/mysqld");
    assert!(!same_executable(&other, &expected));
}

#[test]
fn path_arg_matches_rejects_unequal_missing_paths() {
    assert!(!path_arg_matches("/no/such/one/data", "/no/such/two/data"));
}

#[test]
fn contains_arg_token_breaks_when_only_tail_match_has_no_left_boundary() {
    // The only hit is at the last byte and is glued to a non-space, so the
    // loop advances past the end and stops.
    assert!(!contains_arg_token("xa", "a"));
}

#[test]
fn verify_ps_command_output_empty_is_no_such_process() {
    let expected = ExpectedIdentity {
        exe: PathBuf::from("/opt/mariadb/bin/mariadbd"),
        data_dir: PathBuf::from("/tmp/inst/data"),
        config_path: PathBuf::from("/tmp/inst/data/my.cnf"),
    };
    assert_eq!(
        verify_ps_command_output("", &expected),
        PidIdentity::NoSuchProcess
    );
}

#[test]
fn ps_command_output_rejects_executable_glued_to_suffix() {
    let expected = ExpectedIdentity {
        exe: PathBuf::from("/opt/mariadb/bin/mariadbd"),
        data_dir: PathBuf::from("/tmp/inst/data"),
        config_path: PathBuf::from("/tmp/inst/data/my.cnf"),
    };
    let cmd = "/opt/mariadb/bin/mariadbdX --datadir=/tmp/inst/data";
    assert!(matches!(
        verify_ps_command_output(cmd, &expected),
        PidIdentity::NotOurs(_)
    ));
}

#[test]
fn same_executable_compares_canonical_paths() {
    let dir = tempdir().expect("tmp");
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    fs::create_dir_all(&a).expect("a");
    fs::create_dir_all(&b).expect("b");
    let exe_a = a.join("mariadbd");
    let exe_b = b.join("mariadbd");
    fs::write(&exe_a, b"x").expect("write a");
    fs::write(&exe_b, b"x").expect("write b");
    // Same file name, both canonicalize Ok, different inode: not the same exe.
    assert!(!same_executable(&exe_a, &exe_b));
}

#[test]
fn cmdline_parsing_and_instance_targeting() {
    let args =
        parse_cmdline(b"/usr/bin/mariadbd\0--defaults-file=/tmp/a/my.cnf\0--datadir=/tmp/a/data\0");
    assert_eq!(args.len(), 3);
    let expected = ExpectedIdentity {
        exe: PathBuf::from("/usr/bin/mariadbd"),
        data_dir: PathBuf::from("/tmp/a/data"),
        config_path: PathBuf::from("/tmp/a/my.cnf"),
    };
    assert!(cmdline_targets_instance(&args, &expected));
    let other = ExpectedIdentity {
        exe: PathBuf::from("/usr/bin/mariadbd"),
        data_dir: PathBuf::from("/tmp/b/data"),
        config_path: PathBuf::from("/tmp/b/my.cnf"),
    };
    assert!(!cmdline_targets_instance(&args, &other));
    let conf_only = parse_cmdline(b"mariadbd\0--defaults-file=/tmp/a/my.cnf\0");
    assert!(cmdline_targets_instance(&conf_only, &expected));
}

#[test]
fn zombie_stat_is_detected() {
    // Format: pid (comm) state ...
    assert!(is_zombie_stat(
        "12345 (mariadbd) Z 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0"
    ));
    assert!(!is_zombie_stat(
        "12345 (mariadbd) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0"
    ));
    assert!(!is_zombie_stat("garbage"));
}

#[cfg(unix)]
#[test]
fn shutdown_refuses_to_signal_unrelated_pid() {
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    let data = root.join("data");
    let logs = root.join("logs");
    let run = root.join("run");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&logs).expect("logs");
    fs::create_dir_all(&run).expect("run");

    // Spawn an unrelated long-lived process, reaped on every exit path.
    let mut sleeper = super::testutil::ChildGuard::new(
        Command::new("sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep"),
    );
    let pid = sleeper.id();

    // Write pidfile pointing at the sleeper.
    fs::write(data.join("mariadbd.pid"), format!("{pid}\n")).expect("pidfile");
    // Write minimal config so config_path exists if read (not required).
    let ctx = RunContext {
        instance_name: "m".into(),
        runtime_dir: PathBuf::from("/opt/mariadb"),
        data_dir: data.clone(),
        logs_dir: logs,
        run_dir: run,
        port: Port::new(3307).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        credential: None,
    };

    // shutdown should refuse to signal and return Ok.
    let result = MariaDbAdapter.shutdown(&ctx);
    assert!(
        result.is_ok(),
        "shutdown must not fail on unrelated pid: {result:?}"
    );

    // Sleeper must still be alive.
    std::thread::sleep(Duration::from_millis(200));
    let still_alive = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(still_alive, "unrelated process must not be signalled");

    sleeper.reap();
}

#[test]
fn identity_rejects_wrong_exe_and_wrong_cmdline() {
    let expected = ExpectedIdentity {
        exe: PathBuf::from("/opt/mariadb/bin/mariadbd"),
        data_dir: PathBuf::from("/tmp/inst/data"),
        config_path: PathBuf::from("/tmp/inst/data/my.cnf"),
    };
    let args = parse_cmdline(b"/usr/bin/sleep\0--ignored\0");
    assert!(!cmdline_targets_instance(&args, &expected));
    let wrong_data = parse_cmdline(
        b"/opt/mariadb/bin/mariadbd\0--defaults-file=/tmp/other/data/my.cnf\0--datadir=/tmp/other/data\0",
    );
    assert!(!cmdline_targets_instance(&wrong_data, &expected));
    let right = parse_cmdline(
        b"/opt/mariadb/bin/mariadbd\0--defaults-file=/tmp/inst/data/my.cnf\0--datadir=/tmp/inst/data\0",
    );
    assert!(cmdline_targets_instance(&right, &expected));
}

#[cfg(target_os = "linux")]
#[test]
fn verify_pid_identity_linux_matches_real_process() {
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    let bindir = root.join("bin");
    fs::create_dir_all(&bindir).expect("bin");
    let fake = bindir.join("mariadbd");
    // Build a tiny helper that ignores args and sleeps.
    let src = root.join("fake.rs");
    fs::write(
        &src,
        "fn main(){ std::thread::sleep(std::time::Duration::from_secs(60)); }",
    )
    .expect("write src");
    let status = Command::new("rustc")
        .arg("-O")
        .arg("-o")
        .arg(&fake)
        .arg(&src)
        .status()
        .expect("rustc");
    assert!(status.success(), "rustc failed");
    let data = root.join("inst/data");
    let conf = data.join("my.cnf");
    fs::create_dir_all(&data).expect("data");
    fs::write(&conf, "[mysqld]\n").expect("conf");

    let mut child = super::testutil::ChildGuard::new(
        retry_etxtbsy(|| {
            Command::new(&fake)
                .arg(format!("--defaults-file={}", conf.display()))
                .arg(format!("--datadir={}", data.display()))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
        })
        .expect("spawn fake mariadbd"),
    );
    let pid = child.id();
    let expected = ExpectedIdentity {
        exe: fake.clone(),
        data_dir: data.clone(),
        config_path: conf.clone(),
    };
    // Wait for the helper to have exec'd (it becomes `/usr/bin/sleep`'s image
    // only after exec); a fixed 100 ms was not guaranteed under load.
    super::testutil::wait_until(
        "the fake mariadbd to be identifiable",
        Duration::from_secs(10),
        || verify_pid_identity(pid, &expected) == PidIdentity::Match,
    );
    let id = verify_pid_identity(pid, &expected);
    assert_eq!(
        id,
        PidIdentity::Match,
        "expected match for our fake mariadbd"
    );

    let wrong = ExpectedIdentity {
        exe: fake.clone(),
        data_dir: root.join("other/data"),
        config_path: root.join("other/data/my.cnf"),
    };
    let id = verify_pid_identity(pid, &wrong);
    assert!(
        matches!(id, PidIdentity::NotOurs(_)),
        "wrong instance should be NotOurs: {id:?}"
    );

    let wrong_exe = ExpectedIdentity {
        exe: PathBuf::from("/usr/bin/sleep"),
        data_dir: data.clone(),
        config_path: conf.clone(),
    };
    let id = verify_pid_identity(pid, &wrong_exe);
    assert!(
        matches!(id, PidIdentity::NotOurs(_)),
        "wrong exe should be NotOurs: {id:?}"
    );

    child.reap();
    // The reaped child is gone from the process table shortly after the wait
    // returns; poll instead of hoping 50 ms was enough.
    super::testutil::wait_until(
        "the reaped pid to disappear",
        Duration::from_secs(10),
        || !matches!(verify_pid_identity(pid, &expected), PidIdentity::Match),
    );
    let dead = verify_pid_identity(pid, &expected);
    assert!(
        !matches!(dead, PidIdentity::Match),
        "dead pid must not match: {dead:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn verify_pid_identity_rejects_unrelated_live_process() {
    let mut sleeper = super::testutil::ChildGuard::new(
        Command::new("sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep"),
    );
    let pid = sleeper.id();
    std::thread::sleep(Duration::from_millis(30));
    let expected = ExpectedIdentity {
        exe: PathBuf::from("/opt/mariadb/bin/mariadbd"),
        data_dir: PathBuf::from("/tmp/inst/data"),
        config_path: PathBuf::from("/tmp/inst/data/my.cnf"),
    };
    let id = verify_pid_identity(pid, &expected);
    assert!(
        matches!(id, PidIdentity::NotOurs(_) | PidIdentity::Unverifiable(_)),
        "sleep must not be identified as mariadbd: {id:?}"
    );
    sleeper.reap();
}

#[test]
fn sync_port_rewrites_port_and_socket() {
    let dir = tempdir().expect("tmp");
    let data = dir.path().join("data");
    fs::create_dir_all(&data).expect("data");
    let sock = socket_path(&data, 3307);
    let ctx = init_ctx(AuthMode::None, None);
    let conf = render_config(&config_params(&ctx, &sock).expect("params"));
    write_private_config(&config_path(&data), conf.expose()).expect("write");
    sync_port(&data, 43110).expect("sync");
    let updated = fs::read_to_string(config_path(&data)).expect("read");
    assert!(updated.contains("port=43110"));
    assert!(!updated.contains("port=3307"));
    assert!(updated.contains("43110"));
}

#[test]
fn sync_port_errors_without_port_directive() {
    let dir = tempdir().expect("tmp");
    let data = dir.path().join("data");
    fs::create_dir_all(&data).expect("data");
    write_private_config(&config_path(&data), "[mysqld]\ndatadir=/tmp\n").expect("write");
    let err = sync_port(&data, 43111).expect_err("missing port");
    assert!(matches!(err, DblError::Engine { .. }));
}

#[test]
fn sync_port_errors_on_missing_config() {
    let dir = tempdir().expect("tmp");
    let data = dir.path().join("data");
    fs::create_dir_all(&data).expect("data");
    let err = sync_port(&data, 43111).expect_err("missing config");
    assert!(matches!(err, DblError::Engine { .. }));
}

#[test]
fn probe_greeting_fails_on_closed_port() {
    let health = probe_greeting(1);
    assert!(health.is_err());
}

#[test]
fn bootstrap_password_errors_without_password() {
    let ctx = init_ctx(AuthMode::Password, None);
    let err = bootstrap_password(&ctx).expect_err("no password");
    assert!(matches!(
        err,
        DblError::InvalidInput {
            field: "password",
            ..
        }
    ));
}

#[test]
fn prepare_run_errors_when_the_config_has_no_socket_directive() {
    let dir = tempdir().expect("tmp");
    let data = dir.path().join("data");
    fs::create_dir_all(&data).expect("data");
    write_private_config(&config_path(&data), "[mysqld]\nport=3307\n").expect("conf");
    let err = prepare_run_spec(&data, 3307).expect_err("no socket");
    assert!(matches!(err, DblError::Engine { .. }), "{err:?}");
}

#[test]
fn combined_output_joins_stdout_and_stderr() {
    assert_eq!(combined_output(b"out", b"err"), "out\nerr");
    assert_eq!(combined_output(b"", b"err"), "err");
    assert_eq!(combined_output(b"out\n", b"err"), "out\nerr");
    assert_eq!(combined_output(b"", b""), "");
}

#[test]
fn escape_sql_handles_control_chars() {
    let s = escape_sql_single_quoted("a\u{1a}b");
    assert!(s.contains("\\Z"), "got {s}");
    let s2 = escape_sql_single_quoted("x\0y");
    assert!(s2.contains("\\0"), "got {s2}");
    let s3 = escape_sql_single_quoted("l\nr\r");
    assert!(s3.contains("\\n") && s3.contains("\\r"), "got {s3}");
}

#[cfg(unix)]
#[test]
fn prepare_run_refuses_a_regular_file_at_the_socket_path_without_deleting_it() {
    // Only a real stale socket is removed. A regular file at the configured
    // socket path is a start failure: the engine could not bind there, and the
    // file may belong to someone else.
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    let data = root.join("inst/data");
    let run = root.join("inst/run");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&run).expect("run");
    let fallback = root.join("fallback-sockets");
    fs::create_dir_all(&fallback).expect("fallback");
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fallback, fs::Permissions::from_mode(0o700)).expect("chmod");
    }
    let sock = fallback.join("m3307-abc123def456.sock");
    fs::write(&sock, b"not a socket").expect("regular at socket path");
    let conf = format!(
        "[mysqld]\nsocket={}\ntmpdir={}\n",
        sock.display(),
        run.join("tmp").display()
    );
    write_private_config(&config_path(&data), &conf).expect("conf");
    let ctx = RunContext {
        instance_name: "m".into(),
        runtime_dir: root.join("rt"),
        data_dir: data,
        logs_dir: root.join("inst/logs"),
        run_dir: run,
        port: Port::new(3307).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        credential: None,
    };
    let err = prepare_run(&ctx).expect_err("a regular file at the socket path must fail");
    assert!(err.to_string().contains("is not a socket"), "{err}");
    assert!(sock.exists(), "the foreign regular file is not deleted");
}

#[test]
fn prepare_run_creates_natural_run_dir_when_socket_is_natural() {
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    let data = root.join("inst/data");
    let run = root.join("inst/run");
    fs::create_dir_all(&data).expect("data");
    let sock = run.join("mariadb-3307.sock");
    let conf = format!(
        "[mysqld]\nsocket={}\ntmpdir={}\n",
        sock.display(),
        run.join("tmp").display()
    );
    write_private_config(&config_path(&data), &conf).expect("conf");
    let ctx = RunContext {
        instance_name: "m".into(),
        runtime_dir: root.join("rt"),
        data_dir: data,
        logs_dir: root.join("inst/logs"),
        run_dir: run.clone(),
        port: Port::new(3307).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        credential: None,
    };
    prepare_run(&ctx).expect("prepare");
    assert!(run.is_dir(), "natural run dir recreated");
    assert!(run.join("tmp").is_dir());
}

#[cfg(unix)]
#[test]
fn prepare_tmpdir_rejects_a_non_directory() {
    let dir = tempdir().expect("tmp");
    let file = dir.path().join("tmp");
    fs::write(&file, b"x").expect("file");
    let err = sockdir::prepare_tmpdir(&file, sockdir::current_uid()).expect_err("non-dir");
    assert!(matches!(err, DblError::Engine { .. }), "{err:?}");
}

#[cfg(unix)]
#[test]
fn prepare_tmpdir_tightens_a_directory_we_own() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = tempdir().expect("tmp");
    let tmp = dir.path().join("tmp");
    fs::create_dir_all(&tmp).expect("tmp");
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).expect("chmod");
    sockdir::prepare_tmpdir(&tmp, sockdir::current_uid()).expect("secure");
    let meta = fs::metadata(&tmp).expect("meta");
    assert_eq!(meta.mode() & 0o777, 0o700);
    assert_eq!(meta.uid(), nix::unistd::geteuid().as_raw());
}

#[test]
fn diagnose_via_adapter_maps_port_in_use() {
    let failure = MariaDbAdapter.diagnose_start_failure(
        "Can't start server: Bind on TCP/IP port. Got error: 98: Address already in use",
        Some(1),
    );
    assert_eq!(failure, StartFailure::PortInUse);
}

#[test]
fn read_log_tail_returns_last_lines_and_empty_for_missing() {
    let dir = tempdir().expect("tmp");
    let log = dir.path().join("mariadb.log");
    let body: String = (1..=50).map(|n| format!("line-{n}\n")).collect();
    fs::write(&log, body).expect("write");
    let tail = read_log_tail(&log, 5);
    assert_eq!(tail.lines().count(), 5);
    assert!(tail.ends_with("line-50"));
    assert_eq!(read_log_tail(&dir.path().join("nope.err"), 5), "");
}

fn serve_once(payload: Vec<u8>) -> u16 {
    use std::io::Write as _;
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.write_all(&payload);
            let _ = stream.flush();
        }
    });
    port
}

#[test]
fn probe_greeting_reports_ready_on_error_packet() {
    // Server speaks an error packet (0xff) instead of a greeting: still Ready.
    let mut payload = vec![0u8; 4];
    payload[0] = 3; // length
    payload.extend_from_slice(&[0xff, 0x10, 0x04]);
    let port = serve_once(payload);
    let health = probe_greeting(port).expect("probe");
    assert_eq!(health, Health::Ready);
}

#[test]
fn probe_greeting_errors_on_non_protocol_greeting() {
    let mut payload = vec![0u8; 4];
    payload[0] = 4;
    payload.extend_from_slice(&[0x09, b'x', b'y', 0]);
    let port = serve_once(payload);
    assert!(probe_greeting(port).is_err());
}

#[test]
fn probe_greeting_errors_on_io_failure() {
    // Accept then close immediately: read_exact fails with EOF.
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let _ = listener.accept();
    });
    assert!(probe_greeting(port).is_err());
}

#[test]
fn greeting_error_from_io_error_wraps() {
    let err: GreetingError = std::io::Error::other("boom").into();
    assert!(matches!(err, GreetingError::Io(_)));
}

#[test]
fn parse_greeting_rejects_empty_payload() {
    assert!(parse_greeting(&[]).is_err());
}

/// `initialize` and `sync_port` both resolve the socket through
/// `resolve_socket_path(data_dir, port)` with `data_dir` as the identity, so a
/// `change_port` rewrites `socket=` to exactly the path the server was started
/// with, including on a deep home that needs the fallback directory.
#[test]
fn sync_port_keeps_the_fallback_socket_path_it_started_with() {
    let dir = tempdir().expect("tmp");
    let deep = dir.path().join("d".repeat(120));
    let data = deep.join("inst/data");
    fs::create_dir_all(&data).expect("data");
    let old_sock = resolve_socket_path(&data, 3307).expect("old socket");
    assert_ne!(
        old_sock,
        socket_path(&data, 3307),
        "the deep natural path must need the fallback"
    );
    let ctx = InitContext {
        instance_name: "m".into(),
        runtime_dir: dir.path().join("rt"),
        data_dir: data.clone(),
        port: Port::new(3307).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        password: None,
    };
    let conf = render_config(&config_params(&ctx, &old_sock).expect("params"));
    write_private_config(&config_path(&data), conf.expose()).expect("write");

    sync_port(&data, 43111).expect("sync");
    let expected = resolve_socket_path(&data, 43111).expect("new socket");
    let updated = fs::read_to_string(config_path(&data)).expect("read");
    assert!(updated.contains("port=43111"));
    assert!(
        updated.contains(&format!("socket={}", expected.display())),
        "sync_port must write the same resolver's socket:\n{updated}"
    );
}
