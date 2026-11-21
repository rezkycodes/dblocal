//! Instance creation (`F-INS-01/02`, `F-RTM-*`, `F-PRT-01`, `F-CRD-01/03`).

use std::sync::Arc;
use std::time::SystemTime;

use crate::DblError;
use crate::download::{CancellationToken, DownloadOptions, InstallOptions, SourceSpec};
use crate::engine::InitContext;
use crate::error::Result;
use crate::model::{CredentialBackend, Instance, InstanceId, InstanceSpec, InstanceStatus, Port};
use crate::secrets::credential_ref_for;
use crate::service::{OperationId, OperationStage};

use super::catalog_ops::{record_runtime, runtime_is_installed, source_for};
use super::helpers::{
    blocking, db_async, emit_changed, emit_stage, ensure_port_is_free, finish_operation,
    runtime_dir_for, secret_for, validate_spec_boundary,
};
use super::read_ops::suggest_port;
use super::service::{Inner, RuntimeSource};

pub(crate) fn create_instance(inner: &Arc<Inner>, spec: InstanceSpec) -> Result<OperationId> {
    validate_spec_boundary(&spec)?;
    if inner
        .db(|store| store.get_instance_by_name(spec.name().as_str()))?
        .is_some()
    {
        return Err(DblError::AlreadyExists {
            kind: "instance name",
            name: spec.name().as_str().to_owned(),
        });
    }

    // The id is chosen and locked synchronously, so reconciliation (which may
    // be running at the same moment) sees this instance as busy rather than as
    // an operation the app died in the middle of.
    let id = InstanceId::new();
    let lock = inner.ops.lock_instance(&id)?;

    let op = inner.ops.next_id();
    let token = CancellationToken::new();
    inner.ops.register(&op, token.clone())?;

    let inner = Arc::clone(inner);
    let op_task = op.clone();
    inner.handle().spawn(async move {
        let _lock = lock;
        emit_stage(&inner, &op_task, OperationStage::Queued);
        let result = run_create(Arc::clone(&inner), &op_task, id, spec, token).await;
        finish_operation(&inner, &op_task, result);
    });

    Ok(op)
}

async fn run_create(
    inner: Arc<Inner>,
    op: &OperationId,
    id: InstanceId,
    spec: InstanceSpec,
    token: CancellationToken,
) -> super::helpers::OpResult<()> {
    // Port: explicit or suggested, then probed and checked against reservations.
    let port = match spec.port() {
        Some(port) => port,
        None => suggest_port(&inner, spec.engine())?,
    };
    ensure_port_is_free(&inner, port, None)?;
    let spec = spec.with_port(Some(port));

    let source = if runtime_is_installed(&inner, &spec) {
        None
    } else {
        Some(source_for(&inner, &spec)?)
    };

    // Reserve the row so a concurrent create cannot take the name or port.
    let initial = if source.is_some() {
        InstanceStatus::Provisioning
    } else {
        InstanceStatus::Initializing
    };
    let instance = Instance::new(&spec, id.clone(), initial, SystemTime::now());
    {
        let row = instance.clone();
        db_async(&inner, move |store| store.insert_instance(&row)).await?;
    }
    emit_changed(&inner, &id);

    match create_body(&inner, op, &spec, &id, source.as_ref(), &token).await {
        Ok(()) => Ok(()),
        Err(failure) => {
            rollback(&inner, &id, &spec).await;
            Err(failure)
        }
    }
}

async fn create_body(
    inner: &Arc<Inner>,
    op: &OperationId,
    spec: &InstanceSpec,
    id: &InstanceId,
    source: Option<&RuntimeSource>,
    token: &CancellationToken,
) -> super::helpers::OpResult<()> {
    let instance_name = spec.name().clone();
    let data_dir = inner.paths.data_dir(&instance_name);
    let logs_dir = inner.paths.logs_dir(&instance_name);
    let run_dir = inner.paths.run_dir(&instance_name);

    if let Some(source) = source {
        check_cancel(token)?;
        emit_stage(inner, op, OperationStage::Downloading);
        install_runtime(inner, spec, source, op, token).await?;
        check_cancel(token)?;
        record_runtime(inner, spec)?;

        emit_stage(inner, op, OperationStage::Initializing);
        transition(
            inner,
            id,
            InstanceStatus::Provisioning,
            InstanceStatus::Initializing,
        )
        .await?;
    } else {
        emit_stage(inner, op, OperationStage::Initializing);
    }

    check_cancel(token)?;

    for dir in [&data_dir, &logs_dir, &run_dir] {
        super::helpers::ensure_private_dir(dir)?;
    }

    let password = secret_for(spec.auth())?;
    let init_ctx = InitContext {
        instance_name: instance_name.as_str().to_owned(),
        runtime_dir: runtime_dir_for(&inner.paths, spec.engine(), spec.version()),
        data_dir: data_dir.clone(),
        port: spec
            .port()
            .unwrap_or_else(|| Port::assume_valid(spec.engine().canonical_port())),
        superuser: spec.engine().default_superuser().to_owned(),
        auth: spec.auth(),
        password: password.clone(),
    };

    let engine = spec.engine();
    let init_inner = Arc::clone(inner);
    blocking(move || init_inner.adapter(engine).initialize(&init_ctx)).await?;

    check_cancel(token)?;

    // The secret is stored before the row can claim the instance exists.
    if let Some(secret) = password.as_ref() {
        let placeholder = credential_ref_for(
            id,
            CredentialBackend::File,
            spec.engine().default_superuser(),
        );
        let backend = inner.secrets.set(&placeholder, secret)?;
        let reference = credential_ref_for(id, backend, spec.engine().default_superuser());
        db_async(inner, move |store| store.set_credential(&reference)).await?;
    }

    transition(
        inner,
        id,
        InstanceStatus::Initializing,
        InstanceStatus::Stopped,
    )
    .await?;
    Ok(())
}

