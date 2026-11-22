//! MySQL and MariaDB through the real `LocalService`, which never ran these two
//! engines before (W3-B2 review: no artifact existed, so `LocalService::start`
//! for them was dead code). This is the W4-E service-level proof, and it also
//! proves `change_port` for both engines end to end (finding 6).
//!
//! Everything is production code except the credential backend
//! (`FileOnlySecrets`) and the temp home; the runtime comes from the shared
//! process-external cache so the test does not re-download 800 MB. The two
//! downloaders are exercised for real by [`super::catalog_download`].
//!
//! Run under the machine-wide e2e lock:
//!
//! ```sh
//! flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" \
//!   env DBLOCAL_E2E=1 CARGO_TARGET_DIR="$HOME/.cache/dblocal-target" \
//!   cargo test -p dblocal-core --test service_e2e -- --ignored --nocapture mysql_mariadb
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use dblocal_core::model::{DeleteMode, EngineKey, InstanceStatus, Port};
use dblocal_core::service::DblService;

use crate::support::cache;
use crate::support::clients::{MyClient, wait_tcp_gone};
use crate::support::harness::{Harness, newest_spec, newest_version};
use crate::support::{ports, proc};

const MYSQL_ARCHIVE: &str = "mysql-8.4.11-linux-glibc2.28-x86_64-minimal.tar.xz";
const MARIADB_ARCHIVE: &str = "mariadb-11.8.10-linux-systemd-x86_64.tar.gz";

#[test]
#[ignore = "e2e: needs the cached MySQL archive; run with DBLOCAL_E2E=1 under flock"]
fn mysql_service_create_start_change_port_start_connect_delete() {
    super::require_e2e();
    run_engine_case(
        EngineKey::Mysql,
        "mysql-e2e",
        MYSQL_ARCHIVE,
        |harness, id| harness.wait_listening(id, Duration::from_secs(30)),
    );
}

#[test]
#[ignore = "e2e: needs the cached MariaDB archive and libcrypt.so.1; run with DBLOCAL_E2E=1 under flock"]
fn mariadb_service_create_start_change_port_start_connect_delete() {
    super::require_e2e();
    // MariaDB 11.8 on this host needs `libcrypt.so.1`, which the system lacks;
    // the shared cache installs a shim and the runtime's `bin/` wrapper is what
    // the spawned `mariadbd` uses. If the shim cannot be built, say so instead of
    // failing as if the adapter were broken.
    if !crate::support::mariadb_shim::ensure_shim_for_runtime() {
        eprintln!(
            "skip mariadb service e2e: this host cannot load libcrypt.so.1 \
                   and no compatible library was found to shim it"
        );
        return;
    }
    run_engine_case(
        EngineKey::Mariadb,
        "mariadb-e2e",
        MARIADB_ARCHIVE,
        |harness, id| harness.wait_listening(id, Duration::from_secs(30)),
    );
}

/// create -> start -> SQL -> change_port -> start -> SQL on the new port -> delete.
fn run_engine_case(
    engine: EngineKey,
    name: &str,
    archive: &str,
    wait_ready: impl Fn(&Harness, &dblocal_core::model::InstanceId) -> u16,
) {
    let harness = Harness::new("dblocal-e2e-mysqlfam-");
    let version = newest_version(&harness.service, engine);
    assert!(
        cache::extract_archive_to_runtime(&harness.paths, engine, &version, archive),
        "this case needs {archive} cached in ~/.cache/dblocal-dev"
    );

    let original = ports::reserve();
    let spec = newest_spec(&harness.service, name, engine, original);
    let created = harness.create(spec);
    super::expect_ok(&created, "create through LocalService");
    let id = harness
        .instance(name)
        .expect("row after create")
        .id()
        .clone();
    assert_eq!(
        harness.instance_by_id(&id).expect("row").status(),
        InstanceStatus::Stopped
    );

    // First start on the created port.
    let start = harness.start(&id);
    super::expect_ok(&start, "first start");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    let port = wait_ready(&harness, &id);
    assert_eq!(port, original);
    let uri = harness.uri(&id).expose().to_owned();
    let mut client = MyClient::connect(&uri).expect("connect with the service URI");
    assert_eq!(client.query_i64("SELECT 1").expect("SELECT 1"), 1);
    // The URI carries no database, so create and use one explicitly.
    client
        .batch(
            "CREATE DATABASE IF NOT EXISTS dblocal_e2e; \
             CREATE TABLE IF NOT EXISTS dblocal_e2e.e2e (v INT); \
             INSERT INTO dblocal_e2e.e2e VALUES (7);",
        )
        .expect("create and insert");

    // change_port while stopped: the engine must later bind the NEW port.
    let stop = harness.stop(&id);
    super::expect_ok(&stop, "stop before change_port");
    assert!(wait_tcp_gone(original, Duration::from_secs(20)));
    let renamed = ports::reserve();
    assert_ne!(original, renamed);
    harness
        .service
        .change_port(&id, Port::new(renamed).expect("port"))
        .expect("change_port to a free port");
    assert_eq!(
        harness.instance_by_id(&id).expect("row").port().get(),
        renamed
    );

    let restart = harness.start(&id);
    super::expect_ok(&restart, "start after change_port");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    harness.wait_listening(&id, Duration::from_secs(30));
    let pid = harness
        .instance_by_id(&id)
        .expect("row")
        .pid()
        .expect("pid");
    assert!(
        proc::listeners_on(renamed).contains(&pid),
        "the engine must listen on the new port {renamed}; owners {:?}",
        proc::listeners_on(renamed)
    );
    assert!(
        proc::listeners_on(original).is_empty(),
        "the old port {original} must not be listening"
    );

    // The URI followed the port, the data survived, and the new port answers.
    let uri = harness.uri(&id).expose().to_owned();
    assert!(
        uri.contains(&renamed.to_string()),
        "connection_uri must carry the new port: {uri}"
    );
    let mut client = MyClient::connect(&uri).expect("connect on the new port");
    assert_eq!(
        client
            .query_i64("SELECT v FROM dblocal_e2e.e2e")
            .expect("read the row"),
        7,
        "the inserted row must survive a stop/change_port/start"
    );

    // Delete: engine gone, port released, no process left.
    let delete = harness.delete(&id, DeleteMode::PurgeData);
    super::expect_ok(&delete, "delete with PurgeData");
    harness.wait_row_gone(&id, Duration::from_secs(20));
    assert!(
        !harness
            .paths
            .instance_dir(&dblocal_core::model::InstanceName::new(name).expect("name"))
            .exists(),
        "PurgeData must remove the whole instance directory"
    );
    assert!(
        proc::wait_none_under(harness.home.path(), Duration::from_secs(30)),
        "no {} engine may remain: {:?}",
        engine.as_str(),
        proc::engines_under(harness.home.path())
    );
    assert!(
        proc::listeners_on(renamed).is_empty(),
        "the new port {renamed} must be released after delete"
    );
    // `ss -ltnp` is the reviewer's check; the `/proc` scan above is its
    // equivalent here, plus a direct `ss` read when the tool exists.
    if let Ok(output) = std::process::Command::new("ss").arg("-ltnp").output() {
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            !text.contains(&format!(":{renamed} ")),
            "ss still lists port {renamed}:\n{text}"
        );
    }
}
