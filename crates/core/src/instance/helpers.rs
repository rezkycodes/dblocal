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
use crate::process::identity::{ProcessIdentity, ProcessState};
use crate::process::terminate::TerminateOutcome;
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
        && let Some(failure) = failure_for(inner, instance)
    {
        view = view.with_failure(failure);
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

/// Announces that a row is gone.
///
/// `InstanceChanged` cannot express this (there is no view left), so a delete, a
/// rolled-back create and a cleaned-up interrupted create all end here.
pub(crate) fn emit_removed(inner: &Inner, id: &crate::model::InstanceId) {
    inner.emit(ServiceEvent::InstanceRemoved { id: id.clone() });
}

/// Announces that a stop had to escalate to a forced kill (`F-INS-03` AC3).
pub(crate) fn emit_stop_forced(inner: &Inner, id: &crate::model::InstanceId) {
    inner.emit(ServiceEvent::StopForced { id: id.clone() });
}

/// Builds failure details for a `Failed` row, from the in-memory record or,
/// when that is missing (a new process after a restart), from the log tail.
fn failure_for(inner: &Inner, instance: &Instance) -> Option<FailureInfo> {
    let stored = inner
        .failures
        .lock()
        .ok()
        .and_then(|failures| failures.get(instance.id()).cloned());
    if stored.is_some() {
        return stored;
    }
    // `F-LOG-02` requires an explanation, not just the word "failed", so a row
    // restored from disk is diagnosed lazily from its log instead of showing an
    // empty panel. No schema change is needed: the tail is re-derived.
    let tail = failure_tail(&log_file_for(&inner.paths, instance));
    let diagnosis = diagnose(inner, instance.engine(), &tail.join("\n"));
    Some(failure_info(
        crate::error::CODE_ENGINE,
        crate::strings::ERR_ENGINE.to_owned(),
        diagnosis,
        tail,
    ))
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

/// Claims a transition that must start from `expected`, regardless of what the
/// row currently holds.
///
/// This is the primitive that makes a stale observation safe: the caller passes
/// the status it *believes* the row is in, and the compare-and-set refuses when
/// another writer already moved it. Every operation that reads then claims goes
/// through this; re-reading the row inside the claim would let a competing
/// writer's status be adopted silently and destroy the lock.
///
/// `event` is only applied to `expected` first, so an event that is illegal from
/// `expected` is rejected before any write, with the same error a stale row
/// would produce.
pub(crate) fn claim_from(
    inner: &Inner,
    id: &crate::model::InstanceId,
    expected: InstanceStatus,
    event: InstanceEvent,
) -> Result<Instance> {
    let next = expected.apply(event)?;
    if !inner.db(|store| store.transition_status(id, expected, next))? {
        return Err(DblError::InvalidTransition {
            from: "busy",
            to: next.as_str(),
        });
    }
    let instance = load(inner, id)?;
    if let Ok(view) = view_for(inner, &instance) {
        inner.emit(ServiceEvent::InstanceChanged { view });
    }
    Ok(instance)
}

/// Claims a specific `expected -> next` status change with the compare-and-set,
/// without consulting the state machine.
///
/// Delete needs this: it claims `Deleting` from any non-terminal status because
/// it owns stopping the engine as part of the delete (D-18), which the machine
/// models only for an interactive stop (`StopRequested -> Stopping`). Every other
/// transition goes through [`claim_from`] and therefore the machine.
///
/// The CAS is still the lock: a competing writer that moved the row between the
/// caller's read and this write makes it lose.
pub(crate) fn claim_status(
    inner: &Inner,
    id: &crate::model::InstanceId,
    expected: InstanceStatus,
    next: InstanceStatus,
) -> Result<bool> {
    inner.db(|store| store.transition_status(id, expected, next))
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

/// Rewrites the engine config so its port matches the store.
///
/// Idempotent: safe to call before every start and from `change_port`.
/// Only engines with on-disk port config are touched; others are no-ops.
/// If the engine config file does not exist yet, this is a no-op: the port
/// will be written when the engine is initialised or when the config appears.
pub(crate) fn sync_instance_port(paths: &DblPaths, instance: &Instance) -> Result<()> {
    let name = instance.name();
    let port = instance.port().get();
    match instance.engine() {
        EngineKey::Postgresql => {
            let conf = paths.data_dir(name).join("postgresql.conf");
            if conf.exists() {
                crate::engine::sync_postgres_port(
                    &paths.data_dir(name),
                    &paths.run_dir(name),
                    port,
                )?;
            }
        }
        EngineKey::Valkey => {
            let conf = paths.run_dir(name).join("valkey.conf");
            if conf.exists() {
                crate::engine::sync_valkey_port(&paths.run_dir(name), port)?;
            }
        }
        EngineKey::Mysql | EngineKey::Mariadb => {}
    }
    Ok(())
}

/// Best-effort private directory creation (0700 on Unix). Used for instance
/// layout directories that hold data, sockets, logs or secrets.
pub(crate) fn ensure_private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|source| DblError::Io { source })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|source| DblError::Io { source })?;
    }
    Ok(())
}

