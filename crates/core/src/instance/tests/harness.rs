//! Shared test harness: scripted adapter, probes, spawn and a service builder.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use tempfile::TempDir;

use crate::engine::{Credential, EngineAdapter, Health, InitContext, RunContext};
use crate::error::Result;
use crate::model::{
    AuthMode, CredentialBackend, CredentialRef, EngineKey, Instance, InstanceId, InstanceSpec,
    InstanceStatus, Port, StartFailure,
};
use crate::paths::DblPaths;
use crate::process::identity::ProcessIdentity;
use crate::process::probe::{ProcessObservation, ProcessProbe};
use crate::process::terminate::TerminateOutcome;
use crate::secret::SecretString;
use crate::secrets::{BackendKind, FakeBackend, SecretBackend};
use crate::service::{DblService, LocalService, ServiceEvent, Terminator};
use crate::store::Store;

/// Where a scripted create/start operation should fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailAt {
    None,
    Initialize,
    Health,
    Shutdown,
}

/// Adapter whose behaviour the test scripts.
#[derive(Debug)]
pub struct ScriptedAdapter {
    engine: EngineKey,
    pub fail_at: Mutex<FailAt>,
    pub health_ready: AtomicBool,
    pub init_calls: AtomicUsize,
    pub health_calls: AtomicUsize,
    pub shutdown_calls: AtomicUsize,
    /// Output the adapter should return from `diagnose_start_failure`.
    pub marker: Mutex<Option<String>>,
}

impl ScriptedAdapter {
    pub fn new(engine: EngineKey) -> Self {
        Self {
            engine,
            fail_at: Mutex::new(FailAt::None),
            health_ready: AtomicBool::new(true),
            init_calls: AtomicUsize::new(0),
            health_calls: AtomicUsize::new(0),
            shutdown_calls: AtomicUsize::new(0),
            marker: Mutex::new(None),
        }
    }

    pub fn failing(engine: EngineKey, at: FailAt) -> Self {
        let adapter = Self::new(engine);
        *adapter.fail_at.lock().unwrap() = at;
        adapter
    }

    fn fail_at(&self) -> FailAt {
        *self.fail_at.lock().unwrap()
    }
}

impl EngineAdapter for ScriptedAdapter {
    fn key(&self) -> EngineKey {
        self.engine
    }

    fn initialize(&self, ctx: &InitContext) -> Result<()> {
        self.init_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_at() == FailAt::Initialize {
            return Err(crate::DblError::Engine {
                reason: "initdb failed on purpose".to_owned(),
            });
        }
        // Really write a data directory so rollback has something to remove.
        std::fs::create_dir_all(&ctx.data_dir)?;
        std::fs::write(ctx.data_dir.join("PG_VERSION"), b"18\n")?;
        Ok(())
    }

    fn spawn_command(&self, _ctx: &RunContext) -> std::process::Command {
        // `true` exists everywhere on Unix and exits instantly; tests never run
        // this for real because they inject a fake spawn.
        std::process::Command::new("true")
    }

    fn health_check(&self, _ctx: &RunContext) -> Result<Health> {
        self.health_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_at() == FailAt::Health {
            return Err(crate::DblError::Engine {
                reason: "health probe failed on purpose".to_owned(),
            });
        }
        if self.health_ready.load(Ordering::SeqCst) {
            Ok(Health::Ready)
        } else {
            Ok(Health::NotReady("starting".to_owned()))
        }
    }

    fn shutdown(&self, _ctx: &RunContext) -> Result<()> {
        self.shutdown_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_at() == FailAt::Shutdown {
            return Err(crate::DblError::Engine {
                reason: "shutdown failed on purpose".to_owned(),
            });
        }
        Ok(())
    }

    fn connection_uri(
        &self,
        ctx: &RunContext,
        credential: &Credential,
        _format: crate::service::UriFormat,
    ) -> SecretString {
        SecretString::new(format!(
            "{}://{}:{}@127.0.0.1:{}/db",
            self.engine.as_str(),
            credential.username,
            credential.password.expose(),
            ctx.port.get()
        ))
    }

    fn diagnose_start_failure(&self, _output: &str, _exit_code: Option<i32>) -> StartFailure {
        match self.marker.lock().unwrap().as_deref() {
            Some("port_in_use") => StartFailure::PortInUse,
            Some("permission_denied") => StartFailure::PermissionDenied,
            Some("data_dir_version_mismatch") => StartFailure::DataDirVersionMismatch,
            Some("binary_missing") => StartFailure::BinaryMissing,
            _ => StartFailure::Unknown,
        }
    }
}

