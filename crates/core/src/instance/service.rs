//! `LocalService`: the production `DblService` implementation.
//!
//! The trait is synchronous and must return quickly, so every call either does
//! a short piece of work on the caller's thread (validation, one compare-and-set
//! status change) or hands the long part to a dedicated tokio runtime that lives
//! on its own OS thread. Nothing here runs on, or blocks, the GUI executor.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use tokio::runtime::{Builder, Handle};
use tokio::sync::{broadcast, oneshot};

use crate::DblError;
use crate::catalog::{CatalogFetcher, CatalogState, HttpCatalogFetcher, RefreshOutcome};
use crate::consts;
use crate::download::UrlPolicy;
use crate::engine::{EngineAdapter, adapter_for};
use crate::error::Result;
use crate::model::{CredentialRef, EngineKey, InstanceId, InstanceSpec, Port};
use crate::paths::DblPaths;
use crate::platform::{Arch, Os};
use crate::process::identity::ProcessIdentity;
use crate::process::probe::{ProcessProbe, SysinfoProbe};
use crate::process::terminate::TerminateOutcome;
use crate::secret::SecretString;
use crate::secrets::{BackendKind, SecretStore};
use crate::service::FailureInfo;
use crate::service::{DblService, OperationId, ServiceEvent};
use crate::store::Store;
use std::collections::HashMap;

use super::helpers::buffered_events;
use super::log_watch::LogWatchers;
use super::registry::Ops;

/// One downloadable runtime the service may install.
///
/// Production derives these from the catalog; tests replace catalog selection
/// with a loopback server so the whole pipeline runs offline with a tiny real
/// archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeSource {
    /// Artifact id, used for the `.part` name and diagnostics.
    pub id: String,
    /// Download URL.
    pub url: String,
    /// Expected lowercase hex SHA-256.
    pub sha256: String,
    /// Expected archive size in bytes.
    pub size_bytes: u64,
    /// Archive format.
    pub format: crate::model::ArchiveFormat,
    /// Directory stripped during extraction.
    pub strip_prefix: Option<String>,
    /// Engine the archive unpacks.
    pub engine: EngineKey,
    /// Version tag.
    pub version: String,
    /// Binaries that must exist after extraction; empty means the engine's
    /// canonical set.
    pub expected_binaries: Vec<String>,
}

