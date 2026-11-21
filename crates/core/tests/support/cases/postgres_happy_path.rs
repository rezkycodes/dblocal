//! Case 1 — PostgreSQL: the real catalog and HTTPS downloader, plus the whole
//! lifecycle through the public `DblService`.
//!
//! Two tests, because one of them is blocked by a defect this suite found:
//!
//! - [`postgres_download_and_extract_from_the_catalog`] runs the production
//!   download → verify → extract pipeline against the shipped catalog and the
//!   real HTTPS URL. It **fails today**, and the failure is the finding: the
//!   extractor rejects the in-tree hard links in the artifact (see
//!   `repro_hardlink`).
//! - [`postgres_happy_path_from_catalog_to_archive`] proves everything after an
//!   installed runtime — create, start, SQL, stop, restart, read back,
//!   archive-delete, purge — over the documented "runtime already installed"
//!   path (`F-RTM-03` AC3). It materialises the runtime from the same cached
//!   archive so the hard-link defect does not mask the rest.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use dblocal_core::model::{DeleteMode, EngineKey, InstanceStatus};
use dblocal_core::service::DblService;

use crate::support::cache;
use crate::support::clients::{PgClient, wait_tcp_gone};
use crate::support::harness::{Harness, dir_size, newest_spec, newest_version};
use crate::support::{ports, proc};

const PG_ARCHIVE: &str = "postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz";

/// The stage order a create that has to download must produce.
const DOWNLOAD_STAGES: &[&str] = &[
    "queued",
    "downloading",
    "verifying",
    "extracting",
    "initializing",
    "done",
];

/// Exercises the real downloader against the shipped catalog.
///
/// Fails today with the hard-link defect; the assertion message names it and
/// links the minimal reproduction.
#[test]
#[ignore = "e2e: network + several hundred MB of disk; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn postgres_download_and_extract_from_the_catalog() {
    super::require_e2e();

    let harness = Harness::new("dblocal-e2e-pg-download-");
    let version = newest_version(&harness.service, EngineKey::Postgresql);
    eprintln!("postgres e2e: newest catalog version {version}");
    let port = ports::reserve();
    let spec = newest_spec(&harness.service, "pg-e2e", EngineKey::Postgresql, port);
    assert_eq!(
        spec.version(),
        version,
        "the spec must use the newest version"
    );

    let mut events = harness.events();
    let op = harness
        .service
        .create_instance(spec)
        .expect("create_instance must accept a valid spec");
    let run = events.until_finished(&op, crate::CREATE_TIMEOUT);
    let stages = run.stage_names();
    eprintln!("postgres e2e create stages: {stages:?}");

    // Real progress samples that reach the catalog size: the HTTPS download and
    // its SHA-256 verification both really ran, whatever happens in extraction.
    let downloads = run.downloads();
    eprintln!(
        "postgres e2e download samples: {} (last {} of {} bytes)",
        downloads.len(),
        downloads.last().map(|p| p.bytes_done).unwrap_or(0),
        downloads.last().map(|p| p.bytes_total).unwrap_or(0)
    );
    assert!(
        !downloads.is_empty(),
        "the download must report real progress samples"
    );
    assert!(
        downloads.last().is_some_and(|p| p.is_complete()),
        "the bytes must have fully arrived: {:?}",
        downloads.last()
    );
    assert!(
        stages.contains(&"verifying"),
        "reaching the expected byte count must move to Verifying: {stages:?}"
    );

    if let Err(err) = &run.result {
        panic!(
            "create failed after a complete download: [{}] {}; stages {stages:?}",
            err.code, err.message
        );
    }

    // The download path succeeded: assert the whole stage order.
    assert_eq!(stages.first(), Some(&"queued"), "stages: {stages:?}");
    assert_stage_order(&stages, DOWNLOAD_STAGES);
    assert_eq!(stages.last(), Some(&"done"), "stages: {stages:?}");
    assert_eq!(run.finished_count(), 1, "exactly one OperationFinished");
    let extra = events.drain_for(Duration::from_secs(2));
    assert_eq!(
        extra
            .iter()
            .filter(|event| matches!(
                event,
                dblocal_core::service::ServiceEvent::OperationFinished { id, .. } if *id == op
            ))
            .count(),
        0,
        "no second OperationFinished may arrive"
    );

    let instance = harness.instance("pg-e2e").expect("the created row");
    assert_eq!(instance.status(), InstanceStatus::Stopped);
    cache::seed_cache_from(&harness.paths, EngineKey::Postgresql, &version);
    let delete = harness.delete(&instance.id().clone(), DeleteMode::PurgeData);
    super::expect_ok(&delete, "cleanup delete");
}

