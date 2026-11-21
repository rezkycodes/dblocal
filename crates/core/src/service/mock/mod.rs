//! In-memory [`DblService`] for the GUI shell and tests.
//!
//! Enabled by the `mock` feature. Operations run asynchronously and emit the
//! same event shapes the real service will produce. The mock never touches the
//! network or real engine processes.

#![cfg(feature = "mock")]

mod catalog;
mod clock;
mod ops;
mod uri;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use tokio::sync::broadcast;

use crate::DblError;
use crate::model::{
    ArchivedData, AuthMode, CatalogStatus, CredentialBackend, EngineKey, Instance, InstanceEvent,
    InstanceId, InstanceSpec, InstanceStatus, StartFailure,
};
use crate::platform::{Arch, Os};
use crate::secret::SecretString;
use crate::service::{CatalogView, FailureInfo, InstanceView, OperationId, ServiceEvent};

use clock::Deterministic;

const EVENT_CAPACITY: usize = 512;
const DEFAULT_STEP_DELAY_MS: u64 = 250;

/// A scripted failure the mock will inject into the next matching operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptedFailure {
    /// Instance create/start reports the port as already bound.
    PortInUse { port: u16 },
    /// Downloading the runtime fails mid-flight.
    DownloadFailed,
    /// Start times out and never reaches a healthy state.
    StartTimeout,
    /// The graceful stop is ignored and the stop escalates to a forced kill.
    StopNeedsForce,
    /// Create fails before the instance exists, so its row is rolled back.
    CreateRolledBack,
}

/// Controls simulated timing and deterministic randomness.
#[derive(Debug, Clone)]
pub struct MockConfig {
    seed: u64,
    step_delay: Duration,
    start_time: SystemTime,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            seed: 1,
            step_delay: Duration::from_millis(DEFAULT_STEP_DELAY_MS),
            start_time: SystemTime::UNIX_EPOCH,
        }
    }
}

impl MockConfig {
    /// Zero-delay, fixed-clock config for unit tests.
    pub fn deterministic() -> Self {
        Self {
            seed: 1,
            step_delay: Duration::from_millis(0),
            start_time: SystemTime::UNIX_EPOCH,
        }
    }

    /// Sets the RNG seed used for generated ids and passwords.
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Sets the delay between simulated operation steps.
    pub fn with_step_delay(mut self, delay: Duration) -> Self {
        self.step_delay = delay;
        self
    }

    /// Sets the base wall-clock time reported by the mock.
    pub fn with_start_time(mut self, start_time: SystemTime) -> Self {
        self.start_time = start_time;
        self
    }
}

#[derive(Debug, Clone)]
enum OpKind {
    RefreshCatalog,
    Create,
    Start,
    Stop,
    Delete,
}

#[derive(Debug, Clone)]
struct OpRecord {
    /// Kept for future per-kind handling; cancellation only uses the flag today.
    #[allow(dead_code)]
    kind: OpKind,
    cancelled: bool,
}

#[derive(Debug)]
struct Inner {
    step_delay: Duration,
    det: Deterministic,
    tx: broadcast::Sender<ServiceEvent>,
    instances: BTreeMap<InstanceId, Instance>,
    views: BTreeMap<InstanceId, InstanceView>,
    credentials: HashMap<InstanceId, CredentialBackend>,
    passwords: HashMap<InstanceId, SecretString>,
    logs: HashMap<InstanceId, VecDeque<String>>,
    watched: HashSet<InstanceId>,
    runtimes: BTreeSet<(EngineKey, String)>,
    archives: BTreeMap<String, ArchivedData>,
    operations: HashMap<OperationId, OpRecord>,
    catalog_status: CatalogStatus,
    next_op: u64,
    failures: VecDeque<ScriptedFailure>,
}

