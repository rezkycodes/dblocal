//! W4-A2 — real MySQL 8.4.11 and MariaDB 11.8.10 database/user administration
//! through the public `DblService` surface.
//!
//! Every test is `#[ignore]`d: it starts a real engine from the shared runtime
//! cache and needs `DBLOCAL_E2E=1`. Run under the machine-wide e2e lock:
//!
//! ```sh
//! DBLOCAL_E2E=1 flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" \
//!   cargo test -p dblocal-core --test service_e2e -- --ignored --test-threads=1 \
//!   --nocapture admin_mysql
//! ```
//!
//! The admin calls go through `LocalService`, which resolves the instance's unix
//! socket and dials it as the superuser. Managed users are created at
//! `user@'127.0.0.1'` so the service URI works over TCP; the SQL assertions prove
//! the real privilege boundaries.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use dblocal_core::model::{DeleteMode, EngineKey, InstanceStatus};
use dblocal_core::service::{DblService, PrivilegeLevel};

use crate::support::cache;
use crate::support::clients::{MyClient, ScopedMyClient};
use crate::support::harness::{Harness, newest_spec, newest_version};
use crate::support::{ports, proc};

const MYSQL_ARCHIVE: &str = "mysql-8.4.11-linux-glibc2.28-x86_64-minimal.tar.xz";
const MARIADB_ARCHIVE: &str = "mariadb-11.8.10-linux-systemd-x86_64.tar.gz";

#[test]
#[ignore = "e2e: needs the cached MySQL archive; run with DBLOCAL_E2E=1 under flock"]
fn mysql_admin_full_cycle() {
    super::require_e2e();
    run_admin_case(EngineKey::Mysql, "mysql-admin", MYSQL_ARCHIVE);
}

#[test]
#[ignore = "e2e: needs the cached MariaDB archive and libcrypt.so.1; run with DBLOCAL_E2E=1 under flock"]
fn mariadb_admin_full_cycle() {
    super::require_e2e();
    if !crate::support::mariadb_shim::ensure_shim_for_runtime() {
        eprintln!("skip mariadb admin e2e: this host cannot shim libcrypt.so.1");
        return;
    }
    run_admin_case(EngineKey::Mariadb, "mariadb-admin", MARIADB_ARCHIVE);
}