/// A fake process table shared by the probe and the fake terminator.
#[derive(Debug, Default)]
pub struct FakeProcs {
    table: Mutex<HashMap<u32, ProcessObservation>>,
    next_pid: AtomicUsize,
    pub kills: AtomicUsize,
    spawns: AtomicUsize,
}

impl FakeProcs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a live process and returns its identity.
    pub fn spawn(&self, exe: &str) -> ProcessIdentity {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        let pid = u32::try_from(self.next_pid.fetch_add(1, Ordering::SeqCst) + 40_000).unwrap();
        let started_at =
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + u64::from(pid));
        self.table.lock().unwrap().insert(
            pid,
            ProcessObservation::Found {
                exe: Some(PathBuf::from(exe)),
                started_at: Some(started_at),
            },
        );
        ProcessIdentity::new(pid, exe, started_at).unwrap()
    }

    pub fn kill(&self, pid: u32) {
        self.table.lock().unwrap().remove(&pid);
    }

    /// How many spawns have been recorded, for the two-service race test.
    pub fn spawned_count(&self) -> usize {
        self.spawns.load(Ordering::SeqCst)
    }
}

impl ProcessProbe for FakeProcs {
    fn observe(&self, pid: u32) -> ProcessObservation {
        self.table
            .lock()
            .unwrap()
            .get(&pid)
            .cloned()
            .unwrap_or(ProcessObservation::Missing)
    }
}

/// Terminator that removes the process from the fake table.
pub struct FakeTerminator {
    procs: Arc<FakeProcs>,
    pub forced: AtomicBool,
    /// How many terminate calls were made, for assertions.
    pub calls: AtomicUsize,
}

impl FakeTerminator {
    pub fn new(procs: Arc<FakeProcs>) -> Self {
        Self {
            procs,
            forced: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        }
    }
}

impl Terminator for FakeTerminator {
    fn terminate(&self, identity: &ProcessIdentity, _grace: Duration) -> Result<TerminateOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.procs.kills.fetch_add(1, Ordering::SeqCst);
        if self.procs.observe(identity.pid()) == ProcessObservation::Missing {
            return Ok(TerminateOutcome::NotRunning);
        }
        self.procs.kill(identity.pid());
        if self.forced.load(Ordering::SeqCst) {
            Ok(TerminateOutcome::Forced)
        } else {
            Ok(TerminateOutcome::Graceful)
        }
    }
}

/// Secret storage backed by in-memory fakes for both backends, so tests never
/// touch the developer keychain.
pub struct FakeSecrets {
    pub keychain: FakeBackend,
    pub file: FakeBackend,
    pub keychain_available: AtomicBool,
}

impl Default for FakeSecrets {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeSecrets {
    pub fn new() -> Self {
        Self {
            keychain: FakeBackend::new(CredentialBackend::Keychain),
            file: FakeBackend::new(CredentialBackend::File),
            keychain_available: AtomicBool::new(false),
        }
    }

    fn backend(&self, kind: BackendKind) -> &FakeBackend {
        match kind {
            CredentialBackend::Keychain => &self.keychain,
            CredentialBackend::File => &self.file,
        }
    }
}

impl super::super::service::SecretAccess for FakeSecrets {
    fn set(&self, reference: &CredentialRef, secret: &SecretString) -> Result<BackendKind> {
        if self.keychain_available.load(Ordering::SeqCst) {
            self.keychain.set(reference, secret)?;
            return Ok(CredentialBackend::Keychain);
        }
        self.file.set(reference, secret)?;
        Ok(CredentialBackend::File)
    }

    fn get(&self, reference: &CredentialRef) -> Result<SecretString> {
        self.backend(reference.backend()).get(reference)
    }