/// Moves the row from `from` to `to`, refusing if anyone else already moved it.
async fn transition(
    inner: &Arc<Inner>,
    id: &InstanceId,
    from: InstanceStatus,
    to: InstanceStatus,
) -> super::helpers::OpResult<()> {
    let id2 = id.clone();
    let moved = db_async(inner, move |store| store.transition_status(&id2, from, to)).await?;
    if !moved {
        return Err(crate::DblError::InvalidTransition {
            from: "busy",
            to: to.as_str(),
        }
        .into());
    }
    emit_changed(inner, id);
    Ok(())
}

async fn install_runtime(
    inner: &Arc<Inner>,
    spec: &InstanceSpec,
    source: &RuntimeSource,
    op: &OperationId,
    token: &CancellationToken,
) -> super::helpers::OpResult<()> {
    let final_dir = inner
        .paths
        .runtime_dir(spec.engine(), spec.version(), inner.arch)
        .map_err(|_| DblError::InvalidInput {
            field: "runtime",
            reason: "runtime path is invalid".to_owned(),
        })?;
    let downloads_dir = inner.paths.downloads_dir();

    let source_spec = SourceSpec {
        url: &source.url,
        label: &source.id,
        sha256: &source.sha256,
        size_bytes: source.size_bytes,
        format: source.format,
        strip_prefix: source.strip_prefix.as_deref(),
    };

    // `install_source_to` runs download -> verify -> extract as one fused
    // pipeline (that ordering is what makes it safe), so the stages are emitted
    // from the observable progress: reaching the expected byte count means the
    // download is done and verification is starting. `Verifying` is emitted only
    // once the bytes have arrived, so a failed download never claims to verify.
    let expected: Vec<String> = if source.expected_binaries.is_empty() {
        expected_binaries(spec.engine())
            .into_iter()
            .map(str::to_owned)
            .collect()
    } else {
        source.expected_binaries.clone()
    };
    let expected_refs: Vec<&str> = expected.iter().map(String::as_str).collect();

    let watcher_events = inner.events.clone();
    let watcher_op = op.clone();
    let watcher_total = source.size_bytes;
    let watcher = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher_flag = std::sync::Arc::clone(&watcher);
    let progress = std::sync::Arc::new(move |event: crate::download::DownloadEvent| {
        if let crate::download::DownloadEvent::Progress(sample) = event {
            let _ = watcher_events.send(crate::service::ServiceEvent::DownloadProgress {
                id: watcher_op.clone(),
                progress: sample,
            });
            if sample.bytes_done >= watcher_total
                && !watcher_flag.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                let _ = watcher_events.send(crate::service::ServiceEvent::OperationProgress {
                    id: watcher_op.clone(),
                    stage: OperationStage::Verifying,
                });
            }
        }
    });

    let options = InstallOptions {
        download: DownloadOptions {
            policy: inner.download_policy.clone(),
            cancel: token.clone(),
            ..DownloadOptions::default()
        },
        progress: Some(progress),
        free_space: None,
    };
    crate::download::install_source_to(
        &source_spec,
        &downloads_dir,
        &final_dir,
        &expected_refs,
        options,
    )
    .await?;

    // Reaching here means the archive verified and was renamed into place.
    if !watcher.load(std::sync::atomic::Ordering::SeqCst) {
        emit_stage(inner, op, OperationStage::Verifying);
    }
    emit_stage(inner, op, OperationStage::Extracting);
    Ok(())
}

fn expected_binaries(engine: crate::model::EngineKey) -> Vec<&'static str> {
    match engine {
        crate::model::EngineKey::Postgresql => vec!["bin/postgres", "bin/initdb", "bin/pg_ctl"],
        crate::model::EngineKey::Valkey => vec!["bin/valkey-server"],
        crate::model::EngineKey::Mysql => vec!["bin/mysqld"],
        crate::model::EngineKey::Mariadb => vec!["bin/mariadbd", "bin/mysqld"],
    }
}

fn check_cancel(token: &CancellationToken) -> super::helpers::OpResult<()> {
    if token.is_cancelled() {
        Err(DblError::Cancelled {
            operation: "create".to_owned(),
        }
        .into())
    } else {
        Ok(())
    }
}

/// Removes everything a failed create could have produced, then tells consumers
/// the row is gone.
///
/// Delegates to [`super::helpers::rollback_create`], which re-validates every
/// path and refuses to delete when one errs, so the check is not decorative.
async fn rollback(inner: &Arc<Inner>, id: &InstanceId, spec: &InstanceSpec) {
    let removed = super::helpers::rollback_create(inner, id, spec.name()).await;
    if removed {
        super::helpers::emit_removed(inner, id);
    }
}
