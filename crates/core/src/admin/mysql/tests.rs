#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

/// A port nothing in this process listens on. It is only ever dialled, never
/// bound (Common Rules 9).
const TEST_PORT: u16 = 65_000;

fn target() -> MysqlTarget {
    // A non-existent socket: calls fail at connect, never at a live server.
    MysqlTarget::new(
        "/nonexistent/dblocal-test.sock",
        TEST_PORT,
        "root",
        SecretString::from("secret-canary-pw"),
    )
}

/// The target with the transport forced to loopback so the non-unix arm is
/// provable on a unix host. Nothing in production reaches `with_transport`.
fn loopback_target() -> MysqlTarget {
    target().with_transport(Transport::Loopback { port: TEST_PORT })
}

fn socket_target() -> MysqlTarget {
    target().with_transport(Transport::Socket(PathBuf::from(
        "/nonexistent/dblocal-test.sock",
    )))
}

#[test]
fn debug_and_display_never_leak_the_password() {
    let target = target().with_timeouts(Duration::from_secs(1), Duration::from_secs(2));
    let rendered = format!("{target:?}");
    assert!(!rendered.contains("secret-canary-pw"), "{rendered}");
    assert!(rendered.contains("[REDACTED]"));
    assert_eq!(target.superuser(), "root");
    assert_eq!(target.connect_timeout(), Duration::from_secs(1));
    assert_eq!(target.statement_timeout(), Duration::from_secs(2));
    assert_eq!(
        target.socket(),
        std::path::Path::new("/nonexistent/dblocal-test.sock")
    );
    assert_eq!(target.port(), TEST_PORT);
}

// --- W4-A3: the transport decision, both arms on a unix host -----------------

#[test]
fn transport_on_unix_is_the_socket_and_never_loopback_even_with_a_port() {
    let socket = Path::new("/run/dblocal/mysqld-40001.sock");
    let chosen = Transport::for_platform(true, socket, 40001);
    assert_eq!(chosen, Transport::Socket(socket.to_path_buf()));
    // The old guarantee, restated: a port being available must not move a unix
    // host onto TCP.
    assert!(
        !matches!(chosen, Transport::Loopback { .. }),
        "unix must never dial TCP: {chosen:?}"
    );
    // Nor does an empty socket path, which is the fictional "TCP fallback".
    assert!(!matches!(
        Transport::for_platform(true, Path::new(""), 40001),
        Transport::Loopback { .. }
    ));
}

#[test]
fn transport_off_unix_is_loopback_tcp_on_the_instance_port() {
    let socket = Path::new("C:/Users/x/.dblocal/instances/db/run/mysqld-40001.sock");
    let chosen = Transport::for_platform(false, socket, 40001);
    assert_eq!(chosen, Transport::Loopback { port: 40001 });
    // The path is not carried into the loopback arm: a Windows host has no
    // socket to fall back to, and nothing may silently use a named pipe.
    assert!(
        !matches!(chosen, Transport::Socket(_)),
        "loopback must not carry a socket: {chosen:?}"
    );
}

#[test]
fn the_target_built_on_this_platform_takes_the_matching_arm() {
    let target = target();
    match (cfg!(unix), target.transport()) {
        (true, Transport::Socket(path)) => {
            assert_eq!(path, Path::new("/nonexistent/dblocal-test.sock"));
        }
        (false, Transport::Loopback { port }) => assert_eq!(*port, TEST_PORT),
        other => panic!("unexpected transport for this platform: {other:?}"),
    }
}

#[test]
fn opts_use_the_socket_and_never_tcp() {
    let target = socket_target();
    let opts = target.opts(Some("appdb"));
    assert_eq!(opts.socket(), Some("/nonexistent/dblocal-test.sock"));
    assert!(opts.prefer_socket());
    assert_eq!(opts.db_name(), Some("appdb"));
    // A named db for the statement, none for the admin connection.
    assert_eq!(target.opts(None).db_name(), Some(""));
    // The socket arm never carries a port to dial; `mysql_async` connects to the
    // socket before it looks at any TCP address.
    assert_eq!(
        target.opts(None).socket(),
        Some("/nonexistent/dblocal-test.sock")
    );
}

#[test]
fn loopback_opts_dial_the_literal_loopback_and_the_instance_port() {
    let opts = loopback_target().opts(Some("appdb"));
    assert_eq!(
        opts.socket(),
        None,
        "a socket on the loopback arm is the bug"
    );
    assert!(
        !opts.prefer_socket(),
        "prefer_socket would retry over a socket"
    );
    assert_eq!(opts.ip_or_hostname(), "127.0.0.1");
    // The account host and the transport host are the same literal by test, not
    // by comment.
    assert_eq!(opts.ip_or_hostname(), MANAGED_ACCOUNT_HOST);
    assert_eq!(opts.tcp_port(), TEST_PORT);
    assert_eq!(opts.db_name(), Some("appdb"));
    assert_eq!(loopback_target().opts(None).db_name(), Some(""));
    assert!(
        !opts.ip_or_hostname().eq_ignore_ascii_case("localhost"),
        "a name resolver must never be consulted"
    );
}

#[test]
fn a_system_schema_is_refused_before_connecting() {
    let admin = MysqlAdmin::new(target());
    for name in ["mysql", "sys", "information_schema", "performance_schema"] {
        let db = Identifier::new(name).expect("valid identifier");
        // These names pass the allowlist (`mysql`, `sys` are not reserved by
        // `Identifier::new`), so the guard is what must reject them.
        let err = MysqlAdmin::create_database(&admin, &db).expect_err(name);
        assert!(matches!(err, AdminError::SystemSchema), "{name}: {err:?}");
        let err = MysqlAdmin::drop_database(&admin, &db, true).expect_err(name);
        assert!(matches!(err, AdminError::SystemSchema), "{name}: {err:?}");
    }
}

