//! Removing an installed runtime (`F-RTM-04`): what must hold when the GUI and
//! the CLI share one home, when files cannot be deleted, and when the store and
//! the disk disagree.
//!
//! Every runtime file name comes from [`crate::engine::layout::expected_binaries`]
//! (through [`super::runtime_fixture`]), so the fixtures hold on Unix and Windows.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

use crate::download::UrlPolicy;
use crate::engine::layout::{expected_binaries, runtime_is_installed};
use crate::model::{ArchiveFormat, DeleteMode, EngineKey, InstanceStatus, Runtime, RuntimeState};
use crate::service::{DblService, ServiceOptions};

use super::harness::{Harness, RuntimeSource, test_port};
use super::runtime_fixture::{FIXTURE_ARCH, FIXTURE_VERSION, fixture_runtime_dir};

pub(super) const ENGINE: EngineKey = EngineKey::Postgresql;
/// The id `create` records for the harness runtime.
pub(super) const RUNTIME_ID: &str = "postgresql-18.7-x86_64";
/// The lock file every process agrees on for that runtime, relative to the home.
const LOCK_FILE: &str = "locks/runtime-postgresql-18.7-x86_64.lock";

/// Records the harness's pre-planted runtime exactly as `create` records it.
pub(super) fn record_row(h: &Harness) -> PathBuf {
    let dir = fixture_runtime_dir(&h.paths, ENGINE);
    record_row_at(h, &dir);
    dir
}

/// Records a runtime row that claims to live at `path`.
fn record_row_at(h: &Harness, path: &Path) {
    h.store()
        .add_runtime(&Runtime::new(
            RUNTIME_ID,
            ENGINE,
            FIXTURE_VERSION,
            FIXTURE_ARCH,
            path.to_path_buf(),
            0,
            SystemTime::now(),
            RuntimeState::Installed,
        ))
        .expect("add runtime row");
}

pub(super) fn row_exists(h: &Harness) -> bool {
    h.store()
        .list_runtimes()
        .expect("runtimes")
        .iter()
        .any(|runtime| runtime.id() == RUNTIME_ID)
}

/// Opens the runtime lock file the way another DBLocal process does.
pub(super) fn other_process_lock(h: &Harness) -> File {
    let path = h.paths.root().join(LOCK_FILE);
    std::fs::create_dir_all(path.parent().expect("lock parent")).expect("locks dir");
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .expect("open lock file")
}

/// A zip holding exactly the binaries extraction expects, and its SHA-256.
fn runtime_zip() -> (Vec<u8>, String) {
    use zip::write::SimpleFileOptions;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().unix_permissions(0o755);
    for name in expected_binaries(ENGINE) {
        writer.start_file(name, options).expect("start entry");
        writer
            .write_all(b"fake engine binary")
            .expect("write entry");
    }
    let bytes = writer.finish().expect("finish zip").into_inner();
    let hex = sha2::Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    (bytes, hex)
}

/// A harness whose `create` can download the runtime again over loopback HTTP.
///
/// The returned runtime keeps the server alive; the counter says how many times
/// the archive was fetched.
pub(super) fn downloadable() -> (Harness, Arc<AtomicUsize>, tokio::runtime::Runtime) {
    downloadable_with(|_| {})
}

pub(super) fn downloadable_with(
    tweak: impl FnOnce(&mut ServiceOptions),
) -> (Harness, Arc<AtomicUsize>, tokio::runtime::Runtime) {
    served(Arc::new(Semaphore::new(Semaphore::MAX_PERMITS)), tweak)
}

/// Like [`downloadable`], but the server answers only once a permit is added.
pub(super) fn downloadable_gated() -> (Harness, Arc<Semaphore>, tokio::runtime::Runtime) {
    let gate = Arc::new(Semaphore::new(0));
    let (h, _hits, server) = served(Arc::clone(&gate), |_| {});
    (h, gate, server)
}