/// Object-safe view of the catalog transport.
///
/// The real [`CatalogFetcher`] returns `impl Future`, which cannot be stored in
/// a trait object; this wrapper boxes the future so the service can hold one
/// fetcher behind an `Arc`.
pub trait Fetcher: Send + Sync {
    /// Fetches `url`, honouring `etag` as `If-None-Match`.
    fn fetch_boxed(
        &self,
        url: String,
        etag: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<RefreshOutcome>> + Send + 'static>>;
}

impl<T> Fetcher for T
where
    T: CatalogFetcher + Clone + Send + Sync + 'static,
{
    fn fetch_boxed(
        &self,
        url: String,
        etag: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<RefreshOutcome>> + Send + 'static>> {
        // The trait object must be `'static`, so the fetcher is cloned into the
        // future. Every implementation here is cheap to clone (an HTTP client or
        // a shared counter).
        let client = self.clone();
        Box::pin(async move { client.fetch(&url, etag.as_deref()).await })
    }
}

/// Stops a process after validating its identity.
pub trait Terminator: Send + Sync {
    /// Stops `identity`, escalating after `grace`.
    fn terminate(&self, identity: &ProcessIdentity, grace: Duration) -> Result<TerminateOutcome>;
}

/// Production terminator: the process layer's validate-before-signal path.
#[derive(Debug, Default)]
pub struct ProcessTerminator;

impl Terminator for ProcessTerminator {
    fn terminate(&self, identity: &ProcessIdentity, grace: Duration) -> Result<TerminateOutcome> {
        crate::process::terminate(identity, grace)
    }
}

/// Factory that returns the adapter for one engine.
pub type AdapterFactory = Arc<dyn Fn(EngineKey) -> Box<dyn EngineAdapter> + Send + Sync>;

/// Detached spawn: command plus log path to a recorded identity.
pub type SpawnFn =
    Arc<dyn Fn(std::process::Command, &std::path::Path) -> Result<ProcessIdentity> + Send + Sync>;

/// Port availability probe.
pub type PortFreeFn = Arc<dyn Fn(u16) -> bool + Send + Sync>;

/// Credential storage operations the service needs.
pub trait SecretAccess: Send + Sync {
    /// Stores a secret and returns the backend that holds it.
    fn set(&self, reference: &CredentialRef, secret: &SecretString) -> Result<BackendKind>;
    /// Reads the secret from the recorded backend.
    fn get(&self, reference: &CredentialRef) -> Result<SecretString>;
    /// Removes the secret from both backends.
    fn delete(&self, reference: &CredentialRef) -> Result<()>;
}

impl SecretAccess for SecretStore {
    fn set(&self, reference: &CredentialRef, secret: &SecretString) -> Result<BackendKind> {
        SecretStore::set(self, reference, secret)
    }
    fn get(&self, reference: &CredentialRef) -> Result<SecretString> {
        SecretStore::get(self, reference)
    }
    fn delete(&self, reference: &CredentialRef) -> Result<()> {
        SecretStore::delete(self, reference)
    }
}

/// Construction options; production uses [`ServiceOptions::default`].
#[derive(Clone, Default)]
pub struct ServiceOptions {
    /// Catalog refresh transport (defaults to HTTPS).
    pub catalog_fetcher: Option<Arc<dyn Fetcher>>,
    /// Engine adapter factory, so tests can script init/health/shutdown.
    pub adapter_factory: Option<AdapterFactory>,
    /// Process probe used by reconciliation and the supervisor.
    pub probe: Option<Arc<dyn ProcessProbe + Send + Sync>>,
    /// Process terminator.
    pub terminator: Option<Arc<dyn Terminator>>,
    /// Credential store.
    pub secrets: Option<Arc<dyn SecretAccess>>,
    /// Runtime sources that replace catalog selection.
    pub runtime_sources: Vec<RuntimeSource>,
    /// Port availability probe; production does a real loopback bind.
    pub port_free: Option<PortFreeFn>,
    /// Platform override for catalog filtering.
    pub os: Option<Os>,
    /// Architecture override for catalog filtering.
    pub arch: Option<Arch>,
    /// Host glibc override for artifact selection.
    pub host_glibc: Option<(u32, u32)>,
    /// Download URL policy; production is [`UrlPolicy::HttpsOnly`].
    pub download_policy: Option<UrlPolicy>,
    /// Pre-seeded catalog state.
    pub catalog: Option<CatalogState>,
    /// Detached spawn, so tests can avoid real processes.
    pub spawn: Option<SpawnFn>,
    /// Override for the readiness timeout (tests need it short).
    pub start_timeout: Option<Duration>,
    /// Override for the supervisor poll interval (tests need it short).
    pub status_poll: Option<Duration>,
}

impl std::fmt::Debug for ServiceOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceOptions")
            .field("runtime_sources", &self.runtime_sources.len())
            .field("os", &self.os)
            .field("arch", &self.arch)
            .finish_non_exhaustive()
    }
}

/// The dedicated runtime, living on its own OS thread.
pub(crate) struct ServiceRuntime {
    handle: Handle,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ServiceRuntime {
    /// Starts the runtime thread and runs `init` **on it** before returning.
    ///
    /// Everything that touches disk — opening SQLite, reading the catalog
    /// cache, probing the host glibc with `ldd` — happens inside `init`, so the
    /// caller's thread (the GUI executor) never does I/O while constructing the
    /// service. If `init` fails, the runtime is shut down before the error is
    /// returned, so a failed construction leaks no thread.
    fn start<T: Send + 'static>(
        init: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<(Self, T)> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("dblocal-service")
            .build()
            .map_err(|err| DblError::Engine {
                reason: format!("failed to start the service runtime: {err}"),
            })?;
        let handle = runtime.handle().clone();
        let (shutdown, wait) = oneshot::channel::<()>();
        let (init_tx, init_rx) = std::sync::mpsc::channel();
        let thread = thread::Builder::new()
            .name("dblocal-service-rt".to_owned())
            .spawn(move || {
                let _ = init_tx.send(init());
                runtime.block_on(async move {
                    let _ = wait.await;
                });
            })
            .map_err(|err| DblError::Engine {
                reason: format!("failed to spawn the service runtime thread: {err}"),
            })?;
        let this = Self {
            handle,
            shutdown: Some(shutdown),
            thread: Some(thread),
        };
        // Dropping `this` on the error paths below sends the shutdown signal and
        // joins the thread.
        let value = init_rx.recv().map_err(|_| DblError::Engine {
            reason: "the service runtime thread died during startup".to_owned(),
        })??;
        Ok((this, value))
    }
}

