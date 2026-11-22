//! W3-A — real-PostgreSQL proofs for database and user administration
//! (`F-DBU-01..03`), plus the hostile-input proof that nothing reaches the
//! server.
//!
//! Everything here is `#[ignore]`d: it needs the PostgreSQL archive cached in
//! `~/.cache/dblocal-dev` (the same one `service_e2e` uses) and starts a real
//! engine. Real-engine runs are serialized machine-wide (Common Rules 12): every
//! run must hold the shared lock, because the tests use ports from 20000 up and
//! several workers plus the reviewer run engines on this one machine. Run it
//! with
//!
//! ```sh
//! export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target"
//! DBLOCAL_E2E=1 flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" \
//!   cargo test -p dblocal-core --test admin_e2e -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Design notes and the recorded runs live in `docs/architecture/admin.md`.
//!
//! The cluster is a real `initdb` + `pg_ctl` on a port from 20000-29999 inside a
//! temp home; the guard kills every `postgres` whose executable lives in that
//! home even on panic. The admin code under test is the production
//! `dblocal_core::admin::PgAdmin`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(unsafe_code)]

#[path = "admin_support/mod.rs"]
mod admin_support;

use std::time::Duration;

use dblocal_core::admin::{
    AdminError, AdminTarget, CredentialSink, DatabaseInfo, Identifier, PgAdmin, PrivilegeLevel,
};
use dblocal_core::error::Result as DblResult;
use dblocal_core::secret::SecretString;
use tokio_postgres::{Client, NoTls};

use admin_support::cluster::{AdminCluster, SUPERUSER};
use admin_support::{TempHome, port};

const PASSWORD: &str = "superuser-canary-pw-2024";

/// Refuses to run without `DBLOCAL_E2E=1`.
fn require_e2e() {
    if std::env::var("DBLOCAL_E2E").ok().as_deref() != Some("1") {
        panic!(
            "admin_e2e needs `DBLOCAL_E2E=1`; run \
             `DBLOCAL_E2E=1 flock -w 3600 $HOME/.cache/dblocal-e2e.lock \
             cargo test -p dblocal-core --test admin_e2e -- --ignored`"
        );
    }
}

/// A tiny synchronous `tokio-postgres` session for assertions the admin code
/// does not expose (encoding, direct privilege checks, connection counts).
struct Sql {
    runtime: tokio::runtime::Runtime,
    client: Client,
}

impl Sql {
    fn connect(port: u16, user: &str, password: &str, dbname: &str) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("build runtime: {e}"))?;
        let mut cfg = tokio_postgres::Config::new();
        cfg.host("127.0.0.1")
            .port(port)
            .user(user)
            .password(password)
            .dbname(dbname)
            .connect_timeout(Duration::from_secs(5));
        let (client, connection) = runtime
            .block_on(cfg.connect(NoTls))
            .map_err(|e| format!("connect as {user} to {dbname}: {e}"))?;
        runtime.spawn(async move {
            let _ = connection.await;
        });
        Ok(Self { runtime, client })
    }

    fn text(&self, sql: &str) -> Result<String, String> {
        let row = self
            .runtime
            .block_on(self.client.query_one(sql, &[]))
            .map_err(|e| format!("query {sql:?}: {e}"))?;
        row.try_get::<_, String>(0)
            .map_err(|e| format!("column 0 of {sql:?}: {e}"))
    }

    fn count(&self, sql: &str) -> Result<i64, String> {
        let row = self
            .runtime
            .block_on(self.client.query_one(sql, &[]))
            .map_err(|e| format!("query {sql:?}: {e}"))?;
        row.try_get::<_, i64>(0)
            .map_err(|e| format!("column 0 of {sql:?}: {e}"))
    }

    fn batch(&self, sql: &str) -> Result<(), String> {
        self.runtime
            .block_on(self.client.batch_execute(sql))
            .map_err(|e| format!("batch {sql:?}: {e}"))
    }

    /// Whether the statement is allowed, without caring about the result rows.
    fn can(&self, sql: &str) -> bool {
        self.runtime
            .block_on(self.client.batch_execute(sql))
            .is_ok()
    }
}

