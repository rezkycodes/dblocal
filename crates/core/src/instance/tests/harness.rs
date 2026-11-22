//! Shared test harness: the service builder and assertions over the fakes in
//! [`super::harness_fakes`].

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;

use crate::engine::{Credential, EngineAdapter, Health, InitContext, RunContext};
use crate::error::Result;
use crate::model::{
    AuthMode, EngineKey, Instance, InstanceId, InstanceSpec, InstanceStatus, Port, StartFailure,
};
use crate::paths::DblPaths;
use crate::process::probe::ProcessProbe;
use crate::secret::SecretString;
use crate::service::{DblService, LocalService, ServiceEvent, Terminator};
use crate::store::Store;

pub use super::harness_fakes::{
    FailAt, FakeProcs, FakeSecrets, FakeTerminator, GatedProbe, ScriptedAdapter,
};

/// Terminal result of one operation, paired with the events the receiver saw.
///
/// `result` comes from the durable operation registry when the broadcast event
/// was missed under load, so callers never need to treat a missing
/// `OperationFinished` event as "maybe still running" when the registry says done.
#[derive(Clone, Debug)]
pub struct OperationRun {
    pub events: Vec<ServiceEvent>,
    pub result: Option<std::result::Result<(), crate::service::UserFacingError>>,
}

impl OperationRun {
    /// The durable terminal result.
    pub fn result(&self) -> Option<std::result::Result<(), crate::service::UserFacingError>> {
        self.result.clone()
    }

