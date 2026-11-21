//! Starting an instance (`F-INS-03`, `F-PRT-02`, `F-LOG-02`).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::DblError;
use crate::engine::{EngineAdapter, Health, RunContext};
use crate::error::Result;
use crate::model::{EngineKey, Instance, InstanceEvent, InstanceId, InstanceStatus, StartFailure};
use crate::process::identity::{ProcessIdentity, ProcessState};
use crate::service::{OperationId, OperationStage, ServiceEvent};

use super::helpers::{
    OpFailure, OpResult, Stopped, blocking, db_async, emit_changed, emit_stage,
    ensure_port_is_free, failure_info, failure_tail, finish_operation, identity_of, load,
    log_file_for, read_credential, run_context, start_timeout, terminate_verified, view_for,
};
use super::service::Inner;

/// Where the real engine PID comes from once the launcher has run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdentitySource {
    /// The spawned child is the server itself (Valkey, MySQL/MariaDB stubs).
    Child,
    /// `pg_ctl` exits and the server writes `postmaster.pid`.
    PostmasterPid,
    /// The server writes `run/valkey.pid`.
    ValkeyPid,
}

fn identity_source(engine: EngineKey) -> IdentitySource {
    match engine {
        EngineKey::Postgresql => IdentitySource::PostmasterPid,
        EngineKey::Valkey => IdentitySource::ValkeyPid,
        EngineKey::Mysql | EngineKey::Mariadb => IdentitySource::Child,
    }
}

pub(crate) fn start_instance(inner: &Arc<Inner>, id: &InstanceId) -> Result<OperationId> {
    let instance = load(inner, id)?;
    if !matches!(
        instance.status(),
        InstanceStatus::Stopped | InstanceStatus::Failed
    ) {
        return Err(DblError::InvalidTransition {
            from: instance.status().as_str(),
            to: "starting",
        });
    }

    // Claim the transition synchronously: `Starting` is persisted before any
    // side effect, and the compare-and-set is the cross-process lock. The claim
    // is made *from the status just read*, not from a fresh re-read: if another
    // service moved the row between our read and the write, the CAS refuses and
    // this call fails as busy instead of double-starting.
    let lock = inner.ops.lock_instance(id)?;
    let claimed =
        super::helpers::claim_from(inner, id, instance.status(), InstanceEvent::StartRequested)?;

    let op = inner.ops.next_id();
    let op_id = op.clone();
    let inner = Arc::clone(inner);
    let id = id.clone();
    inner.handle().spawn(async move {
        let lock = lock;
        emit_stage(&inner, &op, OperationStage::Queued);
        let result = run_start(Arc::clone(&inner), &op, &id, claimed).await;
        // Release the instance lock before announcing completion: a caller that
        // reacts to `OperationFinished` by retrying (a failed start is retryable)
        // must not race a lock that is still held for another few microseconds.
        drop(lock);
        finish_operation(&inner, &op, result);
    });
    Ok(op_id)
}

/// Runs the start and guarantees the row never stays in `Starting`.
///
/// Any error that escapes the body (a store failure, an unreadable credential,
/// a lost compare-and-set) still leaves a terminal status behind, so a user can
/// retry or delete the instance instead of staring at a stuck spinner.
async fn run_start(
    inner: Arc<Inner>,
    op: &OperationId,
    id: &InstanceId,
    instance: Instance,
) -> OpResult<()> {
    let result = run_start_inner(&inner, op, id, &instance).await;
    if result.is_err() {
        settle_failed(&inner, id).await;
    }
    result
}

/// Moves a row that is still `Starting` to `Failed`, keeping any failure record
/// `fail_with_tail` already stored.
async fn settle_failed(inner: &Arc<Inner>, id: &InstanceId) {
    let id2 = id.clone();
    let _ = db_async(inner, move |store| {
        let Some(current) = store.get_instance(&id2)? else {
            return Ok(());
        };
        if current.status() == InstanceStatus::Starting {
            store.update_process_identity(&id2, None, None, None)?;
            store.update_status(&id2, InstanceStatus::Failed)?;
        }
        Ok(())
    })
    .await;
    emit_changed(inner, id);
}