fn served(
    gate: Arc<Semaphore>,
    tweak: impl FnOnce(&mut ServiceOptions),
) -> (Harness, Arc<AtomicUsize>, tokio::runtime::Runtime) {
    let server = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt");
    let (body, sha256) = runtime_zip();
    let size_bytes = body.len() as u64;
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let url = server.block_on(async move {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let body = body.clone();
                let counter = Arc::clone(&counter);
                let gate = Arc::clone(&gate);
                tokio::spawn(async move {
                    let mut head = [0u8; 1024];
                    let _ = socket.read(&mut head).await;
                    let _permit = gate.acquire().await.expect("gate");
                    counter.fetch_add(1, Ordering::SeqCst);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        format!("http://{addr}/postgresql.zip")
    });
    let source = RuntimeSource {
        id: "postgresql-18.7-linux-x86_64".to_owned(),
        url,
        sha256,
        size_bytes,
        format: ArchiveFormat::Zip,
        strip_prefix: None,
        engine: ENGINE,
        version: FIXTURE_VERSION.to_owned(),
        expected_binaries: expected_binaries(ENGINE),
    };
    let h = Harness::with_options(ENGINE, |options| {
        options.runtime_sources = vec![source];
        options.download_policy = Some(UrlPolicy::allow_loopback_http_for_tests());
        tweak(options);
    });
    (h, hits, server)
}

pub(super) fn delete_all_instances(h: &Harness, mode: DeleteMode) {
    for instance in h.instances() {
        let id = instance.id().clone();
        let (_op, run) = h.run_call(|| h.service.delete_instance(&id, mode));
        assert!(matches!(run.result(), Some(Ok(()))), "{:?}", run.result());
    }
}

/// Everything left under `runtimes/<engine>`.
pub(super) fn engine_entries(h: &Harness) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            out.push(path.clone());
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                walk(&path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&h.paths.runtimes_dir().join(ENGINE.as_str()), &mut out);
    out
}

#[test]
fn removal_deletes_the_tree_the_row_and_the_emptied_directories() {
    let h = Harness::new(ENGINE);
    let dir = record_row(&h);

    h.service.remove_runtime(RUNTIME_ID).expect("remove");

    assert!(!dir.exists(), "the runtime tree is gone");
    assert!(!row_exists(&h), "the row is gone");
    assert!(
        engine_entries(&h).is_empty(),
        "nothing of that version is left behind: {:?}",
        engine_entries(&h)
    );
}

#[test]
fn removal_leaves_another_version_of_the_same_engine_alone() {
    let h = Harness::new(ENGINE);
    record_row(&h);
    let other = h
        .paths
        .runtime_dir(ENGINE, "17.2", FIXTURE_ARCH)
        .expect("runtime dir");
    super::runtime_fixture::plant_runtime_at(&other, ENGINE);

    h.service.remove_runtime(RUNTIME_ID).expect("remove");

    assert!(runtime_is_installed(&other, ENGINE), "17.2 is untouched");
}

#[test]
fn removal_sweeps_the_debris_of_an_install_that_crashed() {
    let h = Harness::new(ENGINE);
    let dir = record_row(&h);
    let staging = dir.with_file_name(".x86_64.extracting-00000000deadbeef");
    std::fs::create_dir_all(&staging).expect("staging");
    std::fs::write(staging.join("partial"), b"half").expect("partial file");

    h.service.remove_runtime(RUNTIME_ID).expect("remove");

    assert!(
        !staging.exists(),
        "the stale staging directory is reclaimed"
    );
}

#[test]
fn removal_is_refused_while_another_process_uses_the_runtime() {
    let h = Harness::new(ENGINE);
    let dir = record_row(&h);
    // A create in another process holds the lock shared while it installs and
    // initialises from the runtime; no instance row exists yet at this point.
    let other = other_process_lock(&h);
    other.lock_shared().expect("shared lock");

    let err = h.service.remove_runtime(RUNTIME_ID).expect_err("busy");

    assert_eq!(err.code(), "DBL-1007", "{err}");
    assert!(runtime_is_installed(&dir, ENGINE), "nothing was removed");
    assert!(row_exists(&h), "the row stays");

    drop(other);
    h.service.remove_runtime(RUNTIME_ID).expect("retry");
    assert!(!dir.exists());
}

#[test]
fn two_creates_can_share_a_runtime_that_is_in_use() {
    let h = Harness::new(ENGINE);
    record_row(&h);
    let other = other_process_lock(&h);
    other.lock_shared().expect("shared lock");

    h.create(h.spec("beside", ENGINE, test_port()));

    assert_eq!(h.instance_status("beside"), Some(InstanceStatus::Stopped));
}

#[test]
fn a_create_waits_while_another_process_removes_the_runtime() {
    let h = Harness::new(ENGINE);
    record_row(&h);
    let other = other_process_lock(&h);
    other.lock().expect("exclusive lock");

    let mut receiver = h.receiver();
    let op = h
        .service
        .create_instance(h.spec("waiting", ENGINE, test_port()))
        .expect("accepted");
    std::thread::sleep(Duration::from_millis(400));

    assert!(
        !h.service.is_operation_finished(&op),
        "the create must wait for the removal"
    );
    assert!(
        h.instances().is_empty(),
        "no row may claim the runtime while it is being removed"
    );
    assert_eq!(h.adapter.init_calls.load(Ordering::SeqCst), 0);

    drop(other);
    let run = h.run_on(&mut receiver, op);
    assert!(matches!(run.result(), Some(Ok(()))), "{:?}", run.result());
    assert_eq!(h.instance_status("waiting"), Some(InstanceStatus::Stopped));
}

#[test]
fn a_create_releases_the_runtime_when_it_finishes() {
    let h = Harness::new(ENGINE);
    h.create(h.spec("done", ENGINE, test_port()));

    let other = other_process_lock(&h);
    let deadline = Instant::now() + Duration::from_secs(10);
    while other.try_lock().is_err() {
        assert!(Instant::now() < deadline, "the create kept the lock");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn removal_is_refused_while_an_instance_uses_it_and_names_the_instances() {
    let h = Harness::new(ENGINE);
    h.create(h.spec("app-dev", ENGINE, test_port()));

    let err = h.service.remove_runtime(RUNTIME_ID).expect_err("in use");

    assert_eq!(err.code(), "DBL-1006", "{err}");
    assert!(err.to_string().contains("app-dev"), "{err}");
    assert!(row_exists(&h));
}

#[test]
fn a_row_pointing_at_instance_data_is_refused_and_nothing_is_deleted() {
    let h = Harness::new(ENGINE);
    let data = h.paths.root().join("instances").join("app").join("data");
    std::fs::create_dir_all(&data).expect("data dir");
    std::fs::write(data.join("PG_VERSION"), b"18\n").expect("data file");
    record_row_at(&h, &data);

    let err = h.service.remove_runtime(RUNTIME_ID).expect_err("refused");

    assert_eq!(err.code(), "DBL-2003", "{err}");
    assert!(data.join("PG_VERSION").is_file(), "instance data survives");
    assert!(row_exists(&h));
}

#[test]
fn a_row_pointing_outside_the_home_is_refused() {
    let h = Harness::new(ENGINE);
    let outside = tempfile::tempdir().expect("outside");
    std::fs::write(outside.path().join("keep"), b"keep").expect("file");
    record_row_at(&h, outside.path());

    let err = h.service.remove_runtime(RUNTIME_ID).expect_err("refused");

    assert_eq!(err.code(), "DBL-2003", "{err}");
    assert!(outside.path().join("keep").is_file());
}

#[cfg(unix)]
#[test]
fn a_symlinked_runtime_directory_is_refused_and_its_target_survives() {
    let h = Harness::new(ENGINE);
    let dir = fixture_runtime_dir(&h.paths, ENGINE);
    let outside = tempfile::tempdir().expect("outside");
    let target = outside.path().join("relocated");
    std::fs::rename(&dir, &target).expect("relocate");
    std::os::unix::fs::symlink(&target, &dir).expect("symlink");
    record_row_at(&h, &dir);

    let err = h.service.remove_runtime(RUNTIME_ID).expect_err("refused");

    assert_eq!(err.code(), "DBL-2003", "{err}");
    assert!(runtime_is_installed(&target, ENGINE));
    assert!(row_exists(&h));
}

#[cfg(unix)]
#[test]
fn a_symlinked_engine_directory_is_refused() {
    let h = Harness::new(ENGINE);
    let engine_dir = h.paths.runtimes_dir().join(ENGINE.as_str());
    let outside = tempfile::tempdir().expect("outside");
    let target = outside.path().join("engine");
    std::fs::rename(&engine_dir, &target).expect("relocate");
    std::os::unix::fs::symlink(&target, &engine_dir).expect("symlink");
    record_row_at(&h, &fixture_runtime_dir(&h.paths, ENGINE));

    let err = h.service.remove_runtime(RUNTIME_ID).expect_err("refused");

    assert_eq!(err.code(), "DBL-2003", "{err}");
    assert!(target.join("18.7").is_dir());
}

#[cfg(unix)]
#[test]
fn a_symlinked_locks_directory_never_writes_outside_the_home() {
    let h = Harness::new(ENGINE);
    record_row(&h);
    let outside = tempfile::tempdir().expect("outside");
    std::os::unix::fs::symlink(outside.path(), h.paths.root().join("locks")).expect("symlink");

    let err = h.service.remove_runtime(RUNTIME_ID).expect_err("refused");

    assert_eq!(err.code(), "DBL-2003", "{err}");
    assert_eq!(
        std::fs::read_dir(outside.path()).expect("read").count(),
        0,
        "no lock file may be created through the link"
    );
}

#[test]
fn removing_a_runtime_deleted_by_hand_drops_the_row() {
    let h = Harness::new(ENGINE);
    let dir = record_row(&h);
    std::fs::remove_dir_all(h.paths.runtimes_dir().join(ENGINE.as_str())).expect("hand delete");

    h.service.remove_runtime(RUNTIME_ID).expect("remove");

    assert!(!row_exists(&h));
    assert!(!dir.exists());
}

#[test]
fn a_create_installs_again_when_the_runtime_was_deleted_by_hand() {
    let (h, hits, _server) = downloadable();
    let dir = record_row(&h);
    std::fs::remove_dir_all(&dir).expect("hand delete");

    h.create(h.spec("again", ENGINE, test_port()));

    assert_eq!(hits.load(Ordering::SeqCst), 1, "downloaded once");
    assert!(runtime_is_installed(&dir, ENGINE));
    assert_eq!(h.instance_status("again"), Some(InstanceStatus::Stopped));
    assert_eq!(h.service.list_runtimes().expect("runtimes").len(), 1);
}

#[test]
fn archived_data_survives_removal_and_the_version_installs_again() {
    let (h, hits, _server) = downloadable();
    h.create(h.spec("kept", ENGINE, test_port()));
    delete_all_instances(&h, DeleteMode::ArchiveData);
    let archive = h.service.list_archives().expect("archives").remove(0);

    h.service.remove_runtime(RUNTIME_ID).expect("remove");

    assert!(archive.path().is_dir(), "the archived data is untouched");
    assert_eq!(h.service.list_archives().expect("archives").len(), 1);

    // Restoring means creating the same version again: it downloads once more.
    h.create(h.spec("restored", ENGINE, test_port()));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(h.instance_status("restored"), Some(InstanceStatus::Stopped));
}

#[test]
fn listing_reports_the_size_on_disk() {
    let h = Harness::new(ENGINE);
    let dir = record_row(&h);
    std::fs::write(dir.join("share.dat"), vec![0u8; 4096]).expect("payload");
    let binaries: u64 = expected_binaries(ENGINE)
        .iter()
        .map(|name| std::fs::metadata(dir.join(name)).expect("binary").len())
        .sum();

    let listed = h.service.list_runtimes().expect("runtimes");

    assert_eq!(listed[0].size_bytes(), 4096 + binaries);
}

/// A real permission failure, with no seam: the shape v0.2.1 turned into a
/// half-removed tree that the next create could neither use nor overwrite.
#[cfg(unix)]
#[test]
fn a_permission_failure_never_leaves_a_create_stuck() {
    use std::os::unix::fs::PermissionsExt;

    let (h, _hits, _server) = downloadable();
    let dir = record_row(&h);
    // Plant the sealed directory before the binaries: a filesystem that lists the
    // newest entry first then deletes the binaries before it hits the failure.
    std::fs::remove_dir_all(&dir).expect("empty the runtime");
    let sealed = dir.join("sealed");
    std::fs::create_dir_all(&sealed).expect("sealed dir");
    std::fs::write(sealed.join("held"), b"held").expect("held file");
    super::runtime_fixture::plant_runtime_at(&dir, ENGINE);
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o500)).expect("seal");
    // Root ignores the mode bits, so there is nothing to observe there.
    if std::fs::write(sealed.join("probe"), b"x").is_ok() {
        return;
    }

    let err = h.service.remove_runtime(RUNTIME_ID).expect_err("partial");

    assert!(
        row_exists(&h),
        "the row stays so the removal can be retried"
    );
    h.create(h.spec("after-failure", ENGINE, test_port()));
    assert!(runtime_is_installed(&dir, ENGINE));
    assert_eq!(err.code(), "DBL-3004", "{err}");

    // The user fixes the permission and retries: every leftover goes.
    delete_all_instances(&h, DeleteMode::PurgeData);
    for path in engine_entries(&h) {
        if path.file_name().is_some_and(|name| name == "sealed") {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("unseal");
        }
    }
    h.service.remove_runtime(RUNTIME_ID).expect("retry");
    assert!(engine_entries(&h).is_empty(), "{:?}", engine_entries(&h));
    assert!(!row_exists(&h));
}
