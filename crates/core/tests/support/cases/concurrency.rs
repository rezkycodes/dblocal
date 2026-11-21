//! Case 6 — concurrency on a real engine.
//!
//! Two independent services over one home start the same stopped instance at the
//! same instant, 10 rounds. Exactly one engine runs per round (counted by
//! process group, because PostgreSQL forks workers into the leader's group), and
//! the loser gets the typed busy error.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Barrier};
use std::time::Duration;

use dblocal_core::error::CODE_INVALID_TRANSITION;
use dblocal_core::model::{DeleteMode, EngineKey, InstanceId, InstanceStatus};
use dblocal_core::service::{DblService, LocalService, ServiceEvent};

use crate::support::cache;
use crate::support::clients::wait_tcp;
use crate::support::env::TempHome;
use crate::support::harness::{self, newest_spec, newest_version};
use crate::support::{ports, proc};

const ROUNDS: usize = 10;

#[test]
#[ignore = "e2e: network + several hundred MB of disk; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn two_services_start_blocking_identical_rounds_each_get_one_engine() {
    super::require_e2e();

    let home = TempHome::new("dblocal-e2e-race-");
    let paths = home.paths();

    // Create and settle the instance through one service, then drop it: both
    // racing services below see a clean `Stopped` row, which is what the PRD
    // race describes.
    let version = {
        let bootstrap = harness::build(&paths);
        let version = newest_version(&bootstrap, EngineKey::Postgresql);
        assert!(
            cache::extract_archive_to_runtime(
                &paths,
                EngineKey::Postgresql,
                &version,
                "postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz",
            ),
            "test 6 needs the PostgreSQL archive cached in ~/.cache/dblocal-dev"
        );
        let port = ports::reserve();
        let spec = newest_spec(&bootstrap, "pg-race", EngineKey::Postgresql, port);
        let mut stream = crate::support::events::Events::new(bootstrap.subscribe());
        let op = bootstrap.create_instance(spec).expect("create_instance");
        let created = stream.until_finished(&op, crate::CREATE_TIMEOUT);
        super::expect_ok(&created, "create");
        version
    };

    let port = harness::row_if_present(&paths, &id_of(&paths, "pg-race"))
        .expect("created row")
        .port()
        .get();
    eprintln!("race instance pg-race on port {port}, version {version}");

    let first: Arc<LocalService> = Arc::new(harness::build(&paths));
    let second: Arc<LocalService> = Arc::new(harness::build(&paths));

    for round in 0..ROUNDS {
        let id = id_of(&paths, "pg-race");
        assert_eq!(
            harness::row(&paths, &id).status(),
            InstanceStatus::Stopped,
            "round {round}: the instance must be Stopped before the race"
        );

        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for service in [Arc::clone(&first), Arc::clone(&second)] {
            let barrier = Arc::clone(&barrier);
            let id = id.clone();
            handles.push(std::thread::spawn(move || {
                let mut stream = crate::support::events::Events::new(service.subscribe());
                barrier.wait();
                let outcome = service.start_instance(&id);
                match outcome {
                    Err(err) => Err(err),
                    Ok(op) => Ok(stream.until_finished(&op, crate::START_TIMEOUT)),
                }
            }));
        }
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("the racing thread must not panic"))
            .collect();

        let mut winners = 0;
        let mut losers = 0;
        for result in &results {
            match result {
                Ok(run) => {
                    if run.result.is_ok() {
                        winners += 1;
                    } else {
                        // The claim can be won by one service and then fail for an
                        // unrelated reason; that is not the race we assert here.
                        let err = run.result.as_ref().expect_err("checked");
                        panic!(
                            "round {round}: the winning start failed: [{}] {}",
                            err.code, err.message
                        );
                    }
                }
                Err(err) => {
                    assert_eq!(
                        err.code(),
                        CODE_INVALID_TRANSITION,
                        "round {round}: the loser must get the typed busy error, got {err:?}"
                    );
                    losers += 1;
                }
            }
        }
        assert_eq!(winners, 1, "round {round}: exactly one start must win");
        assert_eq!(
            losers, 1,
            "round {round}: exactly one start must be refused"
        );

        // Exactly one engine process: one process group under this home.
        let groups = proc::process_groups_under(home.path());
        assert_eq!(
            groups.len(),
            1,
            "round {round}: exactly one engine may run, found groups {groups:?} for processes {:?}",
            proc::engines_under(home.path())
        );
        let row = harness::row(&paths, &id);
        assert_eq!(
            row.status(),
            InstanceStatus::Running,
            "round {round}: the row must be Running"
        );
        assert!(
            wait_tcp(port, Duration::from_secs(20)),
            "round {round}: the engine must listen"
        );
        let pid = row.pid().expect("running row has a pid");
        assert_eq!(
            proc::listeners_on(port),
            vec![pid],
            "round {round}: only the recorded engine may listen on {port}"
        );

        // Bring the instance back to Stopped for the next round.
        let mut stream = crate::support::events::Events::new(first.subscribe());
        let op = first.stop_instance(&id).expect("stop to reset the round");
        let stopped = stream.until_finished(&op, crate::STOP_TIMEOUT);
        super::expect_ok(&stopped, "stop between rounds");
        assert_eq!(harness::row(&paths, &id).status(), InstanceStatus::Stopped);
        assert!(
            proc::wait_none_under(home.path(), Duration::from_secs(20)),
            "round {round}: no engine may survive the reset: {:?}",
            proc::engines_under(home.path())
        );
        eprintln!("round {round}: one winner, one busy loser, one engine, stopped again");
    }

    // Clean up the row through a service so the temp home can be dropped cleanly.
    let id = id_of(&paths, "pg-race");
    let mut stream = crate::support::events::Events::new(first.subscribe());
    let op = first
        .delete_instance(&id, DeleteMode::PurgeData)
        .expect("cleanup delete");
    let deleted = stream.until_finished(&op, crate::DELETE_TIMEOUT);
    super::expect_ok(&deleted, "cleanup delete");
    drop(first);
    drop(second);
    assert!(
        proc::wait_none_under(home.path(), Duration::from_secs(20)),
        "no engine may remain: {:?}",
        proc::engines_under(home.path())
    );
}

/// The instance id of the row called `name`.
fn id_of(paths: &dblocal_core::paths::DblPaths, name: &str) -> InstanceId {
    dblocal_core::store::Store::open(paths)
        .expect("open the state store")
        .get_instance_by_name(name)
        .expect("get_instance_by_name")
        .unwrap_or_else(|| panic!("no row called {name}"))
        .id()
        .clone()
}

#[allow(dead_code)]
fn _unused(_: ServiceEvent) {}
