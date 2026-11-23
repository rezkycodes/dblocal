//! Config editor through the real `LocalService` and real engines (`F-CFG-01`).
//!
//! Two engines where the change is observable from the running server with no
//! extra tooling: PostgreSQL `SHOW max_connections` and Valkey `CONFIG GET
//! maxmemory`. Each case applies a change, restarts, reads it back from the
//! running engine, then applies a value the engine itself rejects, observes
//! `restore_available`, restores, and starts again.
//!
//! Everything is production code except the temp home, the credential backend
//! (`FileOnlySecrets`) and the runtime copied from the shared process-external
//! cache. Run under the machine-wide e2e lock:
//!
//! ```sh
//! flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" \
//!   env DBLOCAL_E2E=1 CARGO_TARGET_DIR="$HOME/.cache/dblocal-target" \
//!   cargo test -p dblocal-core --test service_e2e -- --ignored --nocapture config
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use dblocal_core::model::{DeleteMode, EngineKey, InstanceStatus};
use dblocal_core::service::DblService;

use crate::support::cache;
use crate::support::clients::{PgClient, RedisClient, wait_tcp_gone};
use crate::support::harness::{Harness, newest_spec, newest_version};
use crate::support::{ports, proc};

const PG_ARCHIVE: &str = "postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz";
const VALKEY_ARCHIVE: &str = "valkey-9.2.0-jammy-x86_64.tar.gz";

#[test]
#[ignore = "e2e: needs the cached PostgreSQL archive; run with DBLOCAL_E2E=1 under flock"]
fn config_change_restart_observe_restore_postgresql() {
    super::require_e2e();
    let harness = Harness::new("dblocal-e2e-cfg-pg-");
    let engine = EngineKey::Postgresql;
    let version = newest_version(&harness.service, engine);
    assert!(
        cache::extract_archive_to_runtime(&harness.paths, engine, &version, PG_ARCHIVE),
        "this case needs {PG_ARCHIVE} cached in ~/.cache/dblocal-dev"
    );
    let port = ports::reserve();
    let spec = newest_spec(&harness.service, "cfg-pg", engine, port);
    super::expect_ok(&harness.create(spec), "create");
    let id = harness.instance("cfg-pg").expect("row").id().clone();

    // Start once and read the real value so we know it is the default (100).
    super::expect_ok(&harness.start(&id), "first start");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    let uri = harness.uri(&id).expose().to_owned();
    {
        harness.wait_listening(&id, Duration::from_secs(30));
        let client = PgClient::connect(&uri, Duration::from_secs(10)).expect("connect");
        assert_eq!(
            client
                .query_text("SHOW max_connections")
                .expect("show")
                .trim(),
            "100",
            "the default max_connections is 100"
        );
    }

    // Change it through the Config API.
    let applied = harness
        .service
        .config_apply(&id, vec![("max_connections".to_owned(), "150".to_owned())])
        .expect("config_apply");
    assert!(applied.restart_required);
    assert!(applied.backup.path.is_file(), "a backup was taken");

    // Restart: the running engine now reports 150.
    super::expect_ok(&harness.stop(&id), "stop after apply");
    assert!(wait_tcp_gone(port, Duration::from_secs(20)));
    super::expect_ok(&harness.start(&id), "start after apply");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    {
        harness.wait_listening(&id, Duration::from_secs(30));
        let uri = harness.uri(&id).expose().to_owned();
        let client = PgClient::connect(&uri, Duration::from_secs(10)).expect("connect");
        assert_eq!(
            client
                .query_text("SHOW max_connections")
                .expect("show")
                .trim(),
            "150",
            "the new max_connections reached the engine"
        );
    }

    // A value the engine rejects at start: a shared_buffers far above any
    // machine's limit, which our validator accepts (no upper bound the machine
    // can know) but the engine refuses when it maps the segment.
    let applied = harness
        .service
        .config_apply(&id, vec![("shared_buffers".to_owned(), "200GB".to_owned())])
        .expect("config_apply of a too-large shared_buffers");
    assert!(applied.backup.path.is_file());

    // Stop, then the start must fail with restore_available set.
    let _ = harness.stop(&id);
    let start = harness.start(&id);
    assert!(
        start.result.is_err(),
        "the engine must refuse a 200GB shared_buffers"
    );
    harness.wait_status(&id, InstanceStatus::Failed, Duration::from_secs(5));
    let view = harness.view("cfg-pg").expect("view");
    let failure = view.failure.expect("failure details");
    assert!(
        failure.restore_available,
        "a failed start after a config change must offer a restore; code {}",
        failure.code
    );

    // Restore and start again: running, with the previous good value.
    harness.service.config_restore(&id).expect("config_restore");
    super::expect_ok(&harness.start(&id), "start after restore");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    {
        harness.wait_listening(&id, Duration::from_secs(30));
        let uri = harness.uri(&id).expose().to_owned();
        let client = PgClient::connect(&uri, Duration::from_secs(10)).expect("connect");
        assert_eq!(
            client
                .query_text("SHOW max_connections")
                .expect("show")
                .trim(),
            "150",
            "restore brought back the 150 that started before the bad change"
        );
    }

    let delete = harness.delete(&id, DeleteMode::PurgeData);
    super::expect_ok(&delete, "delete");
    assert!(proc::wait_none_under(
        harness.home.path(),
        Duration::from_secs(30)
    ));
}