impl Inner {
    fn new(config: MockConfig) -> Self {
        let (tx, _) = broadcast::channel(EVENT_CAPACITY);
        let mut runtimes = BTreeSet::new();
        runtimes.insert((EngineKey::Postgresql, "17.4".to_owned()));
        runtimes.insert((EngineKey::Valkey, "8.1.0".to_owned()));
        Self {
            step_delay: config.step_delay,
            det: Deterministic::new(config.seed, config.start_time),
            tx,
            instances: BTreeMap::new(),
            views: BTreeMap::new(),
            credentials: HashMap::new(),
            passwords: HashMap::new(),
            logs: HashMap::new(),
            watched: HashSet::new(),
            runtimes,
            archives: BTreeMap::new(),
            operations: HashMap::new(),
            catalog_status: CatalogStatus::Fresh,
            next_op: 1,
            failures: VecDeque::new(),
        }
    }

    fn emit(&self, event: ServiceEvent) {
        let _ = self.tx.send(event);
    }

    fn alloc_op(&mut self, kind: OpKind) -> OperationId {
        let n = self.next_op;
        self.next_op += 1;
        let id = OperationId::new(format!("op-{n:04}"));
        self.operations.insert(
            id.clone(),
            OpRecord {
                kind,
                cancelled: false,
            },
        );
        id
    }

    fn take_failure<F: Fn(&ScriptedFailure) -> bool>(
        &mut self,
        pred: F,
    ) -> Option<ScriptedFailure> {
        self.failures
            .iter()
            .position(pred)
            .and_then(|ix| self.failures.remove(ix))
    }

    fn catalog_view(&self) -> CatalogView {
        catalog::view(
            self.catalog_status,
            Os::current(),
            Arch::current(),
            &self.runtimes,
        )
    }

    fn push_log(&mut self, id: &InstanceId, line: String) {
        let ring = self.logs.entry(id.clone()).or_default();
        if ring.len() >= crate::consts::LOG_TAIL_LINES {
            ring.pop_front();
        }
        ring.push_back(line.clone());
        if self.watched.contains(id) {
            self.emit(ServiceEvent::LogLines {
                id: id.clone(),
                lines: vec![line],
            });
        }
    }

    fn tail(&self, id: &InstanceId, count: usize) -> Vec<String> {
        self.logs
            .get(id)
            .map(|ring| {
                let skip = ring.len().saturating_sub(count);
                ring.iter().skip(skip).cloned().collect()
            })
            .unwrap_or_default()
    }

    fn rebuild_view(&self, id: &InstanceId) -> Option<InstanceView> {
        let inst = self.instances.get(id)?;
        let mut view = InstanceView::from_instance(inst);
        if let Some(backend) = self.credentials.get(id).copied() {
            view = view.with_credential_backend(backend);
        }
        if inst.status() == InstanceStatus::Failed
            && let Some(existing) = self.views.get(id).and_then(|v| v.failure.clone())
        {
            view = view.with_failure(existing);
        }
        Some(view)
    }

    fn set_failed(
        &mut self,
        id: &InstanceId,
        code: &str,
        message: String,
        diagnosis: StartFailure,
    ) {
        let tail = self.tail(id, crate::consts::LOG_TAIL_ON_FAILURE);
        if let Some(inst) = self.instances.get(id).cloned() {
            if let Ok(next) = inst.status().apply(InstanceEvent::StartFailed) {
                let updated = inst.with_status(next);
                self.instances.insert(id.clone(), updated);
            } else if inst.status() != InstanceStatus::Failed {
                // For creation/provisioning failures we move directly to Failed.
                let updated = inst.with_status(InstanceStatus::Failed);
                self.instances.insert(id.clone(), updated);
            }
        }
        if let Some(mut view) = self.rebuild_view(id) {
            view = view.with_failure(FailureInfo {
                code: code.to_owned(),
                message,
                diagnosis,
                log_tail: tail,
            });
            self.views.insert(id.clone(), view.clone());
            self.emit(ServiceEvent::InstanceChanged { view });
        }
    }

    /// Applies one state-machine event, returning whether it was legal.
    ///
    /// Every transition goes through [`InstanceStatus::apply`], so the mock
    /// cannot invent a state the real service could not reach.
    fn apply(&mut self, id: &InstanceId, event: InstanceEvent) -> bool {
        let Some(inst) = self.instances.get(id).cloned() else {
            return false;
        };
        match inst.status().apply(event) {
            Ok(next) => {
                self.instances.insert(id.clone(), inst.with_status(next));
                true
            }
            Err(_) => false,
        }
    }

