//! Service construction, lifecycle helpers and disk/timing reporting.
//!
//! The only thing replaced is the credential backend: `SecretStore`'s write path
//! prefers the OS keychain, and an e2e run must never touch the developer's real
//! keychain. [`FileOnlySecrets`] therefore routes the *production*
//! [`FileBackend`] for `paths` and reports `File` as the backend, which is
//! exactly the fallback `F-CRD-01` AC2 documents — the same code, minus the
//! keychain attempt.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dblocal_core::error::Result;
use dblocal_core::model::{
    CredentialBackend, CredentialRef, DeleteMode, EngineKey, Instance, InstanceId, InstanceSpec,
    InstanceStatus, Port,
};
use dblocal_core::paths::DblPaths;
use dblocal_core::secret::SecretString;
use dblocal_core::secrets::{BackendKind, FileBackend, SecretBackend};
use dblocal_core::service::{DblService, InstanceView, LocalService, ServiceOptions, UriFormat};
use dblocal_core::store::Store;

use super::env::TempHome;
use super::events::{Events, OpRun};

/// The production `0600` file backend, without the keychain attempt.
pub struct FileOnlySecrets {
    file: FileBackend,
}

impl FileOnlySecrets {
    /// Routes to the file backend under `paths`.
    pub fn new(paths: &DblPaths) -> Self {
        Self {
            file: FileBackend::from_paths(paths),
        }
    }
}

impl dblocal_core::service::SecretAccess for FileOnlySecrets {
    fn set(&self, reference: &CredentialRef, secret: &SecretString) -> Result<BackendKind> {
        self.file.set(reference, secret)?;
        Ok(CredentialBackend::File)
    }

    fn get(&self, reference: &CredentialRef) -> Result<SecretString> {
        self.file.get(reference)
    }

    fn delete(&self, reference: &CredentialRef) -> Result<()> {
        match self.file.delete(reference) {
            Ok(()) => Ok(()),
            Err(dblocal_core::DblError::NotFound { .. }) => Ok(()),
            Err(err) => Err(err),
        }
    }
}

/// Options for a real-service run: production adapters, spawn, probe, terminator,
/// download policy and catalog, with the file-only credential backend.
pub fn real_options(paths: &DblPaths) -> ServiceOptions {
    ServiceOptions {
        secrets: Some(std::sync::Arc::new(FileOnlySecrets::new(paths))),
        ..ServiceOptions::default()
    }
}

/// Builds a real `LocalService` over `paths`.
pub fn build(paths: &DblPaths) -> LocalService {
    LocalService::new(paths.clone(), real_options(paths))
        .expect("LocalService::new over a temp home")
}

/// A temp home plus one real service over it.
pub struct Harness {
    /// Temp home; dropping it kills every engine and removes the tree.
    pub home: TempHome,
    /// State paths inside the temp home.
    pub paths: DblPaths,
    /// The service under test.
    pub service: LocalService,
}

impl Harness {
    /// Creates the temp home and the first service.
    pub fn new(prefix: &str) -> Self {
        let home = TempHome::new(prefix);
        let paths = home.paths();
        let service = build(&paths);
        Self {
            home,
            paths,
            service,
        }
    }

    /// A fresh event stream. Call **before** the operation, or its first stages
    /// are lost to the broadcast.
    pub fn events(&self) -> Events {
        Events::new(self.service.subscribe())
    }

    /// Creates an instance and waits for its operation to finish.
    pub fn create(&self, spec: InstanceSpec) -> OpRun {
        let mut events = self.events();
        let op = self.service.create_instance(spec).expect("create_instance");
        events.until_finished(&op, super::super::CREATE_TIMEOUT)
    }

    /// Starts an instance and waits for its operation to finish.
    pub fn start(&self, id: &InstanceId) -> OpRun {
        let mut events = self.events();
        let op = self.service.start_instance(id).expect("start_instance");
        events.until_finished(&op, super::super::START_TIMEOUT)
    }