/// The whole lifecycle after an installed runtime (`F-RTM-03` AC3).
#[test]
#[ignore = "e2e: network + several hundred MB of disk; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn postgres_happy_path_from_catalog_to_archive() {
    super::require_e2e();

    let harness = Harness::new("dblocal-e2e-pg-");
    let version = newest_version(&harness.service, EngineKey::Postgresql);
    assert!(
        cache::extract_archive_to_runtime(
            &harness.paths,
            EngineKey::Postgresql,
            &version,
            PG_ARCHIVE
        ),
        "this test needs the PostgreSQL archive cached in ~/.cache/dblocal-dev"
    );
    let port = ports::reserve();
    let spec = newest_spec(&harness.service, "pg-e2e", EngineKey::Postgresql, port);

    // The runtime is already installed, so create skips download/verify/extract
    // and goes straight to initialising (`F-RTM-03` AC3).
    let run = harness.create(spec);
    super::expect_ok(&run, "create");
    let stages = run.stage_names();
    eprintln!("postgres e2e create stages (runtime already installed): {stages:?}");
    assert!(
        !stages.contains(&"downloading"),
        "an installed runtime must skip the download: {stages:?}"
    );
    assert!(
        stages.contains(&"initializing"),
        "an installed runtime still initialises: {stages:?}"
    );
    assert_eq!(stages.last(), Some(&"done"), "stages: {stages:?}");
    assert_eq!(run.finished_count(), 1, "exactly one OperationFinished");

    let instance = harness.instance("pg-e2e").expect("the created row");
    let id = instance.id().clone();
    assert_eq!(instance.status(), InstanceStatus::Stopped);

    // Start, then talk SQL.
    let started = std::time::Instant::now();
    let start_run = harness.start(&id);
    super::expect_ok(&start_run, "start");
    eprintln!(
        "postgres e2e start to healthy: {:.2}s",
        started.elapsed().as_secs_f64()
    );
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    harness.wait_listening(&id, Duration::from_secs(30));

    let uri = harness.uri(&id);
    let client = PgClient::connect(uri.expose(), Duration::from_secs(10))
        .unwrap_or_else(|err| panic!("connect with the URI from connection_uri: {err}"));
    assert_eq!(client.query_one("SELECT 1").expect("SELECT 1"), 1);
    client
        .batch(
            "CREATE TABLE e2e_persist (id integer PRIMARY KEY, note text NOT NULL);
             INSERT INTO e2e_persist (id, note) VALUES (1, 'survives a restart');",
        )
        .expect("create table and insert");
    assert_eq!(
        client
            .query_text("SELECT note FROM e2e_persist WHERE id = 1")
            .expect("read the inserted row"),
        "survives a restart"
    );

    // Stop: the port must be released.
    let stop_run = harness.stop(&id);
    super::expect_ok(&stop_run, "stop");
    assert_eq!(
        harness
            .instance_by_id(&id)
            .expect("row after stop")
            .status(),
        InstanceStatus::Stopped
    );
    assert!(
        wait_tcp_gone(port, Duration::from_secs(20)),
        "port {port} must be released after stop"
    );

    // Start again and read the row: persistence.
    let start_run = harness.start(&id);
    super::expect_ok(&start_run, "restart");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    harness.wait_listening(&id, Duration::from_secs(30));
    let client = PgClient::connect(harness.uri(&id).expose(), Duration::from_secs(10))
        .expect("connect after restart");
    assert_eq!(
        client
            .query_text("SELECT note FROM e2e_persist WHERE id = 1")
            .expect("read after restart"),
        "survives a restart",
        "the row must survive a stop/start cycle"
    );

    // Delete with ArchiveData: data moved, row gone, InstanceRemoved emitted.
    let delete_run = harness.delete(&id, DeleteMode::ArchiveData);
    super::expect_ok(&delete_run, "delete with ArchiveData");
    let archives = harness.service.list_archives().expect("list_archives");
    assert_eq!(archives.len(), 1, "one archive must be recorded");
    let archive = &archives[0];
    eprintln!(
        "postgres e2e archive {} size {} bytes at {}",
        archive.id(),
        archive.size_bytes(),
        archive.path().display()
    );
    assert!(
        archive.size_bytes() > 0,
        "the archive must be listed with a non-zero size"
    );
    assert!(
        dir_size(archive.path()) > 0,
        "the archived data must not be empty"
    );
    assert!(
        archive.path().join("PG_VERSION").is_file(),
        "the PostgreSQL data must be inside the archive at {}",
        archive.path().display()
    );
    assert_eq!(archive.instance_name().as_str(), "pg-e2e");
    harness.wait_row_gone(&id, Duration::from_secs(20));
    assert!(
        delete_run.saw_removed(&id),
        "InstanceRemoved must be emitted for the deleted instance"
    );
    assert!(
        harness.view("pg-e2e").is_none(),
        "the instance must be gone from list_instances"
    );

    // Purge the archive.
    let archive_id = archive.id().to_owned();
    let archive_path = archive.path().to_path_buf();
    harness
        .service
        .purge_archive(&archive_id)
        .expect("purge_archive");
    assert!(
        harness.service.list_archives().expect("list").is_empty(),
        "the archive record must be gone after purge"
    );
    assert!(
        !archive_path.exists(),
        "the archived directory must be removed by purge"
    );

    // No engine may survive the test.
    assert!(
        proc::engines_under(harness.home.path()).is_empty(),
        "no engine process may be left: {:?}",
        proc::engines_under(harness.home.path())
    );
}

/// Asserts `expected` appears in `actual` in order (as a subsequence).
fn assert_stage_order(actual: &[&'static str], expected: &[&'static str]) {
    let mut cursor = 0;
    for wanted in expected {
        let Some(found) = actual[cursor..].iter().position(|stage| stage == wanted) else {
            panic!("stage {wanted} missing or out of order in {actual:?}, expected {expected:?}");
        };
        cursor += found + 1;
    }
}
