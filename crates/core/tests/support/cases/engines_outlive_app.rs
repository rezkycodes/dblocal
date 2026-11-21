//! Case 3 — engines outlive the app (D-02, `F-INS-03` AC2, `F-TRY-02` AC3).
//!
//! Start an instance, drop the whole `LocalService` (the app exited), prove the
//! engine still serves queries, adopt it through a fresh service over the same
//! home, stop it through that service, then `kill -9` an engine and prove the
//! supervisor notices and that a retry works.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::{Duration, Instant};

use dblocal_core::model::{DeleteMode, EngineKey, InstanceStatus};
use dblocal_core::service::{DblService, LocalService, ServiceEvent, UriFormat};

use crate::support::cache;
use crate::support::clients::{PgClient, wait_tcp};
use crate::support::harness::{self, Harness, build, newest_spec, newest_version};
use crate::support::{events, ports, proc};

#[test]
#[ignore = "e2e: network + several hundred MB of disk; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn engines_outlive_the_app_and_are_adopted() {
    super::require_e2e();

    let harness = Harness::new("dblocal-e2e-outlive-");
    let version = newest_version(&harness.service, EngineKey::Postgresql);
    assert!(
        cache::extract_archive_to_runtime(
            &harness.paths,
            EngineKey::Postgresql,
            &version,
            "postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz",
        ),
        "test 3 needs the PostgreSQL archive cached in ~/.cache/dblocal-dev; \
         the production extractor rejects its hard links (see repro_hardlink), \
         so the runtime is materialised from the cached archive instead"
    );
    let port = ports::reserve();
    let spec = newest_spec(&harness.service, "pg-outlive", EngineKey::Postgresql, port);
    let created = harness.create(spec);
    super::expect_ok(&created, "create");
    let id = harness
        .instance("pg-outlive")
        .expect("row after create")
        .id()
        .clone();
    let start_run = harness.start(&id);
    super::expect_ok(&start_run, "start");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    assert!(
        wait_tcp(port, Duration::from_secs(20)),
        "the engine must listen"
    );

    let running = harness.instance_by_id(&id).expect("row");
    let pid = running.pid().expect("a running row records its pid");
    let started_at = running.process_started_at().expect("a start time");
    let exe = running.process_exe().expect("an executable").to_path_buf();
    eprintln!(
        "engine identity before the app exit: pid={pid} exe={} started={started_at:?}",
        exe.display()
    );
    let uri_text = harness.uri(&id).expose().to_owned();

    // Simulate a full app exit: drop every service handle.
    let Harness {
        home,
        paths,
        service,
    } = harness;
    drop(service);
    // `LocalService::drop` signals the runtime thread and joins it, so after this
    // instant no code from the app runs at all.
    std::thread::sleep(Duration::from_millis(300));

    // The engine must still be alive and answer a real query.
    assert!(
        proc::alive(pid),
        "the engine pid {pid} must survive the app exit (D-02)"
    );
    let client = PgClient::connect(&uri_text, Duration::from_secs(10))
        .expect("the engine must still accept the URI after the app exited");
    assert_eq!(
        client.query_one("SELECT 7").expect("query a live engine"),
        7
    );
    eprintln!("the engine answered SELECT 7 after the whole service was dropped");

    // A new service over the same home adopts the engine as Running.
    let service = build(&paths);
    let adopted = wait_for_status(
        &paths,
        &id,
        InstanceStatus::Running,
        Duration::from_secs(30),
    );
    eprintln!(
        "adopted row: status={:?} pid={:?}",
        adopted.status(),
        adopted.pid()
    );
    assert_eq!(
        adopted.status(),
        InstanceStatus::Running,
        "adopted as Running"
    );
    assert_eq!(
        adopted.pid(),
        Some(pid),
        "the same identity must be recorded"
    );
    assert_eq!(
        adopted.process_exe(),
        Some(exe.as_path()),
        "the executable must match the pre-exit identity"
    );
    assert_eq!(
        adopted.process_started_at(),
        Some(started_at),
        "the kernel start time must match the pre-exit identity"
    );
    assert_eq!(
        service
            .connection_uri(&id, UriFormat::Uri)
            .expect("uri after adoption")
            .expose(),
        uri_text,
        "the adopted instance must keep the same connection URI"
    );

    // Stop through the new service: the engine must go away.
    let mut stream = events::Events::new(service.subscribe());
    let op = service
        .stop_instance(&id)
        .expect("stop through the new service");
    let stop_run = stream.until_finished(&op, crate::STOP_TIMEOUT);
    super::expect_ok(&stop_run, "stop after adoption");
    assert!(
        proc::wait_gone(pid, Duration::from_secs(30)),
        "the adopted engine must be stopped by the new service"
    );
    assert_eq!(
        harness::row(&paths, &id).status(),
        InstanceStatus::Stopped,
        "the row must be Stopped after the stop"
    );

    // Start again, then kill -9 and watch the supervisor report Failed.
    let mut stream = events::Events::new(service.subscribe());
    let op = service.start_instance(&id).expect("restart");
    let restarted = stream.until_finished(&op, crate::START_TIMEOUT);
    super::expect_ok(&restarted, "restart before the kill");
    let pid = harness::row(&paths, &id).pid().expect("pid after restart");
    assert!(
        wait_tcp(port, Duration::from_secs(20)),
        "the engine must listen again"
    );

    // The supervisor polls at STATUS_POLL_SECS (2 s) in production.
    let mut stream = events::Events::new(service.subscribe());
    proc::sigkill(pid);
    assert!(
        proc::wait_gone(pid, Duration::from_secs(30)),
        "kill -9 must take the process down"
    );
    let killed = Instant::now();
    let seen = stream.until(
        Duration::from_secs(30),
        "the supervisor's Failed event",
        |event| {
            matches!(
                event,
                ServiceEvent::InstanceChanged { view }
                    if view.id == id && view.status == InstanceStatus::Failed
            )
        },
    );
    eprintln!(
        "supervisor noticed the kill after {:.2}s: {:?}",
        killed.elapsed().as_secs_f64(),
        events::describe(&seen)
    );
    let view = service
        .list_instances()
        .expect("list_instances")
        .into_iter()
        .find(|view| view.id == id)
        .expect("the row must still be listed as Failed");
    assert_eq!(
        view.status,
        InstanceStatus::Failed,
        "the supervisor must mark a dead engine Failed"
    );
    let failure = view.failure.expect("a Failed row must carry a diagnosis");
    eprintln!(
        "diagnosis for the killed engine: code={} message={} lines={} cause={:?}",
        failure.code,
        failure.message,
        failure.log_tail.len(),
        failure.diagnosis
    );
    assert!(
        !failure.message.is_empty(),
        "the diagnosis message must not be empty"
    );
    assert!(
        !failure.log_tail.is_empty()
            || failure.diagnosis != dblocal_core::model::StartFailure::Unknown,
        "the failure must explain itself: a log tail or a known cause"
    );

    // A Failed instance must be startable again.
    let mut stream = events::Events::new(service.subscribe());
    let op = service.start_instance(&id).expect("start after a crash");
    let recovered = stream.until_finished(&op, crate::START_TIMEOUT);
    super::expect_ok(&recovered, "start after a crash");
    assert_eq!(
        harness::row(&paths, &id).status(),
        InstanceStatus::Running,
        "Start must work again after a crash"
    );

    // Clean up through the service, then prove nothing is left.
    let mut stream = events::Events::new(service.subscribe());
    let op = service
        .delete_instance(&id, DeleteMode::PurgeData)
        .expect("cleanup delete");
    let cleanup = stream.until_finished(&op, crate::DELETE_TIMEOUT);
    super::expect_ok(&cleanup, "cleanup delete");
    drop(service);
    assert!(
        proc::wait_none_under(home.path(), Duration::from_secs(20)),
        "no engine may remain: {:?}",
        proc::engines_under(home.path())
    );
}

/// Polls the store until the row has `status`.
fn wait_for_status(
    paths: &dblocal_core::paths::DblPaths,
    id: &dblocal_core::model::InstanceId,
    status: InstanceStatus,
    timeout: Duration,
) -> dblocal_core::model::Instance {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(row) = harness::row_if_present(paths, id)
            && row.status() == status
        {
            return row;
        }
        assert!(
            Instant::now() < deadline,
            "instance {id} never reached {status:?} through reconciliation"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[allow(dead_code)]
fn _unused(_: &LocalService) {}