    fn delete(&self, reference: &CredentialRef) -> Result<()> {
        // Both backends, like the real store; absence is not an error.
        for backend in [&self.keychain, &self.file] {
            match backend.delete(reference) {
                Ok(()) | Err(crate::DblError::NotFound { .. }) => {}
                Err(crate::DblError::Unsupported { .. }) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }
}

/// A fully wired service over a temp home plus every fake it uses.
pub struct Harness {
    pub dir: TempDir,
    pub paths: DblPaths,
    pub adapter: Arc<ScriptedAdapter>,
    pub procs: Arc<FakeProcs>,
    pub terminator: Arc<FakeTerminator>,
    pub secrets: Arc<FakeSecrets>,
    pub service: LocalService,
    pub events: Mutex<Vec<ServiceEvent>>,
    /// Ports the injected probe reports as busy.
    pub busy_ports: Arc<Mutex<Vec<u16>>>,
}

impl Harness {
    /// Builds a harness whose adapter is `engine`, with every port free.
    pub fn new(engine: EngineKey) -> Self {
        Self::with_adapter(Arc::new(ScriptedAdapter::new(engine)), engine)
    }

    /// Builds a harness that injects `adapter`.
    pub fn with_adapter(adapter: Arc<ScriptedAdapter>, engine: EngineKey) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = DblPaths::with_root(dir.path());
        let procs = Arc::new(FakeProcs::new());
        let terminator = Arc::new(FakeTerminator::new(Arc::clone(&procs)));
        let secrets = Arc::new(FakeSecrets::new());
        let busy_ports = Arc::new(Mutex::new(Vec::new()));

        let adapter_for_service = Arc::clone(&adapter);
        let procs_for_spawn = Arc::clone(&procs);
        let busy_for_probe = Arc::clone(&busy_ports);
        let options = ServiceOptions {
            adapter_factory: Some(Arc::new(move |key| {
                if key == adapter_for_service.key() {
                    Box::new(ScriptedAdapterClone(Arc::clone(&adapter_for_service)))
                } else {
                    Box::new(ScriptedAdapter::new(key))
                }
            })),
            probe: Some(Arc::clone(&procs) as Arc<dyn ProcessProbe + Send + Sync>),
            terminator: Some(Arc::clone(&terminator) as Arc<dyn Terminator>),
            secrets: Some(Arc::clone(&secrets) as Arc<dyn super::super::service::SecretAccess>),
            port_free: Some(Arc::new(move |port: u16| {
                !busy_for_probe.lock().unwrap().contains(&port)
            })),
            spawn: Some(Arc::new(move |_command, _log| {
                Ok(procs_for_spawn.spawn("/fake/engine/bin/postgres"))
            })),
            start_timeout: Some(Duration::from_millis(400)),
            status_poll: Some(Duration::from_millis(60)),
            os: Some(crate::platform::Os::Linux),
            arch: Some(crate::platform::Arch::X86_64),
            host_glibc: Some((2, 39)),
            ..ServiceOptions::default()
        };

        // Pre-install the runtime so create skips the download path
        // (F-RTM-03 AC3). The download path has its own test.
        for (engine_key, version) in [(EngineKey::Postgresql, "18.7"), (EngineKey::Valkey, "18.7")]
        {
            if engine_key == engine {
                let dir = paths
                    .runtime_dir(engine_key, version, crate::platform::Arch::X86_64)
                    .expect("runtime dir");
                std::fs::create_dir_all(dir.join("bin")).expect("runtime bin");
            }
        }

        let service = LocalService::new(paths.clone(), options).expect("service");
        let events = Mutex::new(Vec::new());

        Self {
            dir,
            paths,
            adapter,
            procs,
            terminator,
            secrets,
            service,
            events,
            busy_ports,
        }
    }

    /// A spec on a reserved test port.
    pub fn spec(&self, name: &str, engine: EngineKey, port: u16) -> InstanceSpec {
        InstanceSpec::new(
            crate::model::InstanceName::new(name).expect("name"),
            engine,
            "18.7",
            Some(Port::new(port).expect("port")),
            AuthMode::Password,
        )
        .expect("spec")
    }

    /// Subscribes *before* an operation starts, so no event is missed.
    pub fn receiver(&self) -> tokio::sync::broadcast::Receiver<ServiceEvent> {
        self.service.subscribe()
    }

    /// Waits for one operation to finish, returning every event it produced.
    pub fn run(&self, op: crate::service::OperationId) -> Vec<ServiceEvent> {
        let mut receiver = self.receiver();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut collected = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                panic!("operation {op} did not finish in time");
            }
            match receiver.blocking_recv() {
                Ok(event) => {
                    let finished = matches!(
                        &event,
                        ServiceEvent::OperationFinished { id, .. } if id == &op
                    );
                    collected.push(event);
                    if finished {
                        return collected;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event channel closed before {op} finished")
                }
            }
        }
    }

    /// Waits for `op` on an existing receiver, returning every event it produced.
    pub fn run_on(
        &self,
        receiver: &mut tokio::sync::broadcast::Receiver<ServiceEvent>,
        op: crate::service::OperationId,
    ) -> Vec<ServiceEvent> {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut collected = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "operation {op} did not finish in time"
            );
            match receiver.blocking_recv() {
                Ok(event) => {
                    let finished =
                        matches!(&event, ServiceEvent::OperationFinished { id, .. } if id == &op);
                    collected.push(event);
                    if finished {
                        return collected;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event channel closed before {op} finished")
                }
            }
        }
    }