    /// Stops an instance and waits for its operation to finish.
    pub fn stop(&self, id: &InstanceId) -> OpRun {
        let mut events = self.events();
        let op = self.service.stop_instance(id).expect("stop_instance");
        events.until_finished(&op, super::super::STOP_TIMEOUT)
    }

    /// Deletes an instance and waits for its operation to finish.
    pub fn delete(&self, id: &InstanceId, mode: DeleteMode) -> OpRun {
        let mut events = self.events();
        let op = self
            .service
            .delete_instance(id, mode)
            .expect("delete_instance");
        events.until_finished(&op, super::super::DELETE_TIMEOUT)
    }

    /// The view of the instance called `name`, if it exists.
    pub fn view(&self, name: &str) -> Option<InstanceView> {
        self.service
            .list_instances()
            .expect("list_instances")
            .into_iter()
            .find(|view| view.name == name)
    }

    /// The instance called `name`, read from the store.
    pub fn instance(&self, name: &str) -> Option<Instance> {
        self.store()
            .list_instances()
            .expect("list_instances")
            .into_iter()
            .find(|instance| instance.name().as_str() == name)
    }

    /// The id of the instance called `name`.
    pub fn list_id(&self, name: &str) -> Option<InstanceId> {
        self.instance(name).map(|instance| instance.id().clone())
    }

    /// The instance a view points at, read from the store.
    pub fn instance_by_id(&self, id: &InstanceId) -> Option<Instance> {
        self.store().get_instance(id).expect("get_instance")
    }

    /// A separate store handle over the same home, for row-level assertions.
    pub fn store(&self) -> Store {
        Store::open(&self.paths).expect("open the state store")
    }

    /// Waits until the instance's stored status is `status`.
    pub fn wait_status(
        &self,
        id: &InstanceId,
        status: InstanceStatus,
        timeout: Duration,
    ) -> Instance {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(instance) = self.instance_by_id(id)
                && instance.status() == status
            {
                return instance;
            }
            assert!(
                Instant::now() < deadline,
                "instance {id} never reached {status:?}; last was {:?}",
                self.instance_by_id(id).map(|i| i.status())
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Waits until the row for `id` is gone.
    pub fn wait_row_gone(&self, id: &InstanceId, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.instance_by_id(id).is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the row for {id} was never removed");
    }

    /// The connection URI the service would hand to a client.
    pub fn uri(&self, id: &InstanceId) -> SecretString {
        self.service
            .connection_uri(id, UriFormat::Uri)
            .expect("connection_uri")
    }

    /// Waits until the instance answers on its port at the TCP level.
    pub fn wait_listening(&self, id: &InstanceId, timeout: Duration) -> u16 {
        let instance = self.instance_by_id(id).expect("instance row");
        let port = instance.port().get();
        assert!(
            super::clients::wait_tcp(port, timeout),
            "port {port} never accepted a connection (status {:?})",
            instance.status()
        );
        port
    }
}

/// Reads one instance row straight from the store, by id.
///
/// The tests that drop the whole service need a row without a service handle.
pub fn row(paths: &DblPaths, id: &InstanceId) -> Instance {
    row_if_present(paths, id).unwrap_or_else(|| panic!("no row for {id}"))
}

/// Reads one instance row, returning `None` when it does not exist.
pub fn row_if_present(paths: &DblPaths, id: &InstanceId) -> Option<Instance> {
    Store::open(paths)
        .expect("open the state store")
        .get_instance(id)
        .expect("get_instance")
}

/// The newest version of `engine` the catalog offers for this host.
pub fn newest_version(service: &LocalService, engine: EngineKey) -> String {
    let view = service.catalog().expect("catalog");
    let entry = view
        .engines
        .iter()
        .find(|entry| entry.engine == engine)
        .unwrap_or_else(|| panic!("{} missing from the catalog", engine.display_name()));
    assert!(
        entry.available,
        "{} is not available on this platform: {:?}",
        engine.display_name(),
        entry.unavailable_reason
    );
    entry
        .versions
        .first()
        .unwrap_or_else(|| panic!("{} has no installable version", engine.display_name()))
        .version
        .clone()
}

/// Builds a spec for the newest available version of `engine`.
pub fn newest_spec(
    service: &LocalService,
    name: &str,
    engine: EngineKey,
    port: u16,
) -> InstanceSpec {
    let version = newest_version(service, engine);
    InstanceSpec::new(
        dblocal_core::model::InstanceName::new(name).expect("instance name"),
        engine,
        version,
        Some(Port::new(port).expect("port")),
        dblocal_core::model::AuthMode::Password,
    )
    .expect("spec")
}

/// Total size of a directory tree.
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total = total.saturating_add(dir_size(&entry.path()));
        } else {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

/// Temporary files with a `.part` suffix anywhere under `root`.
pub fn part_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_part_files(root, &mut out);
    out
}

fn collect_part_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "runtimes" {
                continue;
            }
            collect_part_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("part") {
            out.push(path);
        }
    }
}