impl Drop for ServiceRuntime {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl std::fmt::Debug for ServiceRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceRuntime").finish_non_exhaustive()
    }
}

/// Everything an operation needs, shared behind one `Arc`.
pub(crate) struct Inner {
    pub paths: DblPaths,
    pub store: Mutex<Store>,
    pub events: broadcast::Sender<ServiceEvent>,
    pub ops: Ops,
    pub catalog: Mutex<CatalogState>,
    pub fetcher: Arc<dyn Fetcher>,
    pub adapters: AdapterFactory,
    pub probe: Arc<dyn ProcessProbe + Send + Sync>,
    pub terminator: Arc<dyn Terminator>,
    pub secrets: Arc<dyn SecretAccess>,
    pub log_watchers: LogWatchers,
    pub runtime_sources: Vec<RuntimeSource>,
    pub port_free: PortFreeFn,
    pub os: Os,
    pub arch: Arch,
    pub host_glibc: Option<(u32, u32)>,
    pub runtime_handle: Handle,
    pub download_policy: UrlPolicy,
    /// Detached spawn: production uses `process::spawn_detached`.
    pub spawn: SpawnFn,
    /// In-memory failure details for the current process lifetime.
    pub failures: Mutex<HashMap<InstanceId, FailureInfo>>,
    /// Readiness timeout applied to a start.
    pub start_timeout: Option<Duration>,
    /// Supervisor poll interval.
    pub status_poll: Duration,
    /// Set when the last `LocalService` handle is dropped, so the supervisor
    /// stops instead of holding the state alive forever.
    pub stopping: std::sync::atomic::AtomicBool,
    /// Number of live `LocalService` handles. The supervisor and background
    /// tasks hold `Arc<Inner>` but not a handle, so this — not `strong_count` —
    /// is what decides when the service is really gone.
    pub handles: std::sync::atomic::AtomicUsize,
    /// True while the supervisor task is looping. Cleared when it sees
    /// `stopping`, so a caller can wait for it to actually exit before taking
    /// over the home (a test restart, or a clean shutdown).
    pub supervisor_alive: std::sync::atomic::AtomicBool,
}

impl Inner {
    /// Runs `future` to completion on the service runtime from a caller thread.
    ///
    /// Only legal off the runtime's own worker threads; the trait methods are
    /// called from the GUI or CLI thread, which is exactly that case.
    pub(crate) fn block_on<T>(&self, future: impl Future<Output = T>) -> T {
        self.runtime_handle.block_on(future)
    }

    /// One short store access. The mutex is never held across an `.await`.
    pub(crate) fn db<T>(&self, f: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        let guard = self.store.lock().map_err(|_| DblError::Storage {
            reason: "the state store lock was poisoned".to_owned(),
        })?;
        f(&guard)
    }

    /// A broadcast send only fails when nobody is subscribed, which is fine.
    pub(crate) fn emit(&self, event: ServiceEvent) {
        let _ = self.events.send(event);
    }

    pub(crate) fn adapter(&self, engine: EngineKey) -> Box<dyn EngineAdapter> {
        (self.adapters)(engine)
    }

    pub(crate) fn handle(&self) -> Handle {
        self.runtime_handle.clone()
    }