    /// Whether this run ended with a terminal error.
    pub fn is_err(&self) -> bool {
        matches!(self.result, Some(Err(_)))
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

    /// Subscribes, calls `start` (which returns an `OperationId`), then waits
    /// for that operation's terminal result.
    ///
    /// This is the safe way to observe an operation that may finish before the
    /// caller could subscribe: the receiver exists before the call, so no event
    /// is missed, and the durable registry result covers broadcast lag under load.
    pub fn run_call(
        &self,
        start: impl FnOnce() -> crate::Result<crate::service::OperationId>,
    ) -> (crate::service::OperationId, OperationRun) {
        let mut receiver = self.receiver();
        let op = start().expect("operation start");
        let run = self.run_on(&mut receiver, op.clone());
        (op, run)
    }

    /// Waits for `op` on an existing receiver, returning collected events and the
    /// durable terminal result.
    ///
    /// There is intentionally no `run(op)` that subscribes after the fact: that
    /// shape drops the `OperationFinished` broadcast when the operation wins the
    /// race, which made tests weaken their assertions under load. Use
    /// [`Self::run_call`] when starting an operation, or create a receiver before
    /// the operation starts and pass it here.
    pub fn run_on(
        &self,
        receiver: &mut tokio::sync::broadcast::Receiver<ServiceEvent>,
        op: crate::service::OperationId,
    ) -> OperationRun {
        let events = wait_for_finished(receiver, &op, || self.service.is_operation_finished(&op));
        let result = self
            .operation_result(&op)
            .or_else(|| finished_result(&events, &op));
        OperationRun { events, result }
    }

    /// Durable terminal result of `op`, when it finished inside the retained window.
    pub fn operation_result(
        &self,
        op: &crate::service::OperationId,
    ) -> Option<std::result::Result<(), crate::service::UserFacingError>> {
        self.service.operation_result(op)
    }

    /// Waits until `predicate` holds for the stored instance.
    pub fn wait_for_status(&self, id: &InstanceId, status: InstanceStatus) -> Instance {
        // Generous: the whole instance suite runs many services (each with a
        // two-worker runtime) in parallel, and a busy machine can delay a task's
        // first poll by seconds. The deadline only has to catch a genuine hang.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
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
        // Subscribe before the call: a create can finish so fast that a receiver
        // opened afterwards misses every event.
        let mut receiver = self.receiver();
        let op = self.service.create_instance(spec).expect("create");
        let run = self.run_on(&mut receiver, op.clone());
        if let Some(Err(err)) = run.result() {
            panic!(
                "create {op} failed: {err:?}; events={:?}",
                describe(&run.events)
            );
        }
        // Prove the row is durable before the caller looks it up: a create that
        // reports success while the row is invisible to a fresh connection is a
        // product defect, not a test artefact, and must be reported as such.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let service_view = self
                .service
                .list_instances()
                .map(|views| views.into_iter().map(|view| view.id).collect::<Vec<_>>());
            let fresh_view = self
                .instances()
                .into_iter()
                .map(|instance| instance.id().clone())
                .collect::<Vec<_>>();
            if !fresh_view.is_empty() {
                break;
            }
            let db = self.paths.state_db();
            let wal = db.with_extension("db-wal");
            let shm = db.with_extension("db-shm");
            let raw_rows: String = std::process::Command::new("sqlite3")
                .arg(&db)
                .arg("SELECT id,name,status FROM instances ORDER BY id; SELECT '---'; SELECT name FROM sqlite_master WHERE type='table' ORDER BY name;")
                .output()
                .map(|out| {
                    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
                    if stderr.is_empty() { stdout } else { format!("{stdout} stderr={stderr}") }
                })
                .unwrap_or_else(|err| format!("<sqlite3 unavailable: {err}>"));
            let db_len = std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0);
            let wal_len = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
            let shm_len = std::fs::metadata(&shm).map(|m| m.len()).unwrap_or(0);
            let deletes = crate::store::take_deletes_for_ids(&db);
            let decisions = super::super::reconcile::take_decisions(&self.paths.root());
            assert!(
                std::time::Instant::now() < deadline,
                "create {op} reported success but no row is listed; \
                 service_view={service_view:?} fresh_view={fresh_view:?} \
                 state_db={} exists={} db_len={db_len} wal_len={wal_len} shm_len={shm_len} \
                 raw=[{raw_rows}] deletes={deletes:#?} decisions={decisions:#?} events={:?}",
                db.display(),
                db.exists(),
                describe(&run.events)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        run.events
    }

    /// Number of instances currently watched for live logs.
    pub fn watched_logs(&self) -> usize {
        self.service.watched_logs()
    }

    /// Claims a transition from an explicit expected status (stale-claim tests).
    pub fn claim_for_test(
        &self,
        id: &InstanceId,
        expected: InstanceStatus,
        event: crate::model::InstanceEvent,
    ) -> crate::Result<Instance> {
        self.service.claim_for_test(id, expected, event)
    }

    /// Claims a raw status change like delete does.
    pub fn claim_status_for_test(
        &self,
        id: &InstanceId,
        expected: InstanceStatus,
        next: InstanceStatus,
    ) -> crate::Result<bool> {
        self.service.claim_status_for_test(id, expected, next)
    }

    /// Whether the service recorded a failure for `id`.
    pub fn has_failure_record(&self, id: &InstanceId) -> bool {
        self.service.has_failure_record(id)
    }

    /// Whether this harness's service has been told to stop.
    pub fn is_stopping(&self) -> bool {
        self.service.stopping_for_test()
    }

    /// Whether this harness's supervisor task is still looping.
    pub fn supervisor_alive(&self) -> bool {
        self.service.supervisor_alive_for_test()
    }

    /// Acquires the per-instance operation lock, for a test that must prove the
    /// supervisor skips a busy row.
    pub fn lock_instance(&self, id: &InstanceId) -> super::super::registry::InstanceLock {
        self.service
            .lock_instance_for_test(id)
            .expect("instance lock")
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
            auto_start: Some(false),
            ..ServiceOptions::default()
        };
        tweak(&mut options);

        let runtime_dir = paths
            .runtime_dir(engine, "18.7", crate::platform::Arch::X86_64)
            .expect("runtime dir");
        std::fs::create_dir_all(runtime_dir.join("bin")).expect("runtime bin");

        let service = LocalService::new(paths.clone(), options).expect("service");
        let startup_receiver = service.subscribe();
        service.start_background();
        drop(startup_receiver);
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
pub fn second_service(
    paths: &DblPaths,
    procs: Arc<FakeProcs>,
    secrets: Arc<FakeSecrets>,
) -> Harness {
    // The credential lives in the shared home, so a second service must read it
    // through the same store the first wrote it into. Giving it a fresh store
    // would make a start fail on a credential it "cannot find", which is a
    // harness artefact, not a defect.
    let terminator = Arc::new(FakeTerminator::new(Arc::clone(&procs)));
    let (harness, _startup) = build_over_shared(paths, procs, terminator, secrets);
    harness
}

/// A fresh service over an existing home, sharing the process table so a live
/// engine recorded before the "restart" is still visible afterwards.
pub fn restart(paths: &DblPaths, procs: Arc<FakeProcs>) -> Harness {
    let (harness, _startup) = build_over(paths, procs);
    harness
}

/// Restarts over the same home, **consuming** the old harness.
///
/// Dropping the old service stops its supervisor (which would otherwise observe
/// rows this test mutates by hand) and takes over its temp dir, so the store file
/// survives into the new service. Use this whenever a test must prove what
/// reconciliation, and only reconciliation, does.
pub fn restart_owned(old: Harness) -> Harness {
    restart_owned_with(old, |_| {})
}

/// `restart_owned`, running `prepare` on the home **after** the old service is
/// gone and **before** the new one starts.
///
/// This is the only safe order for a test that must prove what reconciliation
/// does: rows written while an older supervisor is still looping could be
/// witnessed as a death by that supervisor, which is a different (valid) path.
pub fn restart_owned_with(old: Harness, prepare: impl FnOnce(&DblPaths)) -> Harness {
    let Harness {
        dir,
        paths,
        procs,
        terminator,
        secrets,
        service,
        ..
    } = old;
    // Stop the old supervisor **and wait for it to exit** before the new service
    // reads the same home: a late tick would act on rows this test prepared by
    // hand, which is exactly what reconciliation is supposed to decide.
    drop(service);
    // Give the runtime one poll interval to abort the supervisor before we touch
    // the home. `LocalService::drop` sets `stopping`; the runtime shutdown then
    // aborts the parked task, so after this the old service is provably inert.
    std::thread::sleep(Duration::from_millis(120));
    prepare(&paths);
    let (harness, _startup) = build(
        dir,
        paths,
        procs,
        terminator,
        secrets,
        Duration::from_millis(60),
        None,
    );
    harness
}

/// A fresh service over an existing home that also shares the terminator and
/// secret backends of the original, so a test can inspect or script them after
/// the restart.
///
/// Subscribes **before** reconciliation starts, so startup events (notably
/// `InstanceRemoved` from an interrupted create) are observable. The receiver is
/// returned alongside the harness; callers that do not care can drop it.
pub fn restart_with(
    paths: &DblPaths,
    procs: Arc<FakeProcs>,
    terminator: Arc<FakeTerminator>,
    secrets: Arc<FakeSecrets>,
) -> (Harness, tokio::sync::broadcast::Receiver<ServiceEvent>) {
    build_over_shared(paths, procs, terminator, secrets)
}

/// A harness whose supervisor polls every `poll`, so a test can make a race
/// against a stop happen every time rather than occasionally.
pub fn fast_supervisor(poll: Duration) -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    let procs = Arc::new(FakeProcs::new());
    let terminator = Arc::new(FakeTerminator::new(Arc::clone(&procs)));
    let secrets = Arc::new(FakeSecrets::new());
    let (harness, _startup) = build(dir, paths, procs, terminator, secrets, poll, None);
    harness
}

/// A harness whose reconciliation and supervisor use `gated` as the probe, while
/// the fake spawn writes to `procs`.
///
/// The gate must be backed by the same table the spawn writes to, so the test
/// passes a `GatedProbe` wrapping a clone of its own table view. Here `procs` is
/// what the spawn uses and `gated` is what the supervisor validates through.
pub fn with_gated_probe(
    engine: EngineKey,
    procs: Arc<FakeProcs>,
    gated: Arc<GatedProbe>,
    poll: Duration,
) -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    let terminator = Arc::new(FakeTerminator::new(Arc::clone(&procs)));
    let secrets = Arc::new(FakeSecrets::new());
    let _ = engine;
    let (harness, _startup) = build(
        dir,
        paths,
        procs,
        terminator,
        secrets,
        poll,
        Some(gated as Arc<dyn ProcessProbe + Send + Sync>),
    );
    harness
}