/// Tightens the instance-layout root directories to 0700 on Unix.
///
/// `instances/` and `archive/` only contain DBLocal-owned subdirectories, but
/// they are easy to create with a permissive umask in older builds; we tighten
/// them explicitly so instance names are not enumerable by other local users.
pub(crate) fn ensure_private_layout(paths: &DblPaths) -> Result<()> {
    for dir in [
        paths.instances_dir(),
        paths.archive_dir(),
        paths.runtimes_dir(),
        paths.downloads_dir(),
    ] {
        ensure_private_dir(&dir)?;
    }
    Ok(())
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

/// Re-validates a path read from the store (or derived from an instance name)
/// before deleting or moving it.
///
/// Two checks, both required:
///
/// 1. **Lexical containment** — after normalisation the path must be a strict
///    descendant of the home. This rejects `..` and an absolute path.
/// 2. **No symlink on the way** — every existing component below the home must be
///    a real entry, not a symlink. Lexical containment alone is not enough: a
///    path like `<home>/instances/<name>` is inside the home as a *string* while
///    `<name>` may itself be a symlink to `/etc`, and deleting through it would
///    reach outside. The check is on the components, not on the resolved target,
///    so a symlink is refused outright rather than silently followed.
pub(crate) fn safe_within(paths: &DblPaths, candidate: &Path) -> Result<()> {
    let base = normalize(paths.root());
    let target = normalize(candidate);
    if target == base || !target.starts_with(&base) {
        return Err(DblError::UnsafeArchive {
            entry: candidate.display().to_string(),
        });
    }
    if has_symlink_below(&base, &target) {
        return Err(DblError::UnsafeArchive {
            entry: candidate.display().to_string(),
        });
    }
    Ok(())
}

/// Whether any existing component of `target` strictly below `base` is a
/// symlink.
///
/// Only the components that exist are inspected, so a path that has not been
/// created yet (an archive target before the move) is accepted; the moment a
/// component exists as a symlink, it is rejected. Errors reading metadata are
/// treated as "not a symlink" here because the lexical check already confined the
/// path and the caller's own I/O reports a real failure.
fn has_symlink_below(base: &Path, target: &Path) -> bool {
    let Ok(relative) = target.strip_prefix(base) else {
        return true;
    };
    let mut current = base.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => return true,
            Ok(_) => {}
            // Does not exist (or is unreadable): nothing to follow yet.
            Err(_) => return false,
        }
    }
    false
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

/// The credential for a stop, which must never block one.
///
/// A stop exists to make an engine go away; an unreadable secret only removes
/// the graceful path (the adapter cannot authenticate), so it is downgraded to
/// "no credential" with a log line and escalation through the process layer
/// still runs. Raising here would leave the engine running while the row said
/// otherwise.
pub(crate) fn credential_for_stop(inner: &Inner, instance: &Instance) -> Option<Credential> {
    match read_credential(inner, instance) {
        Ok(credential) => credential,
        Err(err) => {
            tracing::warn!(
                instance = %instance.id(),
                error = %err,
                "credential unavailable; stopping through the process layer only"
            );
            None
        }
    }
}