    /// Whether the service is shutting down.
    pub(crate) fn is_stopping(&self) -> bool {
        self.stopping.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Production implementation of [`DblService`].
///
/// Holds the runtime handle separately from the shared state: the runtime must
/// outlive every background task, but those tasks hold `Arc<Inner>`. Putting it
/// here (declared after `inner`) means dropping the last `LocalService` shuts
/// the runtime down, which aborts the tasks and lets `Inner` drop.
pub struct LocalService {
    inner: Arc<Inner>,
    /// Kept only to own the runtime so it drops after `inner`; never read.
    #[allow(dead_code)]
    runtime: Arc<ServiceRuntime>,
}

impl Clone for LocalService {
    fn clone(&self) -> Self {
        self.inner
            .handles
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self {
            inner: Arc::clone(&self.inner),
            runtime: Arc::clone(&self.runtime),
        }
    }
}

impl std::fmt::Debug for LocalService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalService")
            .field("root", &self.inner.paths.root())
            .finish_non_exhaustive()
    }
}

impl LocalService {
    /// Builds the service over `paths`.
    ///
    /// Opens the store synchronously (local disk only), starts the runtime and
    /// schedules reconciliation. It never waits on the network.
    pub fn new(paths: DblPaths, options: ServiceOptions) -> Result<Self> {
        // Open the store, load the catalog and probe the host glibc on the
        // service thread, never on the caller's. See `ServiceRuntime::start`.
        let init_paths = paths.clone();
        let preset_catalog = options.catalog.clone();
        let preset_glibc = options.host_glibc;
        let (runtime, initialized) = ServiceRuntime::start(move || {
            let store = Store::open(&init_paths)?;
            let catalog = match preset_catalog {
                Some(state) => state,
                None => crate::catalog::load_or_bundled(
                    &init_paths,
                    SystemTime::now(),
                    crate::catalog::catalog_ttl(),
                )
                .or_else(|_| crate::catalog::load_bundled_snapshot())?,
            };
            let host_glibc = preset_glibc.or_else(|| {
                crate::engine::glibc::host_version().map(|version| (version.major, version.minor))
            });
            Ok((store, catalog, host_glibc))
        })?;
        let (store, catalog, host_glibc) = initialized;
        super::helpers::ensure_private_layout(&paths)?;

        let (events, _) = broadcast::channel(consts::SERVICE_EVENT_CHANNEL_CAPACITY);

        let fetcher: Arc<dyn Fetcher> = match options.catalog_fetcher {
            Some(fetcher) => fetcher,
            None => Arc::new(
                HttpCatalogFetcher::with_timeout(Duration::from_secs(
                    crate::catalog::CATALOG_REFRESH_TIMEOUT_SECS,
                ))
                .map_err(|err| DblError::Network {
                    reason: format!("could not build the catalog client: {err}"),
                })?,
            ),
        };

        let adapters = options
            .adapter_factory
            .clone()
            .unwrap_or_else(|| Arc::new(|engine| adapter_for(engine)));
        let probe = options
            .probe
            .clone()
            .unwrap_or_else(|| Arc::new(SysinfoProbe::new()));
        let terminator = options
            .terminator
            .clone()
            .unwrap_or_else(|| Arc::new(ProcessTerminator));
        let secrets = match options.secrets.clone() {
            Some(secrets) => secrets,
            None => Arc::new(SecretStore::for_paths(&paths)),
        };
        let spawn = options.spawn.clone().unwrap_or_else(|| {
            Arc::new(|command: std::process::Command, log: &std::path::Path| {
                crate::process::spawn_detached(command, log)
            })
        });
        let port_free = options.port_free.clone().unwrap_or_else(|| {
            Arc::new(|port: u16| match Port::new(port) {
                Ok(port) => crate::ports::probe_or_in_use(port).is_free(),
                Err(_) => false,
            })
        });
        let os = options.os.unwrap_or_else(Os::current);
        let arch = options.arch.unwrap_or_else(Arch::current);

        let runtime = Arc::new(runtime);
        let inner = Arc::new(Inner {
            paths,
            store: Mutex::new(store),
            events,
            ops: Ops::new(),
            catalog: Mutex::new(catalog),
            fetcher,
            adapters,
            probe,
            terminator,
            secrets,
            log_watchers: LogWatchers::new(),
            runtime_sources: options.runtime_sources,
            port_free,
            os,
            arch,
            host_glibc,
            download_policy: options
                .download_policy
                .unwrap_or_else(UrlPolicy::https_only),
            spawn,
            failures: Mutex::new(HashMap::new()),
            start_timeout: options.start_timeout,
            status_poll: options
                .status_poll
                .unwrap_or_else(|| Duration::from_secs(consts::STATUS_POLL_SECS)),
            stopping: std::sync::atomic::AtomicBool::new(false),
            handles: std::sync::atomic::AtomicUsize::new(1),
            supervisor_alive: std::sync::atomic::AtomicBool::new(true),
            runtime_handle: runtime.handle.clone(),
        });

        let service = Self { inner, runtime };
        service.schedule_startup();
        Ok(service)
    }