/// Builds a harness whose state tree is `paths` and whose process table is
/// `procs`.
fn build_over(
    paths: &DblPaths,
    procs: Arc<FakeProcs>,
) -> (Harness, tokio::sync::broadcast::Receiver<ServiceEvent>) {
    let terminator = Arc::new(FakeTerminator::new(Arc::clone(&procs)));
    let secrets = Arc::new(FakeSecrets::new());
    build_over_shared(paths, procs, terminator, secrets)
}

fn build_over_shared(
    paths: &DblPaths,
    procs: Arc<FakeProcs>,
    terminator: Arc<FakeTerminator>,
    secrets: Arc<FakeSecrets>,
) -> (Harness, tokio::sync::broadcast::Receiver<ServiceEvent>) {
    build(
        tempfile::tempdir().expect("tempdir"),
        paths.clone(),
        procs,
        terminator,
        secrets,
        Duration::from_millis(60),
        None,
    )
}

/// The one constructor every harness variant funnels through.
///
/// `probe` overrides the process probe for the reconciliation and supervisor
/// paths (a gated probe, for a deterministic race); the fake spawn still uses
/// `procs` so identities and spawns stay in one table.
///
/// A receiver is subscribed **before** `start_background()` so startup
/// reconciliation events are observable. Every harness variant returns it; most
/// tests ignore it and subscribe their own receiver when they need one.
fn build(
    dir: TempDir,
    paths: DblPaths,
    procs: Arc<FakeProcs>,
    terminator: Arc<FakeTerminator>,
    secrets: Arc<FakeSecrets>,
    status_poll: Duration,
    probe: Option<Arc<dyn ProcessProbe + Send + Sync>>,
) -> (Harness, tokio::sync::broadcast::Receiver<ServiceEvent>) {
    let adapter = Arc::new(ScriptedAdapter::new(EngineKey::Postgresql));
    let busy_ports = Arc::new(Mutex::new(Vec::new()));

    let adapter_for_service = Arc::clone(&adapter);
    let procs_for_spawn = Arc::clone(&procs);
    let busy_for_probe = Arc::clone(&busy_ports);
    let options = ServiceOptions {
        adapter_factory: Some(Arc::new(move |_key| {
            Box::new(ScriptedAdapterClone(Arc::clone(&adapter_for_service)))
                as Box<dyn crate::engine::EngineAdapter>
        })),
        probe: probe.or_else(|| Some(Arc::clone(&procs) as Arc<dyn ProcessProbe + Send + Sync>)),
        terminator: Some(Arc::clone(&terminator) as Arc<dyn Terminator>),
        secrets: Some(Arc::clone(&secrets) as Arc<dyn super::super::service::SecretAccess>),
        port_free: Some(Arc::new(move |port: u16| {
            !busy_for_probe.lock().unwrap().contains(&port)
        })),
        spawn: Some(Arc::new(move |_command, _log| {
            Ok(procs_for_spawn.spawn("/fake/engine/bin/postgres"))
        })),
        start_timeout: Some(Duration::from_millis(400)),
        status_poll: Some(status_poll),
        os: Some(crate::platform::Os::Linux),
        arch: Some(crate::platform::Arch::X86_64),
        host_glibc: Some((2, 39)),
        auto_start: Some(false),
        ..ServiceOptions::default()
    };

    // Pre-install the runtime so create skips the download path (F-RTM-03 AC3).
    let runtime_dir = paths
        .runtime_dir(EngineKey::Postgresql, "18.7", crate::platform::Arch::X86_64)
        .expect("runtime dir");
    std::fs::create_dir_all(runtime_dir.join("bin")).expect("runtime bin");

    let service = LocalService::new(paths.clone(), options).expect("service");
    let startup_receiver = service.subscribe();
    service.start_background();
    let harness = Harness {
        dir,
        paths,
        adapter,
        procs,
        terminator,
        secrets,
        service,
        events: Mutex::new(Vec::new()),
        busy_ports,
    };
    (harness, startup_receiver)
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

/// Whether a `InstanceRemoved` for `id` appears in `events`.
pub fn removed_for(events: &[ServiceEvent], id: &InstanceId) -> bool {
    events
        .iter()
        .any(|event| matches!(event, ServiceEvent::InstanceRemoved { id: got } if got == id))
}

/// Whether a `StopForced` for `id` appears in `events`.
pub fn stop_forced_for(events: &[ServiceEvent], id: &InstanceId) -> bool {
    events
        .iter()
        .any(|event| matches!(event, ServiceEvent::StopForced { id: got } if got == id))
}

/// Counts `OperationFinished` events in `events`.
pub fn finished_count(events: &[ServiceEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, ServiceEvent::OperationFinished { .. }))
        .count()
}

