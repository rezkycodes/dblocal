//! Shared plumbing: views, transitions, failure records and path guards.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;

use crate::DblError;
use crate::consts;
use crate::engine::{Credential, RunContext};
use crate::error::Result;
use crate::model::{
    AuthMode, EngineKey, Instance, InstanceEvent, InstanceStatus, Port, StartFailure,
};
use crate::paths::DblPaths;
use crate::process::identity::ProcessIdentity;
use crate::secret::SecretString;
use crate::service::{FailureInfo, InstanceView, OperationId, OperationStage, ServiceEvent};
use crate::store::Store;

use super::service::Inner;

/// A failed operation: the error plus the engine diagnosis it produced.
#[derive(Debug)]
pub(crate) struct OpFailure {
    pub error: DblError,
    pub diagnosis: Option<StartFailure>,
}

impl OpFailure {
    /// Wraps a plain error.
    pub(crate) fn new(error: DblError) -> Self {
        Self {
            error,
            diagnosis: None,
        }
    }

    /// Attaches a start diagnosis so the UI can offer a concrete fix.
    pub(crate) fn with_diagnosis(mut self, diagnosis: StartFailure) -> Self {
        self.diagnosis = Some(diagnosis);
        self
    }
}

impl From<DblError> for OpFailure {
    fn from(error: DblError) -> Self {
        Self::new(error)
    }
}

/// [`crate::error::Result`] with the operation-failure payload.
pub(crate) type OpResult<T> = std::result::Result<T, OpFailure>;

pub(crate) fn buffered_events(
    sender: &broadcast::Sender<ServiceEvent>,
) -> broadcast::Receiver<ServiceEvent> {
    sender.subscribe()
}

pub(crate) fn view_for(inner: &Inner, instance: &Instance) -> Result<InstanceView> {
    let backend = inner
        .db(|store| store.get_credential(instance.id()))?
        .map(|r| r.backend());
    let mut view = InstanceView::from_instance(instance);
    if let Some(backend) = backend {
        view = view.with_credential_backend(backend);
    }
    if instance.status() == InstanceStatus::Failed
        && let Ok(failures) = inner.failures.lock()
        && let Some(failure) = failures.get(instance.id())
    {
        view = view.with_failure(failure.clone());
    }
    Ok(view)
}

pub(crate) fn emit_changed(inner: &Inner, id: &crate::model::InstanceId) {
    if let Ok(Some(instance)) = inner.db(|store| store.get_instance(id))
        && let Ok(view) = view_for(inner, &instance)
    {
        inner.emit(ServiceEvent::InstanceChanged { view });
    }
}

pub(crate) fn emit_stage(inner: &Inner, op: &OperationId, stage: OperationStage) {
    inner.emit(ServiceEvent::OperationProgress {
        id: op.clone(),
        stage,
    });
}

/// Emits `Failed`/`Done` and then exactly one `OperationFinished`.
pub(crate) fn finish_operation(inner: &Inner, op: &OperationId, result: OpResult<()>) {
    let rendered = match &result {
        Ok(()) => {
            emit_stage(inner, op, OperationStage::Done);
            Ok(())
        }
        Err(failure) => {
            emit_stage(inner, op, OperationStage::Failed);
            let mut facing = crate::service::UserFacingError::from(&failure.error);
            if let Some(diagnosis) = failure.diagnosis {
                facing = facing.with_diagnosis(diagnosis);
            }
            Err(facing)
        }
    };
    inner.emit(ServiceEvent::OperationFinished {
        id: op.clone(),
        result: rendered,
    });
    inner.ops.finish(op);
}

/// Loads an instance or reports a typed `NotFound`.
pub(crate) fn load(inner: &Inner, id: &crate::model::InstanceId) -> Result<Instance> {
    inner
        .db(|store| store.get_instance(id))?
        .ok_or_else(|| DblError::NotFound {
            kind: "instance",
            name: id.as_str().to_owned(),
        })
}

/// Applies `event` through the state machine and persists the result with a
/// compare-and-set, which is also the cross-process lock.
///
/// The loser of the CAS gets `InvalidTransition{from:"busy"}` so a second
/// process (or a second call) never double-starts or double-deletes.
pub(crate) fn claim(
    inner: &Inner,
    id: &crate::model::InstanceId,
    event: InstanceEvent,
) -> Result<Instance> {
    let instance = load(inner, id)?;
    let next = instance.status().apply(event)?;
    let from = instance.status();
    if !inner.db(|store| store.transition_status(id, from, next))? {
        return Err(DblError::InvalidTransition {
            from: "busy",
            to: next.as_str(),
        });
    }
    let claimed = instance.with_status(next);
    if let Ok(view) = view_for(inner, &claimed) {
        inner.emit(ServiceEvent::InstanceChanged { view });
    }
    Ok(claimed)
}

/// Every port already reserved by an instance.
pub(crate) fn reserved_ports(inner: &Inner) -> Result<std::collections::HashSet<Port>> {
    Ok(inner
        .db(|store| store.list_instances())?
        .into_iter()
        .map(|instance| instance.port())
        .collect())
}

pub(crate) fn ensure_port_is_free(
    inner: &Inner,
    port: Port,
    ignore: Option<&crate::model::InstanceId>,
) -> Result<()> {
    for other in inner.db(|store| store.list_instances())? {
        if other.port() == port && ignore.is_none_or(|id| other.id() != id) {
            return Err(DblError::PortInUse { port: port.get() });
        }
    }
    if !(inner.port_free)(port.get()) {
        return Err(DblError::PortInUse { port: port.get() });
    }
    Ok(())
}