async fn run_start_inner(
    inner: &Arc<Inner>,
    op: &OperationId,
    id: &InstanceId,
    instance: &Instance,
) -> OpResult<()> {
    emit_stage(inner, op, OperationStage::Starting);
    let log_path = log_file_for(&inner.paths, instance);

    // F-PRT-02: probe the port again before anything is executed.
    if let Err(err) = ensure_port_is_free(inner, instance.port(), Some(id)) {
        return fail(inner, id, instance, &log_path, err, StartFailure::PortInUse).await;
    }

    // Rotate the log so a stale tail cannot be mistaken for this attempt.
    let rotate = log_path.clone();
    let _ = blocking(move || crate::process::logs::rotate_if_needed(&rotate)).await;

    // Sync the engine config from the store before every start. This repairs a
    // crash between a config rewrite and the store update, and keeps the config
    // authoritative with the persisted port (D-2).
    let instance_for_sync = instance.clone();
    let paths_for_sync = inner.paths.clone();
    if let Err(err) =
        blocking(move || super::helpers::sync_instance_port(&paths_for_sync, &instance_for_sync))
            .await
    {
        return fail(inner, id, instance, &log_path, err, StartFailure::Unknown).await;
    }

    let credential = read_credential(inner, instance)?;
    let ctx = run_context(&inner.paths, instance, credential);

    // Prepare engine-owned directories, then build the command.
    let engine = instance.engine();
    let adapter = inner.adapter(engine);
    let ctx_for_prepare = ctx.clone();
    if let Err(err) = blocking(move || prepare(adapter.as_ref(), &ctx_for_prepare)).await {
        return fail(inner, id, instance, &log_path, err, StartFailure::Unknown).await;
    }

    let adapter = inner.adapter(engine);
    let command = adapter.spawn_command(&ctx);
    let spawn_inner = Arc::clone(inner);
    let spawn_log = log_path.clone();
    let spawned = match blocking(move || (spawn_inner.spawn)(command, &spawn_log)).await {
        Ok(identity) => identity,
        Err(err) => {
            return fail(
                inner,
                id,
                instance,
                &log_path,
                err,
                StartFailure::BinaryMissing,
            )
            .await;
        }
    };

    // One deadline covers both "the engine publishes its PID" and "the engine
    // answers a health probe": that is the start timeout's whole meaning.
    let deadline = Instant::now() + inner.start_timeout.unwrap_or_else(|| start_timeout(engine));
    let identity =
        match resolve_identity(inner, engine, instance, &spawned, &log_path, deadline).await {
            Some(identity) => identity,
            None => {
                return fail(
                    inner,
                    id,
                    instance,
                    &log_path,
                    DblError::Engine {
                        reason: "the engine process could not be identified".to_owned(),
                    },
                    StartFailure::Unknown,
                )
                .await;
            }
        };

    // Persist the identity in the same step as the spawn result, so a crash at
    // any instant leaves a state reconciliation can interpret.
    {
        let id2 = id.clone();
        let identity_for_store = identity.clone();
        db_async(inner, move |store| {
            store.update_process_identity(
                &id2,
                Some(identity_for_store.pid()),
                Some(identity_for_store.started_at()),
                Some(identity_for_store.exe()),
            )
        })
        .await?;
    }

    match poll_health(inner, instance, &ctx, &identity, deadline).await {
        Ok(()) => {
            let id2 = id.clone();
            db_async(inner, move |store| {
                let current = store
                    .get_instance(&id2)?
                    .ok_or_else(|| DblError::NotFound {
                        kind: "instance",
                        name: id2.as_str().to_owned(),
                    })?;
                let next = current.status().apply(InstanceEvent::HealthCheckPassed)?;
                store.update_status(&id2, next)
            })
            .await?;
            emit_changed(inner, id);
            Ok(())
        }
        Err(err) => {
            let tail = failure_tail(&log_path);
            let diagnosis = super::helpers::diagnose(inner, engine, &tail.join("\n"));
            fail_with_tail(inner, id, err, diagnosis, tail).await
        }
    }
}

fn prepare(adapter: &dyn EngineAdapter, ctx: &RunContext) -> Result<()> {
    match adapter.key() {
        EngineKey::Postgresql => crate::engine::prepare_postgres_run(ctx).map(|_| ()),
        EngineKey::Valkey => crate::engine::prepare_valkey_run(ctx),
        EngineKey::Mysql | EngineKey::Mariadb => Ok(()),
    }
}

async fn poll_health(
    inner: &Arc<Inner>,
    instance: &Instance,
    ctx: &RunContext,
    identity: &ProcessIdentity,
    deadline: Instant,
) -> Result<()> {
    let engine = instance.engine();
    loop {
        // An engine that vanished before becoming healthy is an early exit.
        let probe = Arc::clone(&inner.probe);
        let identity_clone = identity.clone();
        let state = blocking(move || Ok(probe.validate(&identity_clone))).await?;
        if matches!(state, ProcessState::Absent | ProcessState::Reused) {
            return Err(DblError::Engine {
                reason: "the engine process exited before it became healthy".to_owned(),
            });
        }

        let adapter = inner.adapter(engine);
        let ctx_clone = ctx.clone();
        if let Ok(Health::Ready) = blocking(move || adapter.health_check(&ctx_clone)).await {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(DblError::Engine {
                reason: "the engine did not become healthy in time".to_owned(),
            });
        }
        tokio::time::sleep(Duration::from_millis(crate::consts::HEALTH_CHECK_POLL_MS)).await;
    }
}