    fn schedule_startup(&self) {
        let inner = Arc::clone(&self.inner);
        self.inner.handle().spawn(async move {
            super::reconcile::reconcile_on_startup(Arc::clone(&inner)).await;
            super::supervisor::run(inner).await;
        });
    }

    /// Opens the state store already held by the service (tests and diagnostics).
    pub fn state_root(&self) -> &std::path::Path {
        self.inner.paths.root()
    }

    /// The shared state behind the service.
    ///
    /// Exposed only to this module's tests so they can call a pure helper
    /// (`catalog_ops::source_for`) directly instead of driving it through a
    /// download.
    #[cfg(test)]
    pub(crate) fn inner(&self) -> &Arc<Inner> {
        &self.inner
    }

    /// Claims a transition from an explicit expected status.
    ///
    /// Exposed to this module's tests so a stale expectation can be exercised
    /// directly: a real competing writer would otherwise be needed to reach the
    /// same branch, and a timing-based test cannot prove the CAS is consulted.
    #[cfg(test)]
    pub(crate) fn claim_for_test(
        &self,
        id: &InstanceId,
        expected: crate::model::InstanceStatus,
        event: crate::model::InstanceEvent,
    ) -> Result<crate::model::Instance> {
        super::helpers::claim_from(&self.inner, id, expected, event)
    }

    /// Claims a raw `expected -> next` status change, like delete does.
    #[cfg(test)]
    pub(crate) fn claim_status_for_test(
        &self,
        id: &InstanceId,
        expected: crate::model::InstanceStatus,
        next: crate::model::InstanceStatus,
    ) -> Result<bool> {
        super::helpers::claim_status(&self.inner, id, expected, next)
    }

    /// Whether the in-memory failure map has an entry, for a test that asserts
    /// the supervisor did *not* record a failure.
    #[cfg(test)]
    pub(crate) fn has_failure_record(&self, id: &InstanceId) -> bool {
        self.inner
            .failures
            .lock()
            .map(|failures| failures.contains_key(id))
            .unwrap_or(false)
    }

    /// Whether an operation has already reported `OperationFinished`.
    ///
    /// A consumer that subscribed after completion (a late UI, or a test harness
    /// that could not subscribe before the call) uses this to notice the end
    /// instead of waiting on an event that will never be re-delivered. Exposed to
    /// this module's tests for exactly that reason.
    #[cfg(test)]
    pub(crate) fn is_operation_finished(&self, op: &OperationId) -> bool {
        self.inner.ops.is_finished(op)
    }

    /// Whether this service has been told to stop (its last handle was dropped).
    #[cfg(test)]
    pub(crate) fn stopping_for_test(&self) -> bool {
        self.inner.is_stopping()
    }

    /// Whether the supervisor task is still looping.
    #[cfg(test)]
    pub(crate) fn supervisor_alive_for_test(&self) -> bool {
        self.inner
            .supervisor_alive
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Acquires the per-instance operation lock, for a test that must prove the
    /// supervisor skips a `Running` row while an operation holds it.
    #[cfg(test)]
    pub(crate) fn lock_instance_for_test(
        &self,
        id: &InstanceId,
    ) -> Result<super::registry::InstanceLock> {
        self.inner.ops.lock_instance(id)
    }

    /// Runs the real create-rollback, for a test that must reach the path guard
    /// rather than assert on it indirectly.
    #[cfg(test)]
    pub(crate) fn rollback_for_test(
        &self,
        id: &InstanceId,
        name: &crate::model::InstanceName,
    ) -> bool {
        self.inner
            .block_on(super::helpers::rollback_create(&self.inner, id, name))
    }

    /// How many subscribers are listening, for diagnostics.
    pub fn subscriber_count(&self) -> usize {
        self.inner.events.receiver_count()
    }

    /// How many instances are being followed for live logs.
    pub fn watched_logs(&self) -> usize {
        self.inner.log_watchers.watched()
    }
}

impl Drop for LocalService {
    fn drop(&mut self) {
        // The supervisor task and the background worker hold their own
        // `Arc<Inner>`, so `Arc::strong_count` never reaches one while they run
        // and could not be used to detect "last handle". A handle counter does:
        // every `clone()` increments it and each `drop()` decrements it, and only
        // the decrement that reaches zero (the `== 1` returned before subtracting)
        // stops the supervisor and the watchers. A clone that is still alive
        // therefore keeps the service fully working.
        if self
            .inner
            .handles
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst)
            == 1
        {
            self.inner
                .stopping
                .store(true, std::sync::atomic::Ordering::SeqCst);
            self.inner.log_watchers.stop_all();
        }
    }
}

