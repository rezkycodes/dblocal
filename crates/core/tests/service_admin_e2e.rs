//! W4-A real-engine proofs through the public `DblService` surface.
//!
//! Every test is `#[ignore]`d: it starts a real PostgreSQL and needs a cached
//! archive. The engine is started by [`admin_support::cluster::AdminCluster`]
//! (the same `initdb`/`pg_ctl` the production adapter builds); the admin calls
//! under test go through **`LocalService`**, which reads the credential pointer
//! from the store, dials loopback through [`PgAdmin`](dblocal_core::admin::PgAdmin)
//! and runs the statements. So the assertions exercise the wiring, not just the
//! admin library.
//!
//! Run it under the machine-wide e2e lock (Common Rules 12):
//!
//! ```sh
//! export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target"
//! DBLOCAL_E2E=1 flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" \
//!   cargo test -p dblocal-core --test service_admin_e2e -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Everything stays inside a temp home (`HOME`/`DBLOCAL_HOME`), on a port in
//! 20000-29999, and the guard kills any `postgres` whose executable lives in the
//! temp tree even on panic.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(unsafe_code)]

#[path = "admin_support/mod.rs"]
mod admin_support;

use std::sync::Arc;

use dblocal_core::admin::Identifier;
use dblocal_core::error::Result as DblResult;
use dblocal_core::model::{
    AuthMode, CredentialBackend, CredentialRef, EngineKey, Instance, InstanceId, InstanceName,
    InstanceSpec, InstanceStatus, Port,
};
use dblocal_core::paths::DblPaths;
use dblocal_core::secret::SecretString;
use dblocal_core::secrets::{BackendKind, FileBackend, SecretBackend};
use dblocal_core::service::{
    DblService, LocalService, PrivilegeLevel, SecretAccess, ServiceOptions, UriFormat,
};
use dblocal_core::store::Store;
use tokio_postgres::{Client, NoTls};

use admin_support::cluster::{AdminCluster, SUPERUSER};
use admin_support::{TempHome, port};

const PASSWORD: &str = "service-admin-canary-pw-2024";

/// Refuses to run without `DBLOCAL_E2E=1`.
fn require_e2e() {
    if std::env::var("DBLOCAL_E2E").ok().as_deref() != Some("1") {
        panic!(
            "service_admin_e2e needs `DBLOCAL_E2E=1`; run \
             `DBLOCAL_E2E=1 flock -w 3600 $HOME/.cache/dblocal-e2e.lock \
             cargo test -p dblocal-core --test service_admin_e2e -- --ignored`"
        );
    }
}

/// The production `0600` file backend, without the keychain attempt.
///
/// Mirrors `tests/support/harness.rs::FileOnlySecrets`; a separate copy so this
/// task never edits a file another worker owns. It can also be told to fail a
/// write, which is how the F-DBU-03 AC2 rollback is driven through the service.
struct FileSecrets {
    file: FileBackend,
    fail_writes: std::sync::atomic::AtomicBool,
}