/// Removes everything a failed or interrupted create could have produced.
///
/// The verified runtime is deliberately kept: it is reusable, and removing it
/// would make a retry download it again. Paths are re-validated first and a
/// rejected path aborts the removal rather than being ignored, so a corrupt row
/// cannot turn cleanup into a delete outside the home.
///
/// Returns whether a row was actually removed, so the caller can announce it.
pub(crate) async fn rollback_create(
    inner: &Arc<Inner>,
    id: &crate::model::InstanceId,
    name: &crate::model::InstanceName,
) -> bool {
    let instance_dir = inner.paths.instance_dir(name);
    let data_dir = inner.paths.data_dir(name);
    for path in [&instance_dir, &data_dir] {
        if let Err(err) = safe_within(&inner.paths, path) {
            tracing::error!(path = %path.display(), error = %err, "rollback refused an unsafe path");
            return false;
        }
    }
    let _ = blocking(move || {
        remove_tree(&data_dir)?;
        remove_tree(&instance_dir)
    })
    .await;

    for backend in [
        crate::model::CredentialBackend::File,
        crate::model::CredentialBackend::Keychain,
    ] {
        let reference = crate::secrets::credential_ref_for(id, backend, "");
        let _ = inner.secrets.delete(&reference);
    }

    let id2 = id.clone();
    let removed = db_async(inner, move |store| match store.delete_instance(&id2) {
        Ok(()) => Ok(true),
        Err(DblError::NotFound { .. }) => Ok(false),
        Err(err) => Err(err),
    })
    .await
    .unwrap_or(false);
    inner.log_watchers.disable(id);
    removed
}

/// What a verified termination produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Terminated {
    /// Whether the process is gone, decided by a probe *after* the attempt.
    pub state: Stopped,
    /// Whether the terminator had to escalate to a forced kill.
    ///
    /// Only ever `true` when `TerminateOutcome::Forced` was returned, so the UI
    /// is never told "kill paksa" for a signal that never landed.
    pub forced: bool,
}

/// What a verified termination produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stopped {
    /// The process is gone (or never was ours): the identity may be cleared.
    Gone,
    /// The process is still alive under our identity: the identity must be kept.
    StillAlive,
}

/// Terminates `identity` and then **re-probes** it, so the caller never clears a
/// PID that still names a live engine.
///
/// The process layer already refuses a mismatch; this decides the row's fate
/// from an observation made after the attempt, which is the only trustworthy
/// answer to "is it gone?".
pub(crate) async fn terminate_verified(
    inner: &Arc<Inner>,
    identity: Option<ProcessIdentity>,
    grace: Duration,
) -> Terminated {
    let Some(identity) = identity else {
        // No recorded process: there is nothing we may signal, and nothing to
        // keep an identity for.
        return Terminated {
            state: Stopped::Gone,
            forced: false,
        };
    };

    let terminator = Arc::clone(&inner.terminator);
    let probe = Arc::clone(&inner.probe);
    let outcome = blocking(move || {
        let result = terminator.terminate(&identity, grace);
        // Probe regardless of the signal result: a refused signal on a live
        // process must still report "alive", and a vanished one "gone".
        let observed = probe.validate(&identity);
        Ok((result, observed))
    })
    .await;

    let (forced, state) = match outcome {
        Ok((result, observed)) => {
            let forced = result
                .as_ref()
                .is_ok_and(|outcome| matches!(outcome, TerminateOutcome::Forced));
            if let Err(err) = &result {
                tracing::warn!(error = %err, "termination reported an error; re-probing");
            }
            let state = match observed {
                ProcessState::Match => Stopped::StillAlive,
                // Absent and Reused mean our process is gone; Unknown means we
                // cannot prove it is ours any more, which also means we must not
                // keep claiming it.
                ProcessState::Absent | ProcessState::Reused | ProcessState::Unknown => {
                    Stopped::Gone
                }
            };
            (forced, state)
        }
        Err(err) => {
            tracing::warn!(error = %err, "termination probe failed");
            (false, Stopped::StillAlive)
        }
    };
    Terminated { state, forced }
}
