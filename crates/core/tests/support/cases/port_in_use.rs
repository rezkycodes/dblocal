//! Case 4 — port in use (`F-PRT-02`, D-16).
//!
//! A foreign listener on the chosen port makes `create_instance` and
//! `start_instance` fail with the typed port-in-use error, nothing is spawned,
//! and once the foreign listener is gone the same instance starts normally.
//!
//! The `F-PRT-02` AC2 continuation — "move to the next free port and start" via
//! `change_port` — is **blocked by a defect this suite found** and is proved
//! separately in `repro_change_port`: `change_port` persists the new port on the
//! row but never regenerates `postgresql.conf`/`valkey.conf`, so the start that
//! follows waits for a health check on a port the engine is not listening on and
//! ends `DBL-4001`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use dblocal_core::error::CODE_PORT_IN_USE;
use dblocal_core::model::{DeleteMode, EngineKey, InstanceStatus};

use crate::support::cache;
use crate::support::clients::wait_tcp;
use crate::support::harness::{Harness, newest_spec, newest_version};
use crate::support::{ports, proc};

#[test]
#[ignore = "e2e: network + several hundred MB of disk; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn a_port_taken_by_a_foreign_listener_is_a_typed_error() {
    super::require_e2e();

    let harness = Harness::new("dblocal-e2e-port-");
    let version = newest_version(&harness.service, EngineKey::Postgresql);
    assert!(
        cache::extract_archive_to_runtime(
            &harness.paths,
            EngineKey::Postgresql,
            &version,
            "postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz",
        ),
        "test 4 needs the PostgreSQL archive cached in ~/.cache/dblocal-dev"
    );

    // create_instance on a held port must fail with the typed error.
    let foreign = ports::hold_reserved();
    let blocked = foreign.port();
    eprintln!("a foreign listener holds port {blocked}");
    let spec = newest_spec(&harness.service, "pg-port", EngineKey::Postgresql, blocked);
    let run = harness.create(spec);
    let err = super::expect_err(&run, "create on a held port");
    eprintln!(
        "create on a held port failed: [{}] {}; stages {:?}",
        err.code,
        err.message,
        run.stage_names()
    );
    assert_eq!(
        err.code, CODE_PORT_IN_USE,
        "create on a held port must report DBL-1005"
    );
    assert!(
        harness.view("pg-port").is_none(),
        "a refused create must leave no row"
    );
    drop(foreign);

    // Create on a free port, then let a foreign listener take it before Start.
    let contested = ports::reserve();
    let spec = newest_spec(
        &harness.service,
        "pg-port",
        EngineKey::Postgresql,
        contested,
    );
    let created = harness.create(spec);
    super::expect_ok(&created, "create on a free port");
    let id = harness.instance("pg-port").expect("row").id().clone();
    let foreign = ports::hold(contested);
    eprintln!("a foreign listener just took the instance's port {contested}");

    let start_run = harness.start(&id);
    let err = super::expect_err(&start_run, "start on a port taken after create");
    eprintln!(
        "start on a port taken after create failed: [{}] {}; stages {:?}",
        err.code,
        err.message,
        start_run.stage_names()
    );
    assert_eq!(
        err.code, CODE_PORT_IN_USE,
        "start on a taken port must report DBL-1005"
    );
    assert!(
        start_run.saw_status(&id, InstanceStatus::Failed),
        "a start refused on its port must end Failed: {:?}",
        start_run.stage_names()
    );
    // AC3: the bind probe closed its socket, so nothing of ours holds the port
    // and nothing was spawned to fail later.
    assert!(
        proc::engines_under(harness.home.path()).is_empty(),
        "a refused start must not spawn anything: {:?}",
        proc::engines_under(harness.home.path())
    );

    // Release the port; the same instance now starts on it.
    drop(foreign);
    let start_run = harness.start(&id);
    super::expect_ok(&start_run, "start after the foreign listener left");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    let pid = harness
        .instance_by_id(&id)
        .expect("row")
        .pid()
        .expect("pid");
    assert!(
        wait_tcp(contested, Duration::from_secs(20)),
        "the engine must listen on {contested}"
    );
    assert!(
        proc::listeners_on(contested).contains(&pid),
        "the engine pid {pid} must own the listener on {contested}; owners: {:?}",
        proc::listeners_on(contested)
    );
    eprintln!("engine pid {pid} listening on {contested} once the port was free");

    // Clean up.
    let delete = harness.delete(&id, DeleteMode::PurgeData);
    super::expect_ok(&delete, "cleanup delete");
    assert!(
        proc::wait_none_under(harness.home.path(), Duration::from_secs(20)),
        "no engine may remain"
    );
}
