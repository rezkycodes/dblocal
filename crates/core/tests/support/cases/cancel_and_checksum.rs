//! Case 5 — cancel and failure leave nothing behind.
//!
//! 1. Cancel a real HTTPS download after the first progress sample: no row, no
//!    data directory, no credential, no `.part`, no runtime recorded.
//! 2. Create with a deliberately wrong SHA-256 (a copy of the shipped catalog
//!    entry): `ChecksumMismatch`, and again nothing left behind.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use dblocal_core::error::{CODE_CANCELLED, CODE_CHECKSUM_MISMATCH};
use dblocal_core::model::{EngineKey, InstanceName};
use dblocal_core::paths::DblPaths;
use dblocal_core::service::{
    DblService, LocalService, OperationStage, ServiceEvent, ServiceOptions,
};

use crate::support::env::TempHome;
use crate::support::harness::{self, FileOnlySecrets, newest_version};
use crate::support::{events, part_files, ports};

#[test]
#[ignore = "e2e: network + several hundred MB of disk; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn cancelling_a_real_download_leaves_nothing_behind() {
    super::require_e2e();

    let home = TempHome::new("dblocal-e2e-cancel-");
    let paths = home.paths();
    let service = harness::build(&paths);
    let version = newest_version(&service, EngineKey::Postgresql);
    let port = ports::reserve();
    let spec =
        crate::support::harness::newest_spec(&service, "pg-cancel", EngineKey::Postgresql, port);

    let mut stream = events::Events::new(service.subscribe());
    let op = service.create_instance(spec).expect("create_instance");

    // Wait for the first real progress sample, then cancel immediately.
    let first = stream.until(
        Duration::from_secs(120),
        "the first DownloadProgress sample",
        |event| matches!(event, ServiceEvent::DownloadProgress { id, .. } if *id == op),
    );
    eprintln!(
        "cancel: observed {} event(s) before cancelling: {:?}",
        first.len(),
        events::describe(&first)
    );
    let cancel_at = Instant::now();
    service
        .cancel_operation(&op)
        .expect("cancel_operation after the first download sample");

    // Keep collecting on the same stream: it saw every event, including the
    // first sample.
    let mut run = crate::support::events::OpRun {
        op: op.clone(),
        events: first,
        result: Ok(()),
        stage_at: Vec::new(),
    };
    let tail = stream.until_finished(&op, crate::STOP_TIMEOUT);
    run.events.extend(tail.events);
    run.result = tail.result;
    eprintln!(
        "cancel finished in {:.2}s: {:?}, stages {:?}",
        cancel_at.elapsed().as_secs_f64(),
        run.result,
        run.stage_names()
    );

    let err = super::expect_err(&run, "cancel mid-download");
    assert_eq!(
        err.code, CODE_CANCELLED,
        "a cancelled create reports DBL-7001"
    );
    assert!(
        run.stage_names().contains(&"failed"),
        "a cancelled operation reports Failed: {:?}",
        run.stage_names()
    );
    assert_eq!(run.finished_count(), 1, "exactly one OperationFinished");

    // Nothing may be left behind.
    let name = InstanceName::new("pg-cancel").expect("name");
    assert!(
        service.list_instances().expect("list_instances").is_empty(),
        "a cancelled create must not leave an instance row"
    );
    assert!(
        harness::row_if_present(&paths, &dblocal_core::model::InstanceId::new()).is_none(),
        "no row can exist"
    );
    assert!(
        !paths.instance_dir(&name).exists(),
        "no instance directory may remain: {}",
        paths.instance_dir(&name).display()
    );
    assert!(
        !paths.data_dir(&name).exists(),
        "no data directory may remain: {}",
        paths.data_dir(&name).display()
    );
    assert!(
        crate::support::security::credential_files(&paths).is_empty(),
        "no credential file may remain: {:?}",
        crate::support::security::credential_files(&paths)
    );
    let parts = part_files(paths.root());
    assert!(
        parts.is_empty(),
        "a cancelled download must remove its .part file: {parts:?}"
    );
    assert!(
        service.list_runtimes().expect("list_runtimes").is_empty(),
        "a cancelled create must not record a runtime"
    );
    let runtime = paths
        .runtime_dir(
            EngineKey::Postgresql,
            &version,
            dblocal_core::platform::Arch::current(),
        )
        .expect("runtime path");
    assert!(
        !runtime.join("bin").is_dir(),
        "no half-installed runtime may be finalised: {}",
        runtime.display()
    );
    eprintln!(
        "cancel: no row, no data dir, no credential, no .part, no runtime recorded (runtime dir absent: {})",
        !runtime.exists()
    );
}