    fn sync_view(&mut self, id: &InstanceId) {
        if let Some(view) = self.rebuild_view(id) {
            self.views.insert(id.clone(), view.clone());
            self.emit(ServiceEvent::InstanceChanged { view });
        }
    }
}

/// In-memory implementation of [`DblService`].
#[derive(Clone)]
pub struct MockService {
    inner: Arc<Mutex<Inner>>,
}

impl MockService {
    /// Creates a mock with demo timing.
    pub fn new() -> Self {
        Self::with_config(MockConfig::default())
    }

    /// Creates a mock with zero-delay deterministic timing.
    pub fn deterministic() -> Self {
        Self::with_config(MockConfig::deterministic())
    }

    /// Creates a mock with the given configuration.
    pub fn with_config(config: MockConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::new(config))),
        }
    }

    /// Advances the mock clock by `by` (tests).
    pub fn advance_clock(&self, by: Duration) {
        lock(&self.inner).det.advance(by);
    }

    /// Returns the mock wall-clock time (tests).
    pub fn now(&self) -> SystemTime {
        lock(&self.inner).det.now()
    }

    /// Generates a deterministic id with the given prefix (tests).
    pub fn next_deterministic_id(&self, prefix: &str) -> String {
        lock(&self.inner).det.gen_id(prefix)
    }

    /// Sets catalog freshness and emits [`ServiceEvent::CatalogUpdated`].
    pub fn set_catalog_status(&self, status: CatalogStatus) {
        let mut g = lock(&self.inner);
        g.catalog_status = status;
        g.emit(ServiceEvent::CatalogUpdated { status });
    }

    /// Queues a scripted failure consumed by the next matching operation.
    pub fn push_failure(&self, failure: ScriptedFailure) {
        lock(&self.inner).failures.push_back(failure);
    }

    /// Seeds an instance in `status` for demos/tests and emits a change event.
    pub fn seed_instance(&self, spec: InstanceSpec, status: InstanceStatus) -> InstanceId {
        let mut g = lock(&self.inner);
        // Derive the id from the seedable RNG so the `BTreeMap` order, and the
        // render tests that depend on it, are reproducible run to run.
        let id = InstanceId::parse(g.det.gen_uuid()).unwrap_or_else(|_| InstanceId::new());
        let created = g.det.now();
        let instance = Instance::new(&spec, id.clone(), status, created);
        let mut view = InstanceView::from_instance(&instance);
        if spec.auth() == AuthMode::Password {
            let backend = CredentialBackend::Keychain;
            g.credentials.insert(id.clone(), backend);
            view = view.with_credential_backend(backend);
            let pwd = SecretString::new(format!("pwd-{:08x}", g.det.next_u64() as u32));
            g.passwords.insert(id.clone(), pwd);
        }
        g.logs.insert(
            id.clone(),
            VecDeque::from([format!(
                "[info] {} {} siap",
                instance.engine().display_name(),
                instance.name()
            )]),
        );
        g.views.insert(id.clone(), view.clone());
        g.instances.insert(id.clone(), instance);
        g.emit(ServiceEvent::InstanceChanged { view });
        id
    }

    fn spawn<F>(&self, op_id: &OperationId, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(fut);
            return;
        }
        let name = format!("mock-{}", op_id);
        let _ = std::thread::Builder::new().name(name).spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build();
            if let Ok(rt) = rt {
                rt.block_on(fut);
            }
        });
    }

    async fn step(delay: Duration) {
        if delay.is_zero() {
            tokio::task::yield_now().await;
        } else {
            tokio::time::sleep(delay).await;
        }
    }

    fn cancelled(inner: &Arc<Mutex<Inner>>, op: &OperationId) -> bool {
        lock(inner)
            .operations
            .get(op)
            .is_some_and(|record| record.cancelled)
    }
}

impl Default for MockService {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn not_found(kind: &'static str, name: impl ToString) -> DblError {
    DblError::NotFound {
        kind,
        name: name.to_string(),
    }
}

fn invalid_transition(from: InstanceStatus, to: &'static str) -> DblError {
    DblError::InvalidTransition {
        from: from.as_str(),
        to,
    }
}

#[cfg(test)]
mod tests;