impl DblService for LocalService {
    fn list_instances(&self) -> Result<Vec<crate::service::InstanceView>> {
        super::read_ops::list_instances(&self.inner)
    }

    fn catalog(&self) -> Result<crate::service::CatalogView> {
        super::read_ops::catalog(&self.inner)
    }

    fn refresh_catalog(&self, force: bool) -> Result<OperationId> {
        super::catalog_ops::refresh(&self.inner, force)
    }

    fn validate_spec(&self, spec: &InstanceSpec) -> Result<()> {
        super::read_ops::validate_spec(&self.inner, spec)
    }

    fn suggest_port(&self, engine: EngineKey) -> Result<Port> {
        super::read_ops::suggest_port(&self.inner, engine)
    }

    fn create_instance(&self, spec: InstanceSpec) -> Result<OperationId> {
        super::create::create_instance(&self.inner, spec)
    }

    fn start_instance(&self, id: &InstanceId) -> Result<OperationId> {
        super::start::start_instance(&self.inner, id)
    }

    fn stop_instance(&self, id: &InstanceId) -> Result<OperationId> {
        super::stop::stop_instance(&self.inner, id)
    }

    fn delete_instance(
        &self,
        id: &InstanceId,
        mode: crate::model::DeleteMode,
    ) -> Result<OperationId> {
        super::delete::delete_instance(&self.inner, id, mode)
    }

    fn change_port(&self, id: &InstanceId, port: Port) -> Result<()> {
        super::read_ops::change_port(&self.inner, id, port)
    }

    fn data_size(&self, id: &InstanceId) -> Result<u64> {
        super::read_ops::data_size(&self.inner, id)
    }

    fn set_autostart(&self, id: &InstanceId, enabled: bool) -> Result<()> {
        super::read_ops::set_autostart(&self.inner, id, enabled)
    }

    fn cancel_operation(&self, id: &OperationId) -> Result<()> {
        super::read_ops::cancel_operation(&self.inner, id)
    }

    fn connection_uri(
        &self,
        id: &InstanceId,
        format: crate::service::UriFormat,
    ) -> Result<SecretString> {
        super::read_ops::connection_uri(&self.inner, id, format)
    }

    fn tail_logs(&self, id: &InstanceId, lines: usize) -> Result<Vec<String>> {
        super::read_ops::tail_logs(&self.inner, id, lines)
    }

    fn watch_logs(&self, id: &InstanceId, enabled: bool) -> Result<()> {
        super::read_ops::watch_logs(&self.inner, id, enabled)
    }

    fn list_runtimes(&self) -> Result<Vec<crate::model::Runtime>> {
        super::runtime_mgmt::list_runtimes(&self.inner)
    }

    fn remove_runtime(&self, runtime: &str) -> Result<()> {
        self.inner
            .block_on(super::runtime_mgmt::remove_runtime(&self.inner, runtime))
    }

    fn list_archives(&self) -> Result<Vec<crate::model::ArchivedData>> {
        super::runtime_mgmt::list_archives(&self.inner)
    }

    fn purge_archive(&self, archive: &str) -> Result<()> {
        self.inner
            .block_on(super::runtime_mgmt::purge_archive(&self.inner, archive))
    }

    fn subscribe(&self) -> broadcast::Receiver<ServiceEvent> {
        buffered_events(&self.inner.events)
    }
}
