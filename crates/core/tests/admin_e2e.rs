//! W3-A — real-PostgreSQL proofs for database and user administration
//! (`F-DBU-01..03`), plus the hostile-input proof that nothing reaches the
//! server.
//!
//! Everything here is `#[ignore]`d: it needs the PostgreSQL archive cached in
//! `~/.cache/dblocal-dev` (the same one `service_e2e` uses) and starts a real
//! engine. Run it with
//!
//! ```sh
//! export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target"
//! DBLOCAL_E2E=1 cargo test -p dblocal-core --test admin_e2e -- --ignored --test-threads=1 --nocapture
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
             `DBLOCAL_E2E=1 cargo test -p dblocal-core --test admin_e2e -- --ignored`"
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