/// The admin client and a raw SQL handle over one started cluster.
struct Fixture {
    /// Keeps the temp home (and its env redirection) alive for the test.
    _home: TempHome,
    cluster: AdminCluster,
    admin: PgAdmin,
}

impl Fixture {
    fn new(prefix: &str) -> Self {
        let home = TempHome::new(prefix);
        let p = port::reserve();
        let password = SecretString::from(PASSWORD);
        let cluster = AdminCluster::start(&home, p, &password);
        let target = AdminTarget::new("127.0.0.1", p, SUPERUSER, password.clone())
            .expect("loopback admin target");
        let admin = PgAdmin::new(target);
        Self {
            _home: home,
            cluster,
            admin,
        }
    }

    fn superuser(&self) -> Sql {
        Sql::connect(
            self.cluster.port(),
            SUPERUSER,
            PASSWORD,
            dblocal_core::consts::DEFAULT_DATABASE,
        )
        .expect("connect as the superuser")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.cluster.stop();
    }
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1"]
fn create_database_is_utf8_and_appears_with_a_size() {
    require_e2e();
    let fixture = Fixture::new("dblocal-admin-create-");
    let before = fixture.admin.list_databases().expect("list before");
    assert!(
        before.iter().all(|db| db.name() != "appdb"),
        "appdb must not exist yet"
    );

    let db = Identifier::new("appdb").expect("valid name");
    fixture.admin.create_database(&db).expect("create database");

    let after = fixture.admin.list_databases().expect("list after");
    let entry = find(&after, "appdb").expect("appdb must be listed");
    assert!(entry.size_bytes() > 0, "a database has a non-zero size");
    assert_eq!(entry.owner(), SUPERUSER);

    let sql = fixture.superuser();
    assert_eq!(
        sql.text("SELECT pg_encoding_to_char(encoding) FROM pg_database WHERE datname = 'appdb'"),
        Ok("UTF8".to_owned()),
        "the new database must be UTF8"
    );
    // Assert only that a size is reported, not its exact value (cluster-dependent).
    let size = sql
        .count("SELECT pg_database_size('appdb')")
        .expect("size query");
    assert!(size >= 0);
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1"]
fn read_only_user_can_select_but_not_insert() {
    require_e2e();
    let fixture = Fixture::new("dblocal-admin-readonly-");

    let db = Identifier::new("rodb").expect("valid");
    fixture.admin.create_database(&db).expect("create db");
    let pw = fixture
        .admin
        .create_user(&Identifier::new("ro_user").expect("valid"))
        .expect("create user");
    assert_eq!(pw.len(), dblocal_core::consts::PASSWORD_LEN);

    // A table the grants must cover, created before GRANT ... ON ALL TABLES.
    let admin_to_db =
        Sql::connect(fixture.cluster.port(), SUPERUSER, PASSWORD, "rodb").expect("connect to rodb");
    admin_to_db
        .batch("CREATE TABLE t (id integer PRIMARY KEY, note text NOT NULL); INSERT INTO t VALUES (1, 'x');")
        .expect("seed table");

    fixture
        .admin
        .grant(
            &Identifier::new("ro_user").expect("valid"),
            &db,
            PrivilegeLevel::ReadOnly,
        )
        .expect("grant read-only");

    let ro = Sql::connect(fixture.cluster.port(), "ro_user", pw.expose(), "rodb")
        .expect("read-only user connects");
    assert_eq!(ro.count("SELECT count(*) FROM t").expect("select"), 1);
    assert!(
        !ro.can("INSERT INTO t (id, note) VALUES (2, 'y')"),
        "a read-only user must not INSERT"
    );
    assert!(
        !ro.can("UPDATE t SET note = 'z' WHERE id = 1"),
        "a read-only user must not UPDATE"
    );
    assert!(
        !ro.can("DELETE FROM t WHERE id = 1"),
        "a read-only user must not DELETE"
    );
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1"]
fn read_write_user_can_insert_update_delete() {
    require_e2e();
    let fixture = Fixture::new("dblocal-admin-readwrite-");
    let user_id = Identifier::new("rw_user").expect("valid");
    let db = Identifier::new("rwdb").expect("valid");
    fixture.admin.create_database(&db).expect("create db");
    let pw = fixture.admin.create_user(&user_id).expect("create user");

    let admin_to_db =
        Sql::connect(fixture.cluster.port(), SUPERUSER, PASSWORD, "rwdb").expect("connect to rwdb");
    admin_to_db
        .batch("CREATE TABLE t (id integer PRIMARY KEY, note text NOT NULL); INSERT INTO t VALUES (1, 'x');")
        .expect("seed table");
    admin_to_db
        .batch("CREATE SEQUENCE s")
        .expect("seed sequence");

    fixture
        .admin
        .grant(&user_id, &db, PrivilegeLevel::ReadWrite)
        .expect("grant read-write");

    let rw = Sql::connect(fixture.cluster.port(), "rw_user", pw.expose(), "rwdb")
        .expect("read-write user connects");
    assert_eq!(rw.count("SELECT count(*) FROM t").expect("select"), 1);
    rw.batch("INSERT INTO t (id, note) VALUES (2, 'y')")
        .expect("insert");
    rw.batch("UPDATE t SET note = 'z' WHERE id = 2")
        .expect("update");
    rw.batch("DELETE FROM t WHERE id = 2").expect("delete");
    let _ = rw.count("SELECT nextval('s')").expect("sequence usable");
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1"]
fn drop_refuses_when_a_connection_exists_and_succeeds_after_it_closes() {
    require_e2e();
    let fixture = Fixture::new("dblocal-admin-drop-");
    let db = Identifier::new("dropdb").expect("valid");
    fixture.admin.create_database(&db).expect("create db");

    // Hold a connection open.
    let held = Sql::connect(fixture.cluster.port(), SUPERUSER, PASSWORD, "dropdb")
        .expect("hold a session");
    let err = fixture
        .admin
        .drop_database(&db, false)
        .expect_err("an open connection must block the drop");
    assert_eq!(err, AdminError::DatabaseInUse);
    // The database is still there.
    let sql = fixture.superuser();
    assert_eq!(
        sql.count("SELECT count(*) FROM pg_database WHERE datname = 'dropdb'"),
        Ok(1)
    );

    drop(held);
    // Wait until the backend exits, then the drop works without FORCE.
    wait_connections(fixture.cluster.port(), "dropdb", 0, Duration::from_secs(10));
    fixture
        .admin
        .drop_database(&db, false)
        .expect("drop after close");
    assert_eq!(
        sql.count("SELECT count(*) FROM pg_database WHERE datname = 'dropdb'"),
        Ok(0)
    );
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1"]
fn hostile_names_are_rejected_before_any_statement_reaches_the_server() {
    require_e2e();
    let fixture = Fixture::new("dblocal-admin-hostile-");
    let sql = fixture.superuser();

    // A canary the would-be injection tries to drop.
    sql.batch("CREATE DATABASE canarydb")
        .expect("create the canary database");
    let databases_before = sql
        .count("SELECT count(*) FROM pg_database")
        .expect("count databases");

    let hostile = [
        "app'; DROP DATABASE canarydb; --",
        "app\"; DROP DATABASE canarydb; --",
        "app`; DROP DATABASE canarydb; --",
        "app\\",
        "app\0x",
        "café",
        "名前",
        "1leading",
        "-dash",
        "--comment",
        "a b",
        "a;b",
    ];
    for raw in hostile {
        let err = Identifier::new(raw)
            .err()
            .unwrap_or_else(|| panic!("validation accepted {raw:?}"));
        assert!(
            matches!(err, AdminError::InvalidIdentifier(_)),
            "{raw:?}: {err:?}"
        );
    }
    // A reserved database name is refused too.
    assert!(Identifier::new("template0").is_err());

    // Nothing changed: validation runs entirely in-process, so the canary
    // database count is unchanged and the canary still exists.
    assert_eq!(
        sql.count("SELECT count(*) FROM pg_database"),
        Ok(databases_before),
        "validating hostile names must not create or drop anything"
    );
    assert_eq!(
        sql.count("SELECT count(*) FROM pg_database WHERE datname = 'canarydb'"),
        Ok(1),
        "the canary database must survive every hostile name"
    );
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1"]
fn reset_password_succeeds_and_the_new_password_works() {
    require_e2e();
    let fixture = Fixture::new("dblocal-admin-reset-");
    struct RecordingSink {
        seen: std::sync::Mutex<Option<String>>,
    }
    impl CredentialSink for RecordingSink {
        fn store(&self, password: &SecretString) -> DblResult<()> {
            *self.seen.lock().expect("lock") = Some(password.expose().to_owned());
            Ok(())
        }
    }
    let sink = RecordingSink {
        seen: std::sync::Mutex::new(None),
    };
    let new_password = fixture
        .admin
        .reset_superuser_password(&sink)
        .expect("reset must succeed");
    assert_eq!(new_password.len(), dblocal_core::consts::PASSWORD_LEN);
    assert_eq!(
        sink.seen.lock().expect("lock").as_deref(),
        Some(new_password.expose())
    );

    // The old password no longer works; the new one does.
    assert!(
        Sql::connect(fixture.cluster.port(), SUPERUSER, PASSWORD, "postgres").is_err(),
        "the old password must stop working"
    );
    let ok = Sql::connect(
        fixture.cluster.port(),
        SUPERUSER,
        new_password.expose(),
        "postgres",
    )
    .expect("the new password must authenticate");
    assert_eq!(ok.count("SELECT 1::bigint"), Ok(1));
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1"]
fn reset_password_rolls_back_when_the_sink_fails() {
    require_e2e();
    let fixture = Fixture::new("dblocal-admin-reset-fail-");
    struct FailingSink;
    impl CredentialSink for FailingSink {
        fn store(&self, _password: &SecretString) -> DblResult<()> {
            Err(dblocal_core::DblError::Secret {
                reason: "keychain locked".to_owned(),
            })
        }
    }
    let err = fixture
        .admin
        .reset_superuser_password(&FailingSink)
        .expect_err("a failing sink must fail the reset");
    assert_eq!(err, AdminError::CredentialStore);

    // F-DBU-03 AC2: the OLD password still works, because the engine was rolled
    // back. This is the whole point of the rollback guarantee.
    let old = Sql::connect(fixture.cluster.port(), SUPERUSER, PASSWORD, "postgres")
        .expect("the old password must still work after a rolled-back reset");
    assert_eq!(old.count("SELECT 1::bigint"), Ok(1));
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1"]
fn list_users_exists_revoke_drop_and_duplicate_and_not_found_paths() {
    require_e2e();
    let fixture = Fixture::new("dblocal-admin-users-");

    // A user shows up in list_users with the right flags.
    let user = Identifier::new("crud_user").expect("valid");
    let _pw = fixture.admin.create_user(&user).expect("create user");
    let users = fixture.admin.list_users().expect("list users");
    let found = users
        .iter()
        .find(|u| u.name() == "crud_user")
        .expect("the new user must be listed");
    assert!(found.can_login());
    assert!(!found.is_superuser());
    assert!(fixture.admin.user_exists(&user).expect("exists"));
    assert!(
        !fixture
            .admin
            .user_exists(&Identifier::new("ghost_user").expect("valid"))
            .expect("exists ghost")
    );

    // Creating the same user twice is a typed UserExists (SQLSTATE 42710).
    let err = fixture
        .admin
        .create_user(&user)
        .expect_err("a duplicate role must fail");
    assert_eq!(err, AdminError::UserExists);

    // Grant then revoke on a database.
    let db = Identifier::new("cruddb").expect("valid");
    fixture.admin.create_database(&db).expect("create db");
    fixture
        .admin
        .grant(&user, &db, PrivilegeLevel::ReadOnly)
        .expect("grant");
    fixture.admin.revoke(&user, &db).expect("revoke");

    // Dropping a database that does not exist is DatabaseNotFound (3D000).
    fixture.admin.drop_database(&db, false).expect("drop db");
    let err = fixture
        .admin
        .drop_database(&db, false)
        .expect_err("dropping a missing database must fail");
    assert_eq!(err, AdminError::DatabaseNotFound);

    // Dropping the role, then dropping it again: 42704 maps to UserNotFound (F4).
    fixture.admin.drop_user(&user).expect("drop user");
    assert!(!fixture.admin.user_exists(&user).expect("exists after drop"));
    let err = fixture
        .admin
        .drop_user(&user)
        .expect_err("dropping a missing user must fail");
    assert_eq!(err, AdminError::UserNotFound);

    // Revoke/grant on a missing role are also UserNotFound (F4).
    let ghost = Identifier::new("ghost_role").expect("valid");
    let db2 = Identifier::new("cruddb2").expect("valid");
    fixture.admin.create_database(&db2).expect("create db2");
    let err = fixture
        .admin
        .revoke(&ghost, &db2)
        .expect_err("revoke missing user");
    assert_eq!(err, AdminError::UserNotFound);
    let err = fixture
        .admin
        .grant(&ghost, &db2, PrivilegeLevel::ReadOnly)
        .expect_err("grant missing user");
    assert_eq!(err, AdminError::UserNotFound);

    // A target pointing at a closed port is a typed Connect error, not a panic.
    let closed = AdminTarget::new(
        "127.0.0.1",
        port::reserve(),
        SUPERUSER,
        SecretString::from(PASSWORD),
    )
    .expect("target");
    let unreachable = PgAdmin::new(closed);
    assert_eq!(
        unreachable.list_databases().err(),
        Some(AdminError::Connect)
    );
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn find<'a>(dbs: &'a [DatabaseInfo], name: &str) -> Option<&'a DatabaseInfo> {
    dbs.iter().find(|db| db.name() == name)
}

/// Polls `pg_stat_activity` until `db` has `want` connections from other
/// backends, or the timeout elapses.
fn wait_connections(port: u16, db: &str, want: i64, timeout: Duration) {
    let Ok(sql) = Sql::connect(port, SUPERUSER, PASSWORD, "postgres") else {
        return;
    };
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let sql_text = format!(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = '{}' AND pid <> pg_backend_pid()",
            db
        );
        if sql.count(&sql_text).is_ok_and(|count| count <= want) {
            return;
        }
        if std::time::Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ---------------------------------------------------------------------------
// W3-A fix 1 additions: F1, F2, F3, F5, F6.
// ---------------------------------------------------------------------------

fn read_log(cluster: &AdminCluster) -> String {
    std::fs::read_to_string(cluster.log_path()).unwrap_or_default()
}

fn assert_log_has_no_canary(cluster: &AdminCluster, canaries: &[&str]) {
    let log = read_log(cluster);
    for canary in canaries {
        assert!(
            !log.contains(canary),
            "engine log leaked canary {canary:?}; log tail: {}",
            &log[log.len().saturating_sub(2000)..]
        );
    }
    assert!(
        !log.contains("PASSWORD E'"),
        "engine log contains a cleartext PASSWORD literal; log tail: {}",
        &log[log.len().saturating_sub(2000)..]
    );
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1"]
fn f1_password_never_reaches_engine_log_on_failures() {
    require_e2e();
    // Allow server-side logging to be observable: SCRAM must be the reason the
    // canary is absent, not connection-level suppression. (Spec F1 mutation note.)
    unsafe { std::env::set_var("DBLOCAL_ADMIN_SUPPRESS_LOG_OPTIONS", "0"); }
    let fixture = Fixture::new("dblocal-admin-f1-");
    fixture
        .cluster
        .append_conf_and_reload("log_min_error_statement = 'error'\n");
    std::thread::sleep(Duration::from_millis(200));
    let canary_pw = "CanaryPw_F1_9f3a!do-not-log";
    let canary = SecretString::from(canary_pw);
    let user = Identifier::new("leaky_role").expect("valid");

    // (a) create_user with known password, then duplicate create fails.
    fixture
        .admin
        .create_user_with_password(&user, &canary)
        .expect("create user with known password");
    let err = fixture
        .admin
        .create_user_with_password(&user, &canary)
        .expect_err("duplicate must fail");
    assert_eq!(err, AdminError::UserExists);

    // (b) failing ALTER on a missing role through our admin seam.
    let missing = Identifier::new("missing_role").expect("valid");
    let err = fixture
        .admin
        .alter_user_password(&missing, &canary)
        .expect_err("alter missing role must fail");
    assert_eq!(err, AdminError::UserNotFound);

    // (c) sink-failure rollback path: reset password fails to persist, rolls back.
    struct FailingSink;
    impl CredentialSink for FailingSink {
        fn store(&self, _password: &SecretString) -> DblResult<()> {
            Err(dblocal_core::DblError::Secret {
                reason: "keychain locked".to_owned(),
            })
        }
    }
    let err = fixture
        .admin
        .reset_superuser_password(&FailingSink)
        .expect_err("sink failure must fail reset");
    assert_eq!(err, AdminError::CredentialStore);

    // Give the server a moment to flush logs.
    std::thread::sleep(Duration::from_millis(200));
    assert_log_has_no_canary(&fixture.cluster, &[canary_pw, PASSWORD]);

    // The created role can still authenticate with the cleartext password.
    let ok = Sql::connect(
        fixture.cluster.port(),
        "leaky_role",
        canary_pw,
        "postgres",
    )
    .expect("role must authenticate with its cleartext password");
    assert_eq!(ok.count("SELECT 1::bigint"), Ok(1));

    // Old superuser password still works after rolled-back reset.
    let su = Sql::connect(fixture.cluster.port(), SUPERUSER, PASSWORD, "postgres")
        .expect("superuser password must survive rollback");
    assert_eq!(su.count("SELECT 1::bigint"), Ok(1));
    unsafe { std::env::remove_var("DBLOCAL_ADMIN_SUPPRESS_LOG_OPTIONS"); }
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1"]
fn f2_wrong_password_is_auth_failed_and_closed_port_is_connect() {
    require_e2e();
    let fixture = Fixture::new("dblocal-admin-f2-");

    let wrong = AdminTarget::new(
        "127.0.0.1",
        fixture.cluster.port(),
        SUPERUSER,
        SecretString::from("wrong-password-canary"),
    )
    .expect("target");
    let err = PgAdmin::new(wrong)
        .list_databases()
        .expect_err("wrong password must fail");
    assert_eq!(err, AdminError::AuthFailed);

    let closed = AdminTarget::new(
        "127.0.0.1",
        port::reserve(),
        SUPERUSER,
        SecretString::from(PASSWORD),
    )
    .expect("target");
    let err = PgAdmin::new(closed)
        .list_databases()
        .expect_err("closed port must fail");
    assert_eq!(err, AdminError::Connect);

    assert_ne!(
        AdminError::AuthFailed.user_message(),
        AdminError::Connect.user_message()
    );
    for e in [AdminError::AuthFailed, AdminError::Connect] {
        let surfaces = format!("{e:?} {e} {}", e.user_message());
        assert!(!surfaces.contains("wrong-password-canary"));
        assert!(!surfaces.contains(PASSWORD));
    }
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1"]
fn f5_force_drop_kills_live_session_and_false_refuses() {
    require_e2e();
    let fixture = Fixture::new("dblocal-admin-f5-");
    let db = Identifier::new("forcedb").expect("valid");
    fixture.admin.create_database(&db).expect("create db");

    let _held = Sql::connect(fixture.cluster.port(), SUPERUSER, PASSWORD, "forcedb")
        .expect("hold a session");
    let err = fixture
        .admin
        .drop_database(&db, false)
        .expect_err("force=false must refuse");
    assert_eq!(err, AdminError::DatabaseInUse);
    let sql = fixture.superuser();
    assert_eq!(
        sql.count("SELECT count(*) FROM pg_database WHERE datname = 'forcedb'"),
        Ok(1),
        "database must still exist after DatabaseInUse"
    );

    fixture
        .admin
        .drop_database(&db, true)
        .expect("force=true must drop");
    assert_eq!(
        sql.count("SELECT count(*) FROM pg_database WHERE datname = 'forcedb'"),
        Ok(0)
    );
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1"]
fn f6_hostile_names_never_reach_server_statement_log() {
    require_e2e();
    let fixture = Fixture::new("dblocal-admin-f6-");
    // Enable full statement logging; our admin connections also suppress logging,
    // so we count statements another way too (pg_stat_database xact counters).
    fixture
        .cluster
        .append_conf_and_reload("log_statement = 'all'\n");
    std::thread::sleep(Duration::from_millis(300));
    let marker = "F6_BOUNDARY_MARKER";
    let marker_sql = fixture.cluster.psql(
        &SecretString::from(PASSWORD),
        &format!("SELECT '{marker}'"),
    );
    assert!(marker_sql.status.success());

    let hostile = [
        "app'; DROP DATABASE postgres; --",
        "app\"; DROP DATABASE postgres; --",
        "app`; DROP DATABASE postgres; --",
        "app\\",
        "app\0x",
        "café",
        "名前",
        "1leading",
        "-dash",
        "--comment",
        "a b",
        "a;b",
    ];
    let valid_db = Identifier::new("f6db").expect("valid");
    fixture.admin.create_database(&valid_db).expect("create f6db");

    let sql = fixture.superuser();
    let xact_before: i64 = sql
        .count("SELECT (xact_commit + xact_rollback)::bigint FROM pg_stat_database WHERE datname = 'postgres'")
        .expect("xact before");

    for raw in hostile {
        // If validation were weakened, construction succeeds and the call reaches
        // the server - the catalog/log assertions below catch it.
        if let Ok(id) = Identifier::new(raw) {
            let _ = fixture.admin.create_database(&id);
            let _ = fixture.admin.drop_database(&id, false);
        }
        if let Ok(role) = Identifier::role(raw) {
            let _ = fixture.admin.drop_user(&role);
        }
        // Every public method that takes a name: the hostile corpus must be
        // rejected by the typed constructor, so no call can be made.
        assert!(
            Identifier::new(raw).is_err(),
            "hostile db name {raw:?} must be rejected"
        );
        // role constructor allows "postgres" but hostile corpus has no reserved role.
        assert!(
            Identifier::role(raw).is_err(),
            "hostile role name {raw:?} must be rejected"
        );
    }

    // Exercise list methods (no name input) to prove cluster is still healthy.
    let _ = fixture.admin.list_databases().expect("list");
    let _ = fixture.admin.list_users().expect("list users");

    std::thread::sleep(Duration::from_millis(300));
    let log = read_log(&fixture.cluster);
    let after_marker = log.split(marker).nth(1).unwrap_or(&log);
    for raw in hostile {
        assert!(
            !after_marker.contains(raw),
            "hostile fragment {raw:?} reached server log"
        );
    }
    let lower = after_marker.to_ascii_lowercase();
    for forbidden in [
        "create role",
        "drop role",
        "grant ",
        "alter role",
        "drop database \"app",
        "create database \"app",
    ] {
        assert!(
            !lower.contains(forbidden),
            "unexpected statement containing {forbidden:?} in log after marker"
        );
    }

    // Catalog: no hostile-named database or role was created.
    for raw in hostile {
        let db_count = sql
            .count(&format!(
                "SELECT count(*)::bigint FROM pg_database WHERE datname = '{}'",
                raw.replace('\'', "''")
            ))
            .unwrap_or(0);
        assert_eq!(db_count, 0, "hostile db {raw:?} appeared in catalog");
        let role_count = sql
            .count(&format!(
                "SELECT count(*)::bigint FROM pg_roles WHERE rolname = '{}'",
                raw.replace('\'', "''")
            ))
            .unwrap_or(0);
        assert_eq!(role_count, 0, "hostile role {raw:?} appeared in catalog");
    }
    assert_eq!(
        sql.count("SELECT count(*) FROM pg_database WHERE datname = 'f6db'"),
        Ok(1)
    );
    // No meaningful transaction spike from hostile attempts (allow small delta for
    // our own list queries).
    let xact_after: i64 = sql
        .count("SELECT (xact_commit + xact_rollback)::bigint FROM pg_stat_database WHERE datname = 'postgres'")
        .expect("xact after");
    assert!(
        xact_after - xact_before < 40,
        "unexpected transaction spike: before={xact_before} after={xact_after}"
    );
}
