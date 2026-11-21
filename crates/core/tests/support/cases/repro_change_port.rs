//! Minimal reproduction of a second defect this suite found (W2-C).
//!
//! **Defect:** `change_port` persists the new port on the instance row but never
//! regenerates the engine's configuration, so the next `start_instance` runs the
//! engine on the *old* port. The service probes the new port, spawns, then waits
//! for a health check on the new port that can never answer, and the start ends
//! `Failed` with `DBL-4001` after the full start timeout.
//!
//! Where it lives:
//!
//! - `crates/core/src/instance/read_ops.rs::change_port` writes `store.update_port`
//!   and emits `InstanceChanged`. Nothing re-renders `postgresql.conf` /
//!   `valkey.conf`.
//! - `crates/core/src/instance/start.rs::run_start_inner` calls `prepare`, which
//!   for PostgreSQL is `prepare_postgres_run` (socket directory only) and for
//!   Valkey `prepare_valkey_run` (directories only). Neither rewrites the port.
//! - The adapters bake the port into their config at `initialize` time:
//!   `postgres.rs::configure_after_init` writes `port = <port>` once, and
//!   `valkey.rs::initialize` writes `port <port>` once.
//!
//! **Why it blocks the PRD:** `F-PRT-02` AC2 says the one-click "move to the next
//! free port" must update the instance configuration and continue the start
//! without another step. With this defect that flow always fails, which is the
//! most common start failure the port manager exists to prevent (`D-16`).
//!
//! **Not patched here.** `read_ops.rs`, `start.rs` and both adapters are
//! non-test code in `dblocal-core`, which this task may not change.
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
fn repro_change_port_does_not_reconfigure_the_engine() {
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

    // Two free ports, no foreign listener involved at all.
    let original = ports::reserve();
    let renamed = ports::reserve();
    assert_ne!(original, renamed);
    let spec = newest_spec(&harness.service, "pg-port", EngineKey::Postgresql, original);
    let created = harness.create(spec);
    super::expect_ok(&created, "create on the original port");
    let id = harness.instance("pg-port").expect("row").id().clone();
    let name = InstanceName::new("pg-port").expect("name");

    // The created instance's configuration names the original port.
    let conf_path = harness.paths.data_dir(&name).join("postgresql.conf");
    let conf = std::fs::read_to_string(&conf_path).expect("read postgresql.conf");
    let configured_port = conf
        .lines()
        .find_map(|line| line.trim().strip_prefix("port = "))
        .map(|value| value.trim().to_owned());
    eprintln!(
        "postgresql.conf at create says `port = {configured_port:?}` (instance port {original})"
    );
    assert_eq!(
        configured_port.as_deref(),
        Some(original.to_string().as_str()),
        "the engine config must start out on the instance port"
    );

    // Move the instance to another free port.
    harness
        .service
        .change_port(&id, Port::new(renamed).expect("port"))
        .expect("change_port to a free port");
    assert_eq!(
        harness.instance_by_id(&id).expect("row").port().get(),
        renamed,
        "the row must record the new port"
    );
    let conf_after = std::fs::read_to_string(&conf_path).expect("re-read postgresql.conf");
    let configured_after = conf_after
        .lines()
        .find_map(|line| line.trim().strip_prefix("port = "))
        .map(|value| value.trim().to_owned());
    eprintln!(
        "after change_port to {renamed}, postgresql.conf still says `port = {configured_after:?}`"
    );

    // Now start. If the config had been rewritten, the engine would come up on
    // `renamed` and the start would succeed.
    let start_run = harness.start(&id);
    let outcome = format!("{:?}", start_run.result);
    eprintln!(
        "start after change_port: {outcome}; stages {:?}",
        start_run.stage_names()
    );

    match &start_run.result {
        Ok(()) => {
            // The config was rewritten (or the engine re-read it): the defect is
            // gone. Prove the engine really is on the new port.
            harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
            let pid = harness
                .instance_by_id(&id)
                .expect("row")
                .pid()
                .expect("pid");
            assert!(
                proc::listeners_on(renamed).contains(&pid),
                "the engine must listen on the new port {renamed}"
            );
            eprintln!("change_port → start works now; remove this reproduction");
            let delete = harness.delete(&id, DeleteMode::PurgeData);
            super::expect_ok(&delete, "cleanup delete");
        }
        Err(err) => {
            // Characterisation: the row is Failed, nothing was left listening on
            // the new port, and the config still names the old one.
            assert_eq!(
                configured_after.as_deref(),
                Some(original.to_string().as_str()),
                "the config is expected to still name the original port"
            );
            assert_eq!(
                harness.instance_by_id(&id).expect("row").status(),
                InstanceStatus::Failed,
                "the start must have ended Failed"
            );
            let listeners = proc::listeners_on(renamed);
            eprintln!(
                "DEFECT CONFIRMED: start after change_port failed with [{}] {}; \
                 postgresql.conf still says port {configured_after:?} while the row says {renamed}; \
                 listeners on {renamed}: {listeners:?}",
                err.code, err.message
            );
            assert!(
                listeners.is_empty(),
                "nothing may be left listening on the new port"
            );
        }
    }

    // Whatever happened, leave no engine behind.
    let stuck: Vec<u32> = proc::engines_under(harness.home.path())
        .into_iter()
        .map(|info| info.pid)
        .collect();
    for pid in stuck {
        proc::sigkill(pid);
    }
    proc::wait_none_under(harness.home.path(), Duration::from_secs(20));
}