#[test]
#[ignore = "e2e: network + several hundred MB of disk; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn a_wrong_checksum_fails_cleanly() {
    super::require_e2e();

    let home = TempHome::new("dblocal-e2e-checksum-");
    let paths = home.paths();

    // A copy of the shipped catalog entry with one flipped hex digit in the
    // digest. Nothing else changes: same URL, same size, same format.
    let snapshot = dblocal_core::catalog::load_bundled_snapshot().expect("bundled catalog");
    let artifact = snapshot
        .snapshot()
        .artifacts_for(
            dblocal_core::platform::Os::current(),
            dblocal_core::platform::Arch::current(),
        )
        .into_iter()
        .filter(|artifact| artifact.engine() == EngineKey::Postgresql)
        .max_by(|left, right| left.version().cmp(right.version()))
        .expect("a PostgreSQL artifact for this host");
    let mut sha = artifact.sha256().to_owned();
    let last = sha.pop().expect("64 hex characters");
    sha.push(if last == '0' { '1' } else { '0' });
    assert_ne!(sha, artifact.sha256(), "the digest must actually differ");
    eprintln!(
        "checksum test: copying catalog entry {} ({} bytes) with a deliberately wrong sha256",
        artifact.id(),
        artifact.size_bytes()
    );

    let source = dblocal_core::service::RuntimeSource {
        id: artifact.id().to_owned(),
        url: artifact.url().to_owned(),
        sha256: sha.clone(),
        size_bytes: artifact.size_bytes(),
        format: artifact.format(),
        strip_prefix: artifact.strip_prefix().map(str::to_owned),
        engine: artifact.engine(),
        version: artifact.version().to_owned(),
        expected_binaries: Vec::new(),
    };
    let options = ServiceOptions {
        secrets: Some(Arc::new(FileOnlySecrets::new(&paths))),
        runtime_sources: vec![source],
        ..ServiceOptions::default()
    };
    let service = LocalService::new(paths.clone(), options).expect("LocalService::new");
    let port = ports::reserve();
    let spec =
        crate::support::harness::newest_spec(&service, "pg-badsha", EngineKey::Postgresql, port);
    let mut stream = events::Events::new(service.subscribe());
    let op = service.create_instance(spec).expect("create_instance");
    let run = stream.until_finished(&op, crate::CREATE_TIMEOUT);
    eprintln!("checksum create stages: {:?}", run.stage_names());
    let err = super::expect_err(&run, "create with a wrong sha256");
    eprintln!("checksum create failed: [{}] {}", err.code, err.message);
    assert_eq!(
        err.code, CODE_CHECKSUM_MISMATCH,
        "a wrong digest reports DBL-2001"
    );
    assert!(
        !run.stage_names().contains(&"extracting"),
        "a bad digest must never claim to extract: {:?}",
        run.stage_names()
    );
    assert_eq!(run.finished_count(), 1, "exactly one OperationFinished");

    let name = InstanceName::new("pg-badsha").expect("name");
    assert!(
        service.list_instances().expect("list_instances").is_empty(),
        "a failed create must not leave an instance row"
    );
    assert!(
        !paths.instance_dir(&name).exists(),
        "no instance directory may remain"
    );
    assert!(
        crate::support::security::credential_files(&paths).is_empty(),
        "no credential may remain"
    );
    assert!(
        part_files(paths.root()).is_empty(),
        "a checksum mismatch deletes the .part file: {:?}",
        part_files(paths.root())
    );
    assert!(
        service.list_runtimes().expect("list_runtimes").is_empty(),
        "no runtime may be recorded"
    );
    let runtime = paths
        .runtime_dir(
            EngineKey::Postgresql,
            artifact.version(),
            dblocal_core::platform::Arch::current(),
        )
        .expect("runtime path");
    assert!(
        !runtime.join("bin").is_dir(),
        "a bad digest must not finalise a runtime"
    );
    drop(service);
    let _ = DblPaths::with_root(home.path());
    let _ = OperationStage::Done;
}