#[test]
#[ignore = "e2e: needs the cached Valkey archive; run with DBLOCAL_E2E=1 under flock"]
fn config_change_restart_observe_restore_valkey() {
    super::require_e2e();
    let harness = Harness::new("dblocal-e2e-cfg-vk-");
    let engine = EngineKey::Valkey;
    let version = newest_version(&harness.service, engine);
    assert!(
        cache::extract_archive_to_runtime(&harness.paths, engine, &version, VALKEY_ARCHIVE),
        "this case needs {VALKEY_ARCHIVE} cached in ~/.cache/dblocal-dev"
    );
    let port = ports::reserve();
    let spec = newest_spec(&harness.service, "cfg-vk", engine, port);
    super::expect_ok(&harness.create(spec), "create");
    let id = harness.instance("cfg-vk").expect("row").id().clone();

    super::expect_ok(&harness.start(&id), "first start");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    harness.wait_listening(&id, Duration::from_secs(30));
    {
        let uri = harness.uri(&id).expose().to_owned();
        let mut client = RedisClient::connect(&uri).expect("connect");
        assert_eq!(client.config_get("maxmemory").expect("get"), "0");
    }

    let applied = harness
        .service
        .config_apply(&id, vec![("maxmemory".to_owned(), "64mb".to_owned())])
        .expect("config_apply");
    assert!(applied.restart_required);
    assert!(applied.backup.path.is_file());

    super::expect_ok(&harness.stop(&id), "stop after apply");
    assert!(wait_tcp_gone(port, Duration::from_secs(20)));
    super::expect_ok(&harness.start(&id), "start after apply");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    harness.wait_listening(&id, Duration::from_secs(30));
    {
        let uri = harness.uri(&id).expose().to_owned();
        let mut client = RedisClient::connect(&uri).expect("connect");
        assert_eq!(
            client.config_get("maxmemory").expect("get"),
            (64 * 1024 * 1024).to_string(),
            "the new maxmemory reached the engine"
        );
    }

    // A value the engine rejects at start. `save` with an odd number of numbers
    // is accepted by our token validator (digits and spaces) but Valkey refuses
    // it when it parses the config at startup.
    let applied = harness
        .service
        .config_apply(&id, vec![("save".to_owned(), "3600".to_owned())])
        .expect("config_apply of an odd save schedule");
    assert!(applied.backup.path.is_file());

    let _ = harness.stop(&id);
    let start = harness.start(&id);
    assert!(
        start.result.is_err(),
        "valkey must refuse a `save` with an odd number of tokens"
    );
    harness.wait_status(&id, InstanceStatus::Failed, Duration::from_secs(5));
    let view = harness.view("cfg-vk").expect("view");
    let failure = view.failure.expect("failure details");
    assert!(
        failure.restore_available,
        "a failed start after a config change must offer a restore; code {}",
        failure.code
    );

    harness.service.config_restore(&id).expect("config_restore");
    super::expect_ok(&harness.start(&id), "start after restore");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    harness.wait_listening(&id, Duration::from_secs(30));
    {
        let uri = harness.uri(&id).expose().to_owned();
        let mut client = RedisClient::connect(&uri).expect("connect");
        assert_eq!(
            client.config_get("maxmemory").expect("get"),
            (64 * 1024 * 1024).to_string(),
            "restore brought back the 64mb that started before the bad change"
        );
    }

    let delete = harness.delete(&id, DeleteMode::PurgeData);
    super::expect_ok(&delete, "delete");
    assert!(proc::wait_none_under(
        harness.home.path(),
        Duration::from_secs(30)
    ));
}