/// Everything on disk that mentions `needle`, as `path:line` hits.
///
/// Reads files in bounded chunks so a multi-megabyte archive is never loaded
/// whole; binary files are still scanned because SQLite pages are binary.
pub fn grep_tree(root: &Path, needle: &str, skip: &[&Path]) -> Vec<String> {
    let mut hits = Vec::new();
    grep_dir(root, needle, skip, &mut hits);
    hits
}

fn grep_dir(dir: &Path, needle: &str, skip: &[&Path], hits: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if skip.contains(&path.as_path()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            grep_dir(&path, needle, skip, hits);
            continue;
        }
        if !meta.is_file() || meta.len() > 256 * 1024 * 1024 {
            continue;
        }
        if file_contains(&path, needle) {
            hits.push(path.display().to_string());
        }
    }
}

fn file_contains(path: &Path, needle: &str) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let needle = needle.as_bytes();
    if needle.is_empty() {
        return false;
    }
    let mut carry: Vec<u8> = Vec::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let Ok(read) = file.read(&mut buffer) else {
            return false;
        };
        if read == 0 {
            return false;
        }
        let mut window = std::mem::take(&mut carry);
        window.extend_from_slice(&buffer[..read]);
        if window.windows(needle.len()).any(|slice| slice == needle) {
            return true;
        }
        let keep = needle.len().saturating_sub(1).min(window.len());
        carry = window[window.len() - keep..].to_vec();
    }
}

/// Wall-clock measurements collected for the reporting test.
#[derive(Debug, Default)]
pub struct Timings {
    /// Create: download + verify + extract.
    pub download: Option<Duration>,
    /// Create: engine initialisation after the runtime was ready.
    pub initialize: Option<Duration>,
    /// Create: the whole operation.
    pub create_total: Option<Duration>,
    /// Start: spawn to healthy.
    pub start: Option<Duration>,
    /// Stop: request to stopped.
    pub stop: Option<Duration>,
    /// Installed runtime size on disk.
    pub runtime_bytes: u64,
    /// Size of a freshly initialised, empty data directory.
    pub empty_data_bytes: u64,
}

impl Timings {
    /// Renders the numbers as a small table for the test log.
    pub fn render(&self, label: &str) -> String {
        let show = |value: Option<Duration>| match value {
            Some(value) => format!("{:.2}s", value.as_secs_f64()),
            None => "-".to_owned(),
        };
        format!(
            "{label}: download={} initialize={} create_total={} start_to_healthy={} stop={} runtime={:.1}MiB empty_data={:.1}MiB",
            show(self.download),
            show(self.initialize),
            show(self.create_total),
            show(self.start),
            show(self.stop),
            self.runtime_bytes as f64 / (1024.0 * 1024.0),
            self.empty_data_bytes as f64 / (1024.0 * 1024.0),
        )
    }
}