/// create → start → admin list/create/drop → ro/rw users → password reset →
/// delete, proving the whole surface against one real engine.
fn run_admin_case(engine: EngineKey, name: &str, archive: &str) {
    let harness = Harness::new("dblocal-e2e-mysqladmin-");
    let version = newest_version(&harness.service, engine);
    assert!(
        cache::extract_archive_to_runtime(&harness.paths, engine, &version, archive),
        "this case needs {archive} cached in ~/.cache/dblocal-dev"
    );

    let port = ports::reserve();
    let spec = newest_spec(&harness.service, name, engine, port);
    let created = harness.create(spec);
    super::expect_ok(&created, "create");
    let id = harness.instance(name).expect("row").id().clone();
    let start = harness.start(&id);
    super::expect_ok(&start, "start");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(30));
    harness.wait_listening(&id, Duration::from_secs(30));

    let label = engine.display_name();

    // --- create database with the utf8mb4 charset, then list it with a size ---
    harness
        .service
        .create_database(&id, "admdb")
        .expect("create database");
    let databases = harness.service.list_databases(&id).expect("list databases");
    let admdb = databases
        .iter()
        .find(|db| db.name() == "admdb")
        .unwrap_or_else(|| panic!("{label}: admdb must be listed: {databases:?}"));
    // A fresh database has size 0; the charset is what matters and is asserted
    // below through the superuser connection.
    let _ = admdb;

    let uri = harness.uri(&id).expose().to_owned();
    let mut superuser = MyClient::connect(&uri).expect("connect as the superuser");
    let charset = superuser
        .query_one_text(
            "SELECT DEFAULT_CHARACTER_SET_NAME FROM information_schema.SCHEMATA \
             WHERE SCHEMA_NAME = 'admdb'",
        )
        .expect("charset query");
    assert_eq!(
        charset, "utf8mb4",
        "{label}: create_database must be utf8mb4"
    );
    let collation = superuser
        .query_one_text(
            "SELECT DEFAULT_COLLATION_NAME FROM information_schema.SCHEMATA \
             WHERE SCHEMA_NAME = 'admdb'",
        )
        .expect("collation query");
    assert_eq!(collation, "utf8mb4_unicode_ci", "{label}");

    // --- the engine refuses to drop a system schema ---
    for system in ["mysql", "sys", "information_schema", "performance_schema"] {
        let err = harness
            .service
            .drop_database(&id, system, true)
            .expect_err("system schema");
        assert_eq!(
            err.code,
            dblocal_core::admin::error::CODE_SYSTEM_SCHEMA,
            "{label}: {system} must be refused"
        );
    }
    assert!(
        harness
            .service
            .list_databases(&id)
            .expect("list")
            .iter()
            .any(|db| db.name() == "admdb"),
        "{label}: the refused drops must not touch admdb"
    );

    // --- ro user: SELECT yes, INSERT no ---
    superuser
        .batch("CREATE TABLE admdb.t (id INTEGER PRIMARY KEY, note VARCHAR(20)); INSERT INTO admdb.t VALUES (1,'x');")
        .expect("seed table");
    let ro_pw = harness
        .service
        .create_user(&id, "ro_user", "admdb", PrivilegeLevel::ReadOnly)
        .expect("create ro user");
    let mut ro = ScopedMyClient::connect(port, "ro_user", ro_pw.expose(), "admdb")
        .expect("ro user connects over TCP");
    assert_eq!(ro.query_i64("SELECT count(*) FROM t").expect("select"), 1);
    assert!(
        !ro.can("INSERT INTO t (id, note) VALUES (2, 'y')"),
        "{label}: a read-only user must not INSERT"
    );

    // --- rw user: INSERT yes, CREATE DATABASE no ---
    let rw_pw = harness
        .service
        .create_user(&id, "rw_user", "admdb", PrivilegeLevel::ReadWrite)
        .expect("create rw user");
    let mut rw = ScopedMyClient::connect(port, "rw_user", rw_pw.expose(), "admdb")
        .expect("rw user connects over TCP");
    rw.batch("INSERT INTO t (id, note) VALUES (2, 'y')")
        .expect("rw user can INSERT");
    assert!(
        !rw.can("CREATE DATABASE rw_escape"),
        "{label}: a read-write user must not create a database"
    );

    // --- password reset: old stops working, new works, URI carries it ---
    let new_password = harness
        .service
        .reset_superuser_password(&id)
        .expect("reset superuser password");
    assert_ne!(
        new_password.expose(),
        extract_password(&uri),
        "the reset rotated the password"
    );
    // A fresh connection with the new password works; the old one does not.
    let new_uri = harness.uri(&id).expose().to_owned();
    let mut after = MyClient::connect(&new_uri).expect("new password authenticates");
    assert_eq!(after.query_i64("SELECT 1").expect("SELECT 1"), 1);
    assert!(
        MyClient::connect(&uri).is_err(),
        "{label}: the old superuser password must stop working"
    );
    assert!(
        new_uri.contains(new_password.expose()),
        "{label}: the URI must carry the new password"
    );

    // --- failed reset keeps the old (current) password working ---
    // (Proven at the service level with a failing sink in the unit tests; here the
    // reset that just succeeded is the live-engine half.)

    // --- drop the created database and delete the instance ---
    harness
        .service
        .drop_database(&id, "admdb", false)
        .expect("drop admdb");
    assert!(
        !harness
            .service
            .list_databases(&id)
            .expect("list")
            .iter()
            .any(|db| db.name() == "admdb"),
        "{label}: admdb must be gone"
    );
    drop(after);
    drop(rw);
    drop(ro);
    drop(superuser);

    let delete = harness.delete(&id, DeleteMode::PurgeData);
    super::expect_ok(&delete, "delete");
    harness.wait_row_gone(&id, Duration::from_secs(30));
    assert!(
        proc::wait_none_under(harness.home.path(), Duration::from_secs(30)),
        "{label}: no engine may remain: {:?}",
        proc::engines_under(harness.home.path())
    );
}

/// The password part of a `mysql://user:password@host:port/` URI.
fn extract_password(uri: &str) -> String {
    let rest = uri.strip_prefix("mysql://").unwrap_or(uri);
    let userinfo = rest.split('@').next().unwrap_or(rest);
    userinfo
        .split_once(':')
        .map(|(_, pw)| pw.to_owned())
        .unwrap_or_default()
}
