//! Removal failing half way, and the limits of waiting for another operation.
//!
//! A locked or unreadable file is simulated through the two file operations a
//! removal needs, so these hold on every platform and for every user.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::engine::layout::{expected_binaries, runtime_is_installed};
use crate::instance::runtime_files::{DiskRuntimeFiles, RuntimeFiles};
use crate::model::{DeleteMode, InstanceStatus};
use crate::service::{DblService, OperationStage, ServiceEvent};

use super::harness::{Harness, test_port};
use super::runtime_removal::{
    ENGINE, RUNTIME_ID, delete_all_instances, downloadable, downloadable_gated, downloadable_with,
    engine_entries, other_process_lock, record_row, row_exists,
};

/// Real file operations that can be told to fail like a file another program
/// holds open.
#[derive(Default)]
struct Faults {
    rename_fails: AtomicBool,
    delete_fails: AtomicBool,
}

fn in_use() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "file in use")
}

impl RuntimeFiles for Faults {
    fn set_aside(&self, from: &Path, to: &Path) -> io::Result<()> {
        if self.rename_fails.load(Ordering::SeqCst) {
            return Err(in_use());
        }
        DiskRuntimeFiles.set_aside(from, to)
    }

    fn delete(&self, path: &Path) -> io::Result<()> {
        if self.delete_fails.load(Ordering::SeqCst) {
            // Stop half way: one engine binary is gone, the rest remains.
            let first = path.join(&expected_binaries(ENGINE)[0]);
            let _ = std::fs::remove_file(first);
            return Err(in_use());
        }
        DiskRuntimeFiles.delete(path)
    }
}

fn faulty() -> (Harness, Arc<Faults>, tokio::runtime::Runtime) {
    let faults = Arc::new(Faults::default());
    let seam: Arc<dyn RuntimeFiles> = Arc::clone(&faults) as Arc<dyn RuntimeFiles>;
    let (h, _hits, server) = downloadable_with(move |options| {
        options.runtime_files = Some(seam);
    });
    (h, faults, server)
}

#[test]
fn a_locked_file_before_the_rename_leaves_the_runtime_intact_and_retryable() {
    let (h, faults, _server) = faulty();
    let dir = record_row(&h);
    faults.rename_fails.store(true, Ordering::SeqCst);

    let err = h.service.remove_runtime(RUNTIME_ID).expect_err("locked");

    assert_eq!(err.code(), "DBL-3004", "{err}");
    assert!(runtime_is_installed(&dir, ENGINE), "nothing was removed");
    assert!(row_exists(&h));

    faults.rename_fails.store(false, Ordering::SeqCst);
    h.service.remove_runtime(RUNTIME_ID).expect("retry");
    assert!(!dir.exists());
    assert!(!row_exists(&h));
}

#[test]
fn a_deletion_that_stops_half_way_never_blocks_the_next_create() {
    let (h, faults, _server) = faulty();
    let dir = record_row(&h);
    faults.delete_fails.store(true, Ordering::SeqCst);

    let err = h.service.remove_runtime(RUNTIME_ID).expect_err("partial");

    assert_eq!(err.code(), "DBL-3004", "{err}");
    assert!(
        !dir.exists(),
        "nothing half-removed sits at the install path"
    );
    assert!(
        row_exists(&h),
        "the row stays so the removal can be retried"
    );
    let leftover = h.service.list_runtimes().expect("runtimes")[0].size_bytes();
    assert!(leftover > 0, "the space still taken is still reported");

    h.create(h.spec("after-failure", ENGINE, test_port()));
    assert!(runtime_is_installed(&dir, ENGINE));
    assert_eq!(
        h.instance_status("after-failure"),
        Some(InstanceStatus::Stopped)
    );

    faults.delete_fails.store(false, Ordering::SeqCst);
    delete_all_instances(&h, DeleteMode::PurgeData);
    h.service.remove_runtime(RUNTIME_ID).expect("retry");
    assert!(engine_entries(&h).is_empty(), "{:?}", engine_entries(&h));
    assert!(!row_exists(&h));
}

#[test]
fn a_removal_can_be_retried_without_a_create_in_between() {
    let (h, faults, _server) = faulty();
    record_row(&h);
    faults.delete_fails.store(true, Ordering::SeqCst);
    h.service.remove_runtime(RUNTIME_ID).expect_err("partial");

    faults.delete_fails.store(false, Ordering::SeqCst);
    h.service.remove_runtime(RUNTIME_ID).expect("retry");

    assert!(engine_entries(&h).is_empty(), "{:?}", engine_entries(&h));
    assert!(!row_exists(&h));
}

#[test]
fn the_users_are_named_even_while_another_create_holds_the_runtime() {
    let (h, _hits, _server) = downloadable();
    h.create(h.spec("first", ENGINE, test_port()));
    let other = other_process_lock(&h);
    other.lock_shared().expect("shared lock");

    let err = h.service.remove_runtime(RUNTIME_ID).expect_err("in use");

    assert_eq!(err.code(), "DBL-1006", "{err}");
}

#[test]
fn a_create_gives_up_when_a_removal_never_finishes() {
    let (h, hits, _server) = downloadable_with(|options| {
        options.runtime_lock_wait = Some(Duration::from_millis(200));
    });
    let other = other_process_lock(&h);
    other.lock().expect("exclusive lock");

    let (_op, run) = h.run_call(|| {
        h.service
            .create_instance(h.spec("never", ENGINE, test_port()))
    });

    let failure = run.result().expect("finished").expect_err("busy");
    assert_eq!(failure.code, "DBL-1007");
    assert!(h.instances().is_empty(), "no row was written");
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[test]
fn a_create_waiting_for_a_removal_can_be_cancelled() {
    let (h, _hits, _server) = downloadable();
    let other = other_process_lock(&h);
    other.lock().expect("exclusive lock");
    let mut receiver = h.receiver();
    let op = h
        .service
        .create_instance(h.spec("cancelled", ENGINE, test_port()))
        .expect("accepted");

    h.service.cancel_operation(&op).expect("cancel");

    let run = h.run_on(&mut receiver, op);
    let failure = run.result().expect("finished").expect_err("cancelled");
    assert_eq!(failure.code, "DBL-7001");
    assert!(h.instances().is_empty(), "no row was written");
}

#[test]
fn a_create_holds_the_runtime_while_it_downloads() {
    let (h, gate, _server) = downloadable_gated();
    h.uninstall_runtime(ENGINE);
    let mut receiver = h.receiver();
    let op = h
        .service
        .create_instance(h.spec("downloading", ENGINE, test_port()))
        .expect("accepted");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match receiver.try_recv() {
            Ok(ServiceEvent::OperationProgress {
                stage: OperationStage::Downloading,
                ..
            }) => break,
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
        assert!(Instant::now() < deadline, "the download never started");
    }

    let other = other_process_lock(&h);
    assert!(other.try_lock().is_err(), "a removal must be shut out");
    assert!(other.try_lock_shared().is_ok(), "another create is not");
    drop(other);

    gate.add_permits(1);
    let run = h.run_on(&mut receiver, op);
    assert!(matches!(run.result(), Some(Ok(()))), "{:?}", run.result());
}