/// Finds the *server* identity, not the launcher's.
///
/// `pg_ctl` exits once the postmaster is up, so the recorded PID must come from
/// `postmaster.pid` (or `valkey.pid`) and be re-observed from the live table.
///
/// `deadline` is the same start deadline the readiness poll uses: if the engine
/// cannot publish its PID within its start timeout, the start has failed and
/// there is no point waiting longer.
async fn resolve_identity(
    inner: &Arc<Inner>,
    engine: EngineKey,
    instance: &Instance,
    spawned: &ProcessIdentity,
    log_path: &Path,
    deadline: Instant,
) -> Option<ProcessIdentity> {
    match identity_source(engine) {
        IdentitySource::Child => Some(spawned.clone()),
        IdentitySource::PostmasterPid | IdentitySource::ValkeyPid => {
            loop {
                if let Some(pid) = read_engine_pid(inner, engine, instance) {
                    let probe = Arc::clone(&inner.probe);
                    let observed = blocking(move || {
                        Ok(match probe.observe(pid) {
                            crate::process::probe::ProcessObservation::Found {
                                exe: Some(exe),
                                started_at: Some(started_at),
                            } => ProcessIdentity::new(pid, exe, started_at).ok(),
                            _ => None,
                        })
                    })
                    .await
                    .ok()
                    .flatten();
                    if observed.is_some() {
                        return observed;
                    }
                }
                if Instant::now() >= deadline {
                    // No pidfile appeared: fall back to the launcher so the
                    // failure path can still terminate and diagnose.
                    let _ = log_path;
                    return Some(spawned.clone());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

fn read_engine_pid(inner: &Inner, engine: EngineKey, instance: &Instance) -> Option<u32> {
    match engine {
        EngineKey::Postgresql => {
            crate::engine::read_postmaster_pid(&inner.paths.data_dir(instance.name()))
                .ok()
                .map(|pid| pid.pid)
        }
        EngineKey::Valkey => crate::engine::read_pidfile(&inner.paths.run_dir(instance.name()))
            .ok()
            .map(|pid| pid.pid),
        EngineKey::Mysql | EngineKey::Mariadb => None,
    }
}

async fn fail(
    inner: &Arc<Inner>,
    id: &InstanceId,
    _instance: &Instance,
    log_path: &Path,
    err: DblError,
    diagnosis: StartFailure,
) -> OpResult<()> {
    let tail = failure_tail(log_path);
    fail_with_tail(inner, id, err, diagnosis, tail).await
}

/// Terminates whatever is left, records the failure and moves to `Failed`.
///
/// The identity is only cleared when a post-attempt probe proves the process is
/// gone. A failed start whose launcher still runs must keep its identity, or the
/// engine becomes an orphan DBLocal can never stop again.
async fn fail_with_tail(
    inner: &Arc<Inner>,
    id: &InstanceId,
    err: DblError,
    diagnosis: StartFailure,
    tail: Vec<String>,
) -> OpResult<()> {
    let identity = load(inner, id)
        .ok()
        .and_then(|current| identity_of(&current));
    let terminated = terminate_verified(
        inner,
        identity,
        Duration::from_secs(crate::consts::STOP_GRACE_SECS),
    )
    .await;

    let info = failure_info(err.code(), err.user_message(), diagnosis, tail);
    if let Ok(mut failures) = inner.failures.lock() {
        failures.insert(id.clone(), info.clone());
    }

    let id2 = id.clone();
    let still_alive = terminated.state == Stopped::StillAlive;
    let _ = db_async(inner, move |store| {
        if !still_alive {
            store.update_process_identity(&id2, None, None, None)?;
        }
        store.update_status(&id2, InstanceStatus::Failed)
    })
    .await;

    if still_alive {
        tracing::warn!(
            instance = %id,
            "the failed start left a live process; its identity is kept for a later stop"
        );
    }

    if let Ok(current) = load(inner, id)
        && let Ok(mut view) = view_for(inner, &current)
    {
        view = view.with_failure(info);
        inner.emit(ServiceEvent::InstanceChanged { view });
    }
    Err(OpFailure::new(err).with_diagnosis(diagnosis))
}
