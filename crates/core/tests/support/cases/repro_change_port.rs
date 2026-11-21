//! Regression tests for D-2 (W2-F): `change_port` must reconfigure the engine.
//!
//! The store port and the engine config must never disagree. `change_port`
//! rewrites the on-disk config (atomic) and `start` re-syncs from the store
//! before spawning, so a crash between the two writes is repaired on next start.
//!
//! Run:
//!
//! ```sh
//! DBLOCAL_E2E=1 cargo test -p dblocal-core --test service_e2e -- --ignored \
//!   --nocapture repro::change_port
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use dblocal_core::model::{DeleteMode, EngineKey, InstanceName, InstanceStatus, Port};
use dblocal_core::service::DblService;

use crate::support::cache;
use crate::support::harness::{self, newest_spec, newest_version};
use crate::support::{ports, proc};

#[test]
#[ignore = "e2e: run with DBLOCAL_E2E=1 cargo test --test service_e2e -- --ignored repro::change_port"]
fn repro_change_port_reconfigures_the_engine() {
    super::require_e2e();

    let harness = harness::Harness::new("dblocal-e2e-changeport-");
    let version = newest_version(&harness.service, EngineKey::Postgresql);
    assert!(
        cache::extract_archive_to_runtime(
            &harness.paths,
            EngineKey::Postgresql,
            &version,
            "postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz",
        ),
        "this reproduction needs the PostgreSQL archive cached in ~/.cache/dblocal-dev"
    );

    let original = ports::reserve();
    let renamed = ports::reserve();
    assert_ne!(original, renamed);
    let spec = newest_spec(&harness.service, "pg-port", EngineKey::Postgresql, original);
    let created = harness.create(spec);
    super::expect_ok(&created, "create on the original port");
    let id = harness.instance("pg-port").expect("row").id().clone();
    let name = InstanceName::new("pg-port").expect("name");

    let conf_path = harness.paths.data_dir(&name).join("postgresql.conf");
    let conf = std::fs::read_to_string(&conf_path).expect("read postgresql.conf");
    assert!(
        conf.lines()
            .any(|l| l.trim() == format!("port = {original}")),
        "postgresql.conf must start with the original port"
    );

    harness
        .service
        .change_port(&id, Port::new(renamed).expect("port"))
        .expect("change_port to a free port");
    assert_eq!(
        harness.instance_by_id(&id).expect("row").port().get(),
        renamed
    );
    let conf_after = std::fs::read_to_string(&conf_path).expect("re-read postgresql.conf");
    assert!(
        conf_after
            .lines()
            .any(|l| l.trim() == format!("port = {renamed}")),
        "change_port must rewrite postgresql.conf, got:\n{conf_after}"
    );
    assert!(
        !conf_after
            .lines()
            .any(|l| l.trim() == format!("port = {original}")),
        "old port must be gone from postgresql.conf"
    );

    let start_run = harness.start(&id);
    super::expect_ok(&start_run, "start after change_port");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    let pid = harness
        .instance_by_id(&id)
        .expect("row")
        .pid()
        .expect("pid");
    assert!(
        proc::listeners_on(renamed).contains(&pid),
        "engine must listen on the new port {renamed}"
    );
    assert!(
        proc::listeners_on(original).is_empty(),
        "old port {original} must not be listening"
    );

    let delete = harness.delete(&id, DeleteMode::PurgeData);
    super::expect_ok(&delete, "cleanup delete");
}