/// Compact rendering used in an assertion message when an event order is wrong.
pub fn describe(events: &[ServiceEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| match event {
            ServiceEvent::OperationProgress { stage, .. } => {
                format!("Progress({})", stage.as_str())
            }
            ServiceEvent::InstanceChanged { view } => format!("Changed({})", view.status.as_str()),
            ServiceEvent::OperationFinished { result, .. } => match result {
                Ok(()) => "Finished(ok)".to_owned(),
                Err(err) => format!("Finished({})", err.code),
            },
            ServiceEvent::DownloadProgress { .. } => "Download".to_owned(),
            ServiceEvent::InstanceRemoved { .. } => "Removed".to_owned(),
            ServiceEvent::StopForced { .. } => "StopForced".to_owned(),
            _ => "Other".to_owned(),
        })
        .collect()
}

#[allow(dead_code)]
fn _p(_: &Path) {}

/// Collects events until `op` reports `OperationFinished`, with a hard deadline.
///
/// Uses `try_recv` plus a short sleep instead of `blocking_recv`: an operation
/// can finish before the receiver subscribes (the receiver is created after the
/// call in the two-service races), and `blocking_recv` would then wait forever
/// because the missed event is never re-delivered. Polling keeps the deadline
/// honest.
fn wait_for_finished(
    receiver: &mut tokio::sync::broadcast::Receiver<ServiceEvent>,
    op: &crate::service::OperationId,
    is_finished: impl Fn() -> bool,
) -> Vec<ServiceEvent> {
    // Generous: the whole instance suite runs many services (each with a
    // two-worker runtime) in parallel, and a busy machine can delay a task's
    // first poll by seconds. The deadline only has to catch a genuine hang.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let mut collected = Vec::new();
    loop {
        loop {
            match receiver.try_recv() {
                Ok(event) => {
                    let finished =
                        matches!(&event, ServiceEvent::OperationFinished { id, .. } if id == op);
                    collected.push(event);
                    if finished {
                        return collected;
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                    panic!("event channel closed before {op} finished")
                }
            }
        }
        // The operation may have finished before this receiver subscribed; its
        // `OperationFinished` is then lost to the broadcast. Completion is still
        // observable through the registry, so stop waiting once it reports done.
        if is_finished() {
            return collected;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "operation {op} did not finish in time; saw: {:?}",
            describe(&collected)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn row_gone_diagnostic(paths: &DblPaths, id: &InstanceId) -> String {
    let store = match Store::open(paths) {
        Ok(store) => store,
        Err(err) => return format!("store open failed: {err}"),
    };
    let row = match store.get_instance(id) {
        Ok(row) => row,
        Err(err) => return format!("store read failed: {err}"),
    };
    let Some(instance) = row else {
        return "row is gone".to_owned();
    };
    let instance_dir = paths.instance_dir(instance.name());
    let marker_path = instance_dir.join(super::super::delete::IN_FLIGHT_MARKER);
    let marker = std::fs::read_to_string(&marker_path)
        .map(|contents| contents.replace('\n', " | "))
        .unwrap_or_else(|err| format!("<unreadable: {err}>"));
    let dir_entries: Vec<String> = std::fs::read_dir(&instance_dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    format!(
        "status={:?} pid={:?} started_at={:?} exe={:?} instance={} instance_dir={} \
         marker_path={} marker=[{marker}] dir={dir_entries:?} db={}",
        instance.status(),
        instance.pid(),
        instance.process_started_at(),
        instance.process_exe(),
        instance.name().as_str(),
        instance_dir.display(),
        marker_path.display(),
        paths.state_db().display(),
    )
}

/// Waits until `id` is gone, with a deadline. Shared by recovery tests.
pub fn wait_until_row_gone(h: &Harness, id: &InstanceId, round: usize) {
    // Generous: the whole instance suite runs many services (each with a
    // two-worker runtime) in parallel, and a busy machine can delay a task's
    // first poll by seconds. The deadline only has to catch a genuine hang.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        if h.instance(id).is_none() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "round {round}: the row was never removed; {}",
            row_gone_diagnostic(&h.paths, id)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Waits until `id` is gone from the store, with a deadline.
///
/// Shared by the race tests: the winning operation may run on either service and
/// finish before the test can subscribe, so the observable end state (the row is
/// gone) is what the test waits for.
pub fn wait_for_row_gone(paths: &DblPaths, id: &InstanceId, round: usize) {
    let store = Store::open(paths).expect("store");
    // Generous: the whole instance suite runs many services (each with a
    // two-worker runtime) in parallel, and a busy machine can delay a task's
    // first poll by seconds. The deadline only has to catch a genuine hang.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        if store.get_instance(id).expect("get").is_none() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "round {round}: the row was never removed; {}",
            row_gone_diagnostic(paths, id)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Waits for an `InstanceRemoved` event for `id`, returning false on timeout.
pub fn wait_for_removed(
    receiver: &mut tokio::sync::broadcast::Receiver<ServiceEvent>,
    id: &InstanceId,
    timeout: Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match receiver.try_recv() {
            Ok(event) if removed_for(std::slice::from_ref(&event), id) => return true,
            Ok(_) => continue,
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return false,
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