    /// Waits until `predicate` holds for the stored instance.
    pub fn wait_for_status(&self, id: &InstanceId, status: InstanceStatus) -> Instance {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(instance) = self.instance(id)
                && instance.status() == status
            {
                return instance;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "instance {id} never reached {status:?}; last was {:?}",
                self.instance(id).map(|i| i.status())
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn instance(&self, id: &InstanceId) -> Option<Instance> {
        let store = Store::open(&self.paths).expect("store");
        store.get_instance(id).expect("get")
    }

    pub fn instances(&self) -> Vec<Instance> {
        let store = Store::open(&self.paths).expect("store");
        store.list_instances().expect("list")
    }

    pub fn store(&self) -> Store {
        Store::open(&self.paths).expect("store")
    }

    /// Creates an instance through the service and waits for it to settle.
    pub fn create(&self, spec: InstanceSpec) -> Vec<ServiceEvent> {
        let op = self.service.create_instance(spec).expect("create");
        self.run(op)
    }

    /// Number of instances currently watched for live logs.
    pub fn watched_logs(&self) -> usize {
        self.service.watched_logs()
    }

    /// The stored status of the named instance, if it exists.
    pub fn instance_status(&self, name: &str) -> Option<InstanceStatus> {
        self.instances()
            .into_iter()
            .find(|instance| instance.name().as_str() == name)
            .map(|instance| instance.status())
    }

    /// Builds a harness with `tweak` applied to the service options.
    ///
    /// The runtime is pre-installed like [`Harness::new`]; a test that wants the
    /// download path removes it first with [`Harness::uninstall_runtime`].
    pub fn with_options(engine: EngineKey, tweak: impl FnOnce(&mut ServiceOptions)) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = DblPaths::with_root(dir.path());
        let procs = Arc::new(FakeProcs::new());
        let terminator = Arc::new(FakeTerminator::new(Arc::clone(&procs)));
        let secrets = Arc::new(FakeSecrets::new());
        let busy_ports = Arc::new(Mutex::new(Vec::new()));
        let adapter = Arc::new(ScriptedAdapter::new(engine));

        let adapter_for_service = Arc::clone(&adapter);
        let procs_for_spawn = Arc::clone(&procs);
        let busy_for_probe = Arc::clone(&busy_ports);
        let mut options = ServiceOptions {
            adapter_factory: Some(Arc::new(move |_key| {
                Box::new(ScriptedAdapterClone(Arc::clone(&adapter_for_service)))
                    as Box<dyn crate::engine::EngineAdapter>
            })),
            probe: Some(Arc::clone(&procs) as Arc<dyn ProcessProbe + Send + Sync>),
            terminator: Some(Arc::clone(&terminator) as Arc<dyn Terminator>),
            secrets: Some(Arc::clone(&secrets) as Arc<dyn super::super::service::SecretAccess>),
            port_free: Some(Arc::new(move |port: u16| {
                !busy_for_probe.lock().unwrap().contains(&port)
            })),
            spawn: Some(Arc::new(move |_command, _log| {
                Ok(procs_for_spawn.spawn("/fake/engine/bin/postgres"))
            })),
            start_timeout: Some(Duration::from_millis(400)),
            status_poll: Some(Duration::from_millis(60)),
            os: Some(crate::platform::Os::Linux),
            arch: Some(crate::platform::Arch::X86_64),
            host_glibc: Some((2, 39)),
            ..ServiceOptions::default()
        };
        tweak(&mut options);

        let runtime_dir = paths
            .runtime_dir(engine, "18.7", crate::platform::Arch::X86_64)
            .expect("runtime dir");
        std::fs::create_dir_all(runtime_dir.join("bin")).expect("runtime bin");

        let service = LocalService::new(paths.clone(), options).expect("service");
        Harness {
            dir,
            paths,
            adapter,
            procs,
            terminator,
            secrets,
            service,
            events: Mutex::new(Vec::new()),
            busy_ports,
        }
    }

    /// Removes the pre-installed runtime so create runs the download pipeline.
    pub fn uninstall_runtime(&self, engine: EngineKey) {
        let dir = self
            .paths
            .runtime_dir(engine, "18.7", crate::platform::Arch::X86_64)
            .expect("runtime dir");
        let _ = std::fs::remove_dir_all(dir);
    }
}

pub use crate::service::{RuntimeSource, ServiceOptions};

/// A second service over the same home, sharing the process table so the race
/// test can count spawns.
pub fn second_service(paths: &DblPaths, procs: Arc<FakeProcs>) -> Harness {
    build_over(paths, procs)
}

/// A fresh service over an existing home, sharing the process table so a live
/// engine recorded before the "restart" is still visible afterwards.
pub fn restart(paths: &DblPaths, procs: Arc<FakeProcs>) -> Harness {
    build_over(paths, procs)
}

/// Builds a harness whose state tree is `paths` and whose process table is
/// `procs`.
fn build_over(paths: &DblPaths, procs: Arc<FakeProcs>) -> Harness {
    let adapter = Arc::new(ScriptedAdapter::new(EngineKey::Postgresql));
    let terminator = Arc::new(FakeTerminator::new(Arc::clone(&procs)));
    let secrets = Arc::new(FakeSecrets::new());
    let busy_ports = Arc::new(Mutex::new(Vec::new()));

    let adapter_for_service = Arc::clone(&adapter);
    let procs_for_spawn = Arc::clone(&procs);
    let busy_for_probe = Arc::clone(&busy_ports);
    let options = ServiceOptions {
        adapter_factory: Some(Arc::new(move |_key| {
            Box::new(ScriptedAdapterClone(Arc::clone(&adapter_for_service)))
                as Box<dyn crate::engine::EngineAdapter>
        })),
        probe: Some(Arc::clone(&procs) as Arc<dyn ProcessProbe + Send + Sync>),
        terminator: Some(Arc::clone(&terminator) as Arc<dyn Terminator>),
        secrets: Some(Arc::clone(&secrets) as Arc<dyn super::super::service::SecretAccess>),
        port_free: Some(Arc::new(move |port: u16| {
            !busy_for_probe.lock().unwrap().contains(&port)
        })),
        spawn: Some(Arc::new(move |_command, _log| {
            Ok(procs_for_spawn.spawn("/fake/engine/bin/postgres"))
        })),
        start_timeout: Some(Duration::from_millis(400)),
        status_poll: Some(Duration::from_millis(60)),
        os: Some(crate::platform::Os::Linux),
        arch: Some(crate::platform::Arch::X86_64),
        host_glibc: Some((2, 39)),
        ..ServiceOptions::default()
    };

    let service = LocalService::new(paths.clone(), options).expect("service");
    Harness {
        dir: tempfile::tempdir().expect("tempdir"),
        paths: paths.clone(),
        adapter,
        procs,
        terminator,
        secrets,
        service,
        events: Mutex::new(Vec::new()),
        busy_ports,
    }
}

/// Clone wrapper so the factory can hand out the same scripted adapter behind a
/// `Box<dyn EngineAdapter>`.
struct ScriptedAdapterClone(Arc<ScriptedAdapter>);

impl EngineAdapter for ScriptedAdapterClone {
    fn key(&self) -> EngineKey {
        self.0.key()
    }
    fn initialize(&self, ctx: &InitContext) -> Result<()> {
        self.0.initialize(ctx)
    }
    fn spawn_command(&self, ctx: &RunContext) -> std::process::Command {
        self.0.spawn_command(ctx)
    }
    fn health_check(&self, ctx: &RunContext) -> Result<Health> {
        self.0.health_check(ctx)
    }
    fn shutdown(&self, ctx: &RunContext) -> Result<()> {
        self.0.shutdown(ctx)
    }
    fn connection_uri(
        &self,
        ctx: &RunContext,
        credential: &Credential,
        format: crate::service::UriFormat,
    ) -> SecretString {
        self.0.connection_uri(ctx, credential, format)
    }
    fn diagnose_start_failure(&self, output: &str, exit_code: Option<i32>) -> StartFailure {
        self.0.diagnose_start_failure(output, exit_code)
    }
}

/// Filename of the archived data directory, for assertions.
pub fn archive_entries(paths: &DblPaths) -> Vec<PathBuf> {
    let dir = paths.archive_dir();
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

/// Reserves a test port in 20000-29999.
pub fn test_port() -> u16 {
    crate::ports::test_support::reserve_test_port().get()
}

/// Reads the last `n` events of a finished operation of a kind.
pub fn stages(events: &[ServiceEvent]) -> Vec<&'static str> {
    events
        .iter()
        .filter_map(|event| match event {
            ServiceEvent::OperationProgress { stage, .. } => Some(stage.as_str()),
            _ => None,
        })
        .collect()
}

pub fn finished_result(
    events: &[ServiceEvent],
    op: &crate::service::OperationId,
) -> Option<std::result::Result<(), crate::service::UserFacingError>> {
    events.iter().find_map(|event| match event {
        ServiceEvent::OperationFinished { id, result } if id == op => Some(result.clone()),
        _ => None,
    })
}

#[allow(dead_code)]
fn _p(_: &Path) {}
