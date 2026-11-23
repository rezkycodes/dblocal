#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

mod fake;

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

/// A target whose loopback port is a live fake server. This is the Windows
/// arm's `Opts` (no socket, `prefer_socket(false)`, `127.0.0.1:<port>`) talking
/// to a real peer, so the closures after `Conn::new` are actually executed.
fn live_target(server: &fake::FakeServer) -> MysqlTarget {
    target().with_transport(Transport::Loopback {
        port: server.port(),
    })
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

// --- Live fake server: the loopback arm against a real peer ------------------
//
// A tiny MySQL-protocol server (see `tests/fake.rs`) answers on an ephemeral
// loopback port. Every test here dials it through the production loopback `Opts`
// — no socket, `prefer_socket(false)`, the literal `127.0.0.1` — so the code
// after `Conn::new` runs for real, and the statement text that reaches the wire
// is asserted.

fn live(admin_reply: impl Fn(&str) -> fake::Reply + Send + Sync + 'static) -> fake::FakeServer {
    fake::FakeServer::start(admin_reply)
}

#[test]
fn list_databases_reads_names_and_sizes_off_the_wire() {
    let server = live(|sql| {
        if sql.contains("information_schema.SCHEMATA") {
            fake::Reply::Rows {
                columns: vec![fake::Col::text("SCHEMA_NAME"), fake::Col::int("size")],
                rows: vec![
                    vec![Some("appdb".to_owned()), Some("4096".to_owned())],
                    vec![Some("other".to_owned()), None],
                ],
            }
        } else {
            fake::Reply::Ok
        }
    });
    let admin = MysqlAdmin::new(live_target(&server));
    let databases = admin.list_databases().expect("list databases");
    assert_eq!(databases.len(), 2);
    assert_eq!(databases[0].name(), "appdb");
    assert_eq!(databases[0].size_bytes(), 4096);
    // A NULL size is reported as 0, never a panic.
    assert_eq!(databases[1].name(), "other");
    assert_eq!(databases[1].size_bytes(), 0);
    assert!(server.saw("information_schema.SCHEMATA"));
    // The list never targets a db.
    assert_eq!(
        admin.target().transport(),
        &Transport::Loopback {
            port: server.port()
        }
    );
}

#[test]
fn list_users_collapses_hosts_and_skips_empty_accounts() {
    let server = live(|sql| {
        if sql.contains("mysql.user") {
            fake::Reply::Rows {
                columns: vec![fake::Col::text("User"), fake::Col::text("Host")],
                rows: vec![
                    vec![Some("root".to_owned()), Some("localhost".to_owned())],
                    vec![Some("root".to_owned()), Some("127.0.0.1".to_owned())],
                    vec![Some("app".to_owned()), Some("127.0.0.1".to_owned())],
                ],
            }
        } else {
            fake::Reply::Ok
        }
    });
    let admin = MysqlAdmin::new(live_target(&server));
    let users = admin.list_users().expect("list users");
    let names: Vec<&str> = users.iter().map(|user| user.name()).collect();
    assert_eq!(names, vec!["root", "app"], "one row per account name");
    assert!(
        users
            .iter()
            .all(|user| user.can_login() && !user.is_superuser())
    );
    assert!(server.saw("mysql.user"));
}

#[test]
fn create_database_sends_utf8mb4_and_maps_a_duplicate() {
    let server = live(|sql| {
        if sql.contains("CREATE DATABASE") {
            fake::Reply::Err {
                code: 1007,
                message: "database exists",
            }
        } else {
            fake::Reply::Ok
        }
    });
    let admin = MysqlAdmin::new(live_target(&server));
    let db = Identifier::new("appdb").expect("valid");
    let err = admin.create_database(&db).expect_err("duplicate");
    assert_eq!(err, AdminError::DatabaseExists);
    assert!(
        server.saw("CREATE DATABASE `appdb` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci"),
        "the create must ask for utf8mb4: {:?}",
        server.statements()
    );
}

#[test]
fn drop_database_refuses_a_live_session_then_drops() {
    // The `force=false` pre-check counts sessions; a positive count is
    // `DatabaseInUse` and no DROP is sent.
    let busy = live(|sql| {
        if sql.contains("PROCESSLIST") {
            fake::count(2)
        } else {
            fake::Reply::Ok
        }
    });
    let admin = MysqlAdmin::new(live_target(&busy));
    let db = Identifier::new("appdb").expect("valid");
    let err = admin.drop_database(&db, false).expect_err("busy");
    assert_eq!(err, AdminError::DatabaseInUse);
    assert!(
        !busy.saw("DROP DATABASE"),
        "a busy database must not be dropped: {:?}",
        busy.statements()
    );

    // No sessions: the DROP goes out.
    let quiet = live(|sql| {
        if sql.contains("PROCESSLIST") {
            fake::count(0)
        } else {
            fake::Reply::Ok
        }
    });
    let admin = MysqlAdmin::new(live_target(&quiet));
    admin.drop_database(&db, false).expect("drop");
    assert!(
        quiet.saw("DROP DATABASE `appdb`"),
        "{:?}",
        quiet.statements()
    );

    // `force=true` skips the pre-check entirely.
    let forced = live(|_| fake::Reply::Ok);
    MysqlAdmin::new(live_target(&forced))
        .drop_database(&db, true)
        .expect("forced drop");
    assert!(
        !forced.saw("PROCESSLIST"),
        "force must not count sessions: {:?}",
        forced.statements()
    );
    assert!(forced.saw("DROP DATABASE `appdb`"));
}

#[test]
fn database_exists_reads_the_count() {
    for (value, expected) in [(1i64, true), (0i64, false)] {
        let server = live(move |sql| {
            if sql.contains("SCHEMATA") {
                fake::count(value)
            } else {
                fake::Reply::Ok
            }
        });
        let admin = MysqlAdmin::new(live_target(&server));
        let db = Identifier::new("appdb").expect("valid");
        assert_eq!(admin.database_exists(&db).expect("exists"), expected);
    }
}

#[test]
fn create_user_grant_and_drop_user_reach_the_wire() {
    let server = live(|_| fake::Reply::Ok);
    let admin = MysqlAdmin::new(live_target(&server));
    let user = Identifier::role("app_user").expect("valid role");
    let db = Identifier::new("appdb").expect("valid");

    let password = admin
        .create_user_at(&user, MANAGED_ACCOUNT_HOST)
        .expect("create user");
    assert!(!password.expose().is_empty());
    assert!(
        server.saw("CREATE USER `app_user`@'127.0.0.1' IDENTIFIED BY"),
        "{:?}",
        server.statements()
    );
    // The password literal never reaches a log or an error; here we only prove
    // the statement went out and the value is returned once.

    admin
        .grant(&user, &db, PrivilegeLevel::ReadOnly, MANAGED_ACCOUNT_HOST)
        .expect("grant ro");
    assert!(
        server.saw("GRANT SELECT ON `appdb`.* TO `app_user`@'127.0.0.1'"),
        "{:?}",
        server.statements()
    );

    admin
        .grant(&user, &db, PrivilegeLevel::ReadWrite, MANAGED_ACCOUNT_HOST)
        .expect("grant rw");
    assert!(
        server.saw("GRANT SELECT, INSERT, UPDATE, DELETE ON `appdb`.*"),
        "{:?}",
        server.statements()
    );

    admin
        .drop_user_at(&user, MANAGED_ACCOUNT_HOST)
        .expect("drop user");
    assert!(
        server.saw("DROP USER `app_user`@'127.0.0.1'"),
        "{:?}",
        server.statements()
    );

    // Every password-bearing statement ran with the query log off first.
    assert!(
        server.saw("SET sql_log_off = 1"),
        "sql_log_off must precede a password statement"
    );
}

#[test]
fn a_missing_host_account_is_skipped_but_other_role_errors_are_reported() {
    // `apply` changes three host accounts on one connection; a `1396` for a
    // host that does not exist must not abort the batch, any other error must.
    let server = live(|sql| {
        if sql.contains("ALTER USER") {
            fake::Reply::Err {
                code: 1396,
                message: "Operation ALTER USER failed; user doesn't exist",
            }
        } else {
            fake::Reply::Ok
        }
    });
    let admin = MysqlAdmin::new(live_target(&server));
    <MysqlAdmin as PasswordEngine>::apply(
        &admin,
        &SecretString::from("old-pw"),
        &SecretString::from("new-pw"),
    )
    .expect("a missing host account is skipped");
    // All three loopback hosts were attempted.
    assert!(
        server.saw("ALTER USER `root`@'localhost'"),
        "{:?}",
        server.statements()
    );
    assert!(server.saw("ALTER USER `root`@'127.0.0.1'"));
    assert!(server.saw("ALTER USER `root`@'::1'"));

    let failing = live(|sql| {
        if sql.contains("ALTER USER") {
            fake::Reply::Err {
                code: 1044,
                message: "access denied",
            }
        } else {
            fake::Reply::Ok
        }
    });
    let admin = MysqlAdmin::new(live_target(&failing));
    let err = <MysqlAdmin as PasswordEngine>::apply(
        &admin,
        &SecretString::from("old-pw"),
        &SecretString::from("new-pw"),
    )
    .expect_err("a non-1396 error must abort");
    assert_eq!(err, AdminError::InsufficientPrivilege);
}

#[test]
fn verify_opens_a_fresh_connection_and_runs_select_1() {
    let server = live(|sql| {
        if sql.contains("SELECT 1") {
            fake::count(1)
        } else {
            fake::Reply::Ok
        }
    });
    let admin = MysqlAdmin::new(live_target(&server));
    <MysqlAdmin as PasswordEngine>::verify(&admin, &SecretString::from("pw")).expect("verify");
    assert!(server.saw("SELECT 1"), "{:?}", server.statements());
}

#[test]
fn a_server_error_maps_by_code_on_a_live_connection() {
    // The connection succeeds; the statement is refused with 1044, which maps to
    // `InsufficientPrivilege` rather than `Connect`.
    let server = live(|sql| {
        if sql.contains("CREATE DATABASE") {
            fake::Reply::Err {
                code: 1044,
                message: "access denied",
            }
        } else {
            fake::Reply::Ok
        }
    });
    let admin = MysqlAdmin::new(live_target(&server));
    let db = Identifier::new("appdb").expect("valid");
    assert_eq!(
        admin.create_database(&db).expect_err("denied"),
        AdminError::InsufficientPrivilege
    );
}

#[test]
fn a_refused_login_on_a_live_connection_is_auth_failed() {
    // The connect handshake itself is refused with 1045: `map_connect_error`
    // turns it into `AuthFailed`, not `Connect`.
    let server = live(|sql| {
        if sql == "@@connect" {
            fake::Reply::Err {
                code: 1045,
                message: "Access denied",
            }
        } else {
            fake::Reply::Ok
        }
    });
    let admin = MysqlAdmin::new(live_target(&server));
    assert_eq!(
        admin.list_databases().expect_err("login refused"),
        AdminError::AuthFailed
    );
}

#[test]
fn the_admin_executor_trait_delegates_to_the_same_methods() {
    use crate::admin::AdminExecutor;
    let server = live(|sql| {
        if sql.contains("COUNT(*)") {
            // `database_exists`, and the `drop_database` session count.
            fake::count(1)
        } else if sql.contains("SCHEMATA") {
            fake::Reply::Rows {
                columns: vec![fake::Col::text("SCHEMA_NAME"), fake::Col::int("size")],
                rows: vec![vec![Some("appdb".to_owned()), Some("0".to_owned())]],
            }
        } else if sql.contains("mysql.user") {
            fake::Reply::Rows {
                columns: vec![fake::Col::text("User"), fake::Col::text("Host")],
                rows: vec![vec![Some("root".to_owned()), Some("localhost".to_owned())]],
            }
        } else {
            fake::Reply::Ok
        }
    });
    let admin = MysqlAdmin::new(live_target(&server));
    let executor: &dyn AdminExecutor = &admin;
    let db = Identifier::new("appdb").expect("valid");
    let user = Identifier::role("app_user").expect("valid role");

    assert_eq!(executor.list_databases().expect("list").len(), 1);
    assert_eq!(executor.list_users().expect("users").len(), 1);
    executor.create_database(&db).expect("create");
    assert!(executor.database_exists(&db).expect("exists"));
    executor.create_user(&user).expect("create user");
    executor
        .grant(&user, &db, PrivilegeLevel::ReadOnly)
        .expect("grant");
    executor.drop_user(&user).expect("drop user");
    // A live session (count 1) blocks the default drop.
    assert_eq!(
        executor.drop_database(&db, false).expect_err("busy"),
        AdminError::DatabaseInUse
    );
    executor.drop_database(&db, true).expect("forced drop");
}