impl FileSecrets {
    fn new(paths: &DblPaths) -> Self {
        Self {
            file: FileBackend::from_paths(paths),
            fail_writes: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn fail_writes(&self) {
        self.fail_writes
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl SecretAccess for FileSecrets {
    fn set(&self, reference: &CredentialRef, secret: &SecretString) -> DblResult<BackendKind> {
        if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(dblocal_core::DblError::Secret {
                reason: "injected keychain failure".to_owned(),
            });
        }
        self.file.set(reference, secret)?;
        Ok(CredentialBackend::File)
    }

    fn get(&self, reference: &CredentialRef) -> DblResult<SecretString> {
        self.file.get(reference)
    }

    fn delete(&self, reference: &CredentialRef) -> DblResult<()> {
        match self.file.delete(reference) {
            Ok(()) => Ok(()),
            Err(dblocal_core::DblError::NotFound { .. }) => Ok(()),
            Err(err) => Err(err),
        }
    }
}

/// A temp home, a real cluster and a `LocalService` whose store points at it.
struct Fixture {
    _home: TempHome,
    paths: DblPaths,
    cluster: AdminCluster,
    service: LocalService,
    secrets: Arc<FileSecrets>,
    id: InstanceId,
}

impl Fixture {
    fn new(prefix: &str) -> Self {
        let home = TempHome::new(prefix);
        let paths = home.paths();
        let p = port::reserve();
        let password = SecretString::from(PASSWORD);
        let cluster = AdminCluster::start(&home, p, &password);

        // A running instance row pointing at the real cluster, with its real
        // superuser credential stored through the file backend.
        let spec = InstanceSpec::new(
            InstanceName::new("pg-admin").expect("name"),
            EngineKey::Postgresql,
            "18.7.0",
            Some(Port::new(p).expect("port")),
            AuthMode::Password,
        )
        .expect("spec");
        let id = InstanceId::new();
        let instance = Instance::new(
            &spec,
            id.clone(),
            InstanceStatus::Running,
            std::time::SystemTime::now(),
        );
        let reference =
            dblocal_core::secrets::credential_ref_for(&id, CredentialBackend::File, SUPERUSER);
        Store::open(&paths)
            .expect("store")
            .insert_instance_with_credential(&instance, &reference)
            .expect("seed instance row");

        let secrets = Arc::new(FileSecrets::new(&paths));
        // Seed the credential through the same backend the service will read.
        secrets.set(&reference, &password).expect("seed credential");

        let options = ServiceOptions {
            secrets: Some(Arc::clone(&secrets) as Arc<dyn SecretAccess>),
            // No reconciliation or supervisor: the engine is ours, not a spawn.
            auto_start: Some(false),
            ..ServiceOptions::default()
        };
        let service = LocalService::new(paths.clone(), options).expect("service");

        Self {
            _home: home,
            paths,
            cluster,
            service,
            secrets,
            id,
        }
    }

    /// The stored credential pointer.
    fn pointer(&self) -> CredentialRef {
        Store::open(&self.paths)
            .expect("store")
            .get_credential(&self.id)
            .expect("read")
            .expect("pointer")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.cluster.stop();
    }
}

/// A tiny synchronous `tokio-postgres` session for assertions the service does
/// not expose (encoding, privilege checks).
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
            .connect_timeout(std::time::Duration::from_secs(5));
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

    fn can(&self, sql: &str) -> bool {
        self.runtime
            .block_on(self.client.batch_execute(sql))
            .is_ok()
    }
}

fn superuser(fixture: &Fixture) -> Sql {
    Sql::connect(
        fixture.cluster.port(),
        SUPERUSER,
        PASSWORD,
        dblocal_core::consts::DEFAULT_DATABASE,
    )
    .expect("connect as the superuser")
}

fn find<'a>(
    databases: &'a [dblocal_core::service::DatabaseInfo],
    name: &str,
) -> Option<&'a dblocal_core::service::DatabaseInfo> {
    databases.iter().find(|db| db.name() == name)
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn create_list_and_drop_database_through_the_service() {
    require_e2e();
    let fixture = Fixture::new("dblocal-w4a-create-");

    // Create: UTF-8, and it appears with a non-zero size.
    fixture
        .service
        .create_database(&fixture.id, "appdb")
        .expect("create database");
    let databases = fixture.service.list_databases(&fixture.id).expect("list");
    let entry = find(&databases, "appdb").expect("appdb must be listed");
    assert!(entry.size_bytes() > 0, "a database has a non-zero size");
    assert_eq!(entry.owner(), SUPERUSER);

    let sql = superuser(&fixture);
    assert_eq!(
        sql.text("SELECT pg_encoding_to_char(encoding) FROM pg_database WHERE datname = 'appdb'")
            .expect("encoding query"),
        "UTF8",
        "the new database must be UTF8"
    );

    // Drop without force refuses while a session is connected.
    let held = Sql::connect(fixture.cluster.port(), SUPERUSER, PASSWORD, "appdb").expect("connect");
    let err = fixture
        .service
        .drop_database(&fixture.id, "appdb", false)
        .expect_err("a live connection must block the drop");
    assert_eq!(err.code, dblocal_core::admin::error::CODE_DATABASE_IN_USE);
    assert!(
        find(
            &fixture.service.list_databases(&fixture.id).expect("list"),
            "appdb"
        )
        .is_some(),
        "a refused drop must not remove the database"
    );
    drop(held);

    // With no session, the drop succeeds.
    fixture
        .service
        .drop_database(&fixture.id, "appdb", false)
        .expect("drop after the session closed");
    assert!(
        find(
            &fixture.service.list_databases(&fixture.id).expect("list"),
            "appdb"
        )
        .is_none(),
        "the dropped database must be gone"
    );
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn read_only_user_can_select_but_not_insert_through_the_service() {
    require_e2e();
    let fixture = Fixture::new("dblocal-w4a-ro-");

    fixture
        .service
        .create_database(&fixture.id, "rodb")
        .expect("create db");
    // A table the grants must cover, created before GRANT ... ON ALL TABLES.
    Sql::connect(fixture.cluster.port(), SUPERUSER, PASSWORD, "rodb")
        .expect("connect to rodb")
        .batch("CREATE TABLE t (id integer PRIMARY KEY, note text NOT NULL); INSERT INTO t VALUES (1, 'x');")
        .expect("seed table");

    let password = fixture
        .service
        .create_user(&fixture.id, "ro_user", "rodb", PrivilegeLevel::ReadOnly)
        .expect("create read-only user");
    assert_eq!(password.len(), dblocal_core::consts::PASSWORD_LEN);

    let ro = Sql::connect(fixture.cluster.port(), "ro_user", password.expose(), "rodb")
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
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn read_write_user_can_insert_through_the_service() {
    require_e2e();
    let fixture = Fixture::new("dblocal-w4a-rw-");

    fixture
        .service
        .create_database(&fixture.id, "rwdb")
        .expect("create db");
    Sql::connect(fixture.cluster.port(), SUPERUSER, PASSWORD, "rwdb")
        .expect("connect to rwdb")
        .batch("CREATE TABLE t (id integer PRIMARY KEY, note text NOT NULL);")
        .expect("seed table");

    let password = fixture
        .service
        .create_user(&fixture.id, "rw_user", "rwdb", PrivilegeLevel::ReadWrite)
        .expect("create read-write user");

    let rw = Sql::connect(fixture.cluster.port(), "rw_user", password.expose(), "rwdb")
        .expect("read-write user connects");
    rw.batch("INSERT INTO t (id, note) VALUES (1, 'x')")
        .expect("a read-write user can INSERT");
    rw.batch("UPDATE t SET note = 'y' WHERE id = 1")
        .expect("a read-write user can UPDATE");
    rw.batch("DELETE FROM t WHERE id = 1")
        .expect("a read-write user can DELETE");
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn password_reset_updates_engine_credential_and_uri() {
    require_e2e();
    let fixture = Fixture::new("dblocal-w4a-reset-");

    let new_password = fixture
        .service
        .reset_superuser_password(&fixture.id)
        .expect("reset");
    assert_ne!(new_password.expose(), PASSWORD);

    // The engine holds the new password: a fresh connection with it works, and
    // the old one no longer does.
    Sql::connect(
        fixture.cluster.port(),
        SUPERUSER,
        new_password.expose(),
        dblocal_core::consts::DEFAULT_DATABASE,
    )
    .expect("the new password authenticates");
    assert!(
        Sql::connect(
            fixture.cluster.port(),
            SUPERUSER,
            PASSWORD,
            dblocal_core::consts::DEFAULT_DATABASE
        )
        .is_err(),
        "the old password must stop working"
    );

    // The stored credential (the "keychain" half, here the file fallback) and the
    // printed URI both carry the new value.
    let stored = fixture.secrets.get(&fixture.pointer()).expect("stored");
    assert_eq!(stored.expose(), new_password.expose());
    let uri = fixture
        .service
        .connection_uri(&fixture.id, UriFormat::Uri)
        .expect("uri");
    assert!(
        uri.expose().contains(new_password.expose()),
        "the URI must carry the new password"
    );
}

#[test]
#[ignore = "e2e: real PostgreSQL; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn a_failed_reset_rolls_back_and_leaves_the_old_password_working() {
    require_e2e();
    let fixture = Fixture::new("dblocal-w4a-failsink-");

    let before = fixture.pointer();
    // The sink refuses every write: the engine applies the new password and
    // proves it, then the store fails, which must roll back to the old one.
    fixture.secrets.fail_writes();

    let err = fixture
        .service
        .reset_superuser_password(&fixture.id)
        .expect_err("the sink refusal must surface");
    assert_eq!(err.code, dblocal_core::admin::error::CODE_CREDENTIAL_STORE);

    // F-DBU-03 AC2: the old password still works.
    Sql::connect(
        fixture.cluster.port(),
        SUPERUSER,
        PASSWORD,
        dblocal_core::consts::DEFAULT_DATABASE,
    )
    .expect("the old password must still authenticate after a failed reset");

    // The credential pointer was not touched.
    assert_eq!(before, fixture.pointer(), "the pointer must not move");
    let stored = fixture.secrets.get(&fixture.pointer()).expect("stored");
    assert_eq!(
        stored.expose(),
        PASSWORD,
        "the stored credential must still be the old password"
    );
}

/// Unused-but-named so the `Identifier`/`Instance` imports stay meaningful to a
/// reader scanning this file's contract.
#[allow(dead_code)]
fn _uses(_: Identifier, _: Instance, _: InstanceName) {}