pub(crate) fn run_context(
    paths: &DblPaths,
    instance: &Instance,
    credential: Option<Credential>,
) -> RunContext {
    let name = instance.name();
    RunContext {
        instance_name: name.as_str().to_owned(),
        runtime_dir: runtime_dir_for(paths, instance.engine(), instance.version()),
        data_dir: paths.data_dir(name),
        logs_dir: paths.logs_dir(name),
        run_dir: paths.run_dir(name),
        port: instance.port(),
        superuser: instance.superuser().to_owned(),
        auth: instance.auth(),
        credential,
    }
}

pub(crate) fn runtime_dir_for(paths: &DblPaths, engine: EngineKey, version: &str) -> PathBuf {
    paths
        .runtime_dir(engine, version, crate::platform::Arch::current())
        .unwrap_or_else(|_| paths.runtimes_dir().join(engine.as_str()).join(version))
}

pub(crate) fn log_file_for(paths: &DblPaths, instance: &Instance) -> PathBuf {
    let logs = paths.logs_dir(instance.name());
    match instance.engine() {
        EngineKey::Postgresql => logs.join("postgres.log"),
        EngineKey::Valkey => logs.join("valkey.log"),
        EngineKey::Mysql => logs.join("mysql.log"),
        EngineKey::Mariadb => logs.join("mariadb.log"),
    }
}

pub(crate) fn failure_tail(path: &Path) -> Vec<String> {
    crate::process::logs::tail_lines(path, consts::FAILURE_LOG_TAIL_LINES).unwrap_or_default()
}

pub(crate) fn identity_of(instance: &Instance) -> Option<ProcessIdentity> {
    match (
        instance.pid(),
        instance.process_started_at(),
        instance.process_exe(),
    ) {
        (Some(pid), Some(started_at), Some(exe)) => {
            ProcessIdentity::new(pid, exe.to_path_buf(), started_at).ok()
        }
        _ => None,
    }
}

pub(crate) fn start_timeout(engine: EngineKey) -> Duration {
    match engine {
        EngineKey::Valkey => Duration::from_secs(consts::START_TIMEOUT_VALKEY_SECS),
        _ => Duration::from_secs(consts::START_TIMEOUT_SECS),
    }
}

/// Removes a directory tree, tolerating absence.
pub(crate) fn remove_tree(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(DblError::Io { source: err }),
    }
}

/// Re-validates a path read from the store before deleting or moving it.
pub(crate) fn safe_within(paths: &DblPaths, candidate: &Path) -> Result<()> {
    let base = normalize(paths.root());
    let target = normalize(candidate);
    if target == base || !target.starts_with(&base) {
        return Err(DblError::UnsafeArchive {
            entry: candidate.display().to_string(),
        });
    }
    Ok(())
}

pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Total size of a directory tree.
pub(crate) fn dir_size(path: &Path) -> u64 {
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

pub(crate) fn secret_for(auth: AuthMode) -> Result<Option<SecretString>> {
    match auth {
        AuthMode::Password => Ok(Some(crate::secrets::generate_password()?)),
        AuthMode::None => Ok(None),
    }
}

pub(crate) fn credential_for(
    instance: &Instance,
    password: Option<&SecretString>,
) -> Option<Credential> {
    match (instance.auth(), password) {
        (AuthMode::Password, Some(password)) => {
            Some(Credential::new(instance.superuser(), password.clone()))
        }
        _ => None,
    }
}

pub(crate) fn validate_spec_boundary(spec: &crate::model::InstanceSpec) -> Result<()> {
    if spec.version().is_empty() {
        return Err(DblError::InvalidInput {
            field: "version",
            reason: "must not be empty".to_owned(),
        });
    }
    Ok(())
}

pub(crate) fn failure_info(
    code: &str,
    message: String,
    diagnosis: StartFailure,
    log_tail: Vec<String>,
) -> FailureInfo {
    FailureInfo {
        code: code.to_owned(),
        message,
        diagnosis,
        log_tail,
    }
}

/// Engine-specific diagnosis for raw start output.
pub(crate) fn diagnose(inner: &Inner, engine: EngineKey, output: &str) -> StartFailure {
    inner.adapter(engine).diagnose_start_failure(output, None)
}

/// Runs a store closure on the blocking pool so a runtime worker never blocks.
pub(crate) async fn db_async<T, F>(inner: &Arc<Inner>, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Store) -> Result<T> + Send + 'static,
{
    let inner = Arc::clone(inner);
    tokio::task::spawn_blocking(move || inner.db(f))
        .await
        .map_err(|err| DblError::Storage {
            reason: format!("the store task failed: {err}"),
        })?
}

/// Runs CPU- or syscall-bound work off the runtime workers.
pub(crate) async fn blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|err| DblError::Engine {
            reason: format!("a blocking task failed: {err}"),
        })?
}

/// Reads the credential for an authenticated instance, if any is recorded.
pub(crate) fn read_credential(inner: &Inner, instance: &Instance) -> Result<Option<Credential>> {
    if instance.auth() != AuthMode::Password {
        return Ok(None);
    }
    let Some(reference) = inner.db(|store| store.get_credential(instance.id()))? else {
        return Ok(None);
    };
    let password = inner.secrets.get(&reference)?;
    Ok(credential_for(instance, Some(&password)))
}