#[test]
fn connection_to_a_missing_socket_is_a_typed_connect_error() {
    // Force the socket arm so this test keeps proving the unix path even when
    // it runs on a host whose default transport is loopback.
    let admin = MysqlAdmin::new(socket_target());
    // No live server: every method must not panic and must map to Connect,
    // covering each method's pre-connect glue.
    let db = Identifier::new("appdb").expect("valid");
    let user = Identifier::role("app_user").expect("valid");
    assert_eq!(
        MysqlAdmin::list_databases(&admin).expect_err("db"),
        AdminError::Connect
    );
    assert_eq!(
        MysqlAdmin::list_users(&admin).expect_err("users"),
        AdminError::Connect
    );
    assert_eq!(
        MysqlAdmin::create_database(&admin, &db).expect_err("create"),
        AdminError::Connect
    );
    assert_eq!(
        MysqlAdmin::drop_database(&admin, &db, true).expect_err("drop"),
        AdminError::Connect
    );
    // force=false counts connections first, which also fails at Connect.
    assert_eq!(
        MysqlAdmin::drop_database(&admin, &db, false).expect_err("drop soft"),
        AdminError::Connect
    );
    assert_eq!(
        MysqlAdmin::database_exists(&admin, &db).expect_err("exists"),
        AdminError::Connect
    );
    assert_eq!(
        MysqlAdmin::create_user_at(&admin, &user, "127.0.0.1").expect_err("create user"),
        AdminError::Connect
    );
    assert_eq!(
        MysqlAdmin::grant(&admin, &user, &db, PrivilegeLevel::ReadOnly, "127.0.0.1")
            .expect_err("grant"),
        AdminError::Connect
    );
    assert_eq!(
        MysqlAdmin::drop_user_at(&admin, &user, "127.0.0.1").expect_err("drop user"),
        AdminError::Connect
    );
}

#[test]
fn connection_to_a_closed_loopback_port_is_a_typed_connect_error() {
    // The non-unix arm end to end: an unreachable 127.0.0.1 port maps to
    // `Connect` (DBL-8100) and never panics, with no socket configured.
    let admin = MysqlAdmin::new(loopback_target());
    let db = Identifier::new("appdb").expect("valid");
    assert_eq!(
        MysqlAdmin::list_databases(&admin).expect_err("db"),
        AdminError::Connect
    );
    assert_eq!(
        MysqlAdmin::create_database(&admin, &db).expect_err("create"),
        AdminError::Connect
    );
}

#[test]
fn server_error_codes_map_without_leaking_text() {
    let make = |code: u16, message: &str| {
        mysql_async::Error::Server(ServerError {
            code,
            message: message.to_owned(),
            state: "HY000".to_owned(),
        })
    };
    assert_eq!(
        map_connect_error(make(1045, "Access denied")),
        AdminError::AuthFailed
    );
    assert_eq!(
        map_connect_error(make(1049, "Unknown database")),
        AdminError::DatabaseNotFound
    );
    assert_eq!(
        map_connect_error(make(2003, "can't connect")),
        AdminError::Connect
    );
    assert_eq!(
        map_query_error(make(1007, "exists")),
        AdminError::DatabaseExists
    );
    assert_eq!(
        map_query_error(make(1008, "no db")),
        AdminError::DatabaseNotFound
    );
    assert_eq!(
        map_query_error(make(1142, "denied")),
        AdminError::InsufficientPrivilege
    );
    assert_eq!(
        map_query_error(make(9999, "weird")),
        AdminError::QueryFailed(None)
    );
    // A non-server error is a payload-free query failure, and the driver text
    // (which could be a leaked statement) is dropped.
    let driver = mysql_async::Error::Driver(mysql_async::DriverError::UnexpectedPacket {
        payload: b"secret-canary-pw".to_vec(),
    });
    let mapped = map_query_error(driver);
    assert_eq!(mapped, AdminError::QueryFailed(None));
    let surfaces = format!("{mapped:?} {mapped} {}", mapped.user_message());
    assert!(!surfaces.contains("secret-canary-pw"), "{surfaces}");
}

#[test]
fn role_error_distinguishes_missing_from_duplicate() {
    let make = |message: &str| {
        mysql_async::Error::Server(ServerError {
            code: 1396,
            message: message.to_owned(),
            state: "HY000".to_owned(),
        })
    };
    assert_eq!(
        map_user_error(make("Operation CREATE USER failed for 'u'@'localhost'")),
        AdminError::UserExists
    );
    assert_eq!(
        map_user_error(make(
            "Operation DROP USER failed for 'u'@'localhost'; user doesn't exist"
        )),
        AdminError::UserNotFound
    );
}

#[test]
fn reset_machinery_is_wired_through_the_shared_state_machine() {
    // A reset against a missing socket fails at the first apply (connect), and
    // nothing reaches the sink.
    let admin = MysqlAdmin::new(socket_target());
    struct RecordingSink(std::sync::atomic::AtomicUsize);
    impl CredentialSink for RecordingSink {
        fn store(&self, _password: &SecretString) -> crate::error::Result<()> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }
    let sink = RecordingSink(std::sync::atomic::AtomicUsize::new(0));
    let err = admin
        .reset_superuser_password(&sink)
        .expect_err("no socket");
    assert_eq!(err, AdminError::Connect);
    assert_eq!(sink.0.load(std::sync::atomic::Ordering::SeqCst), 0);
}
