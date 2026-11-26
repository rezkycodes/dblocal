//! Async operation implementations for [`super::MockService`].

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;

use crate::DblError;
use crate::error::{CODE_ENGINE, CODE_NETWORK, CODE_PORT_IN_USE, Result};
use crate::model::{
    AuthMode, DeleteMode, DownloadProgress, EngineKey, Instance, InstanceEvent, InstanceId,
    InstanceSpec, InstanceStatus, Port, StartFailure,
};
use crate::secret::SecretString;
use crate::service::{
    DblService, OperationId, OperationStage, ServiceEvent, UriFormat, UserFacingError,
};
use crate::strings;

use super::{Inner, MockService, ScriptedFailure, invalid_transition, lock, not_found};

fn emit_cancelled(tx: &broadcast::Sender<ServiceEvent>, op: &OperationId) {
    let _ = tx.send(ServiceEvent::OperationFinished {
        id: op.clone(),
        result: Err(UserFacingError::from(&DblError::Cancelled {
            operation: "operation".to_owned(),
        })),
    });
}

fn emit_port_failure(
    tx: &broadcast::Sender<ServiceEvent>,
    inner: &Arc<std::sync::Mutex<Inner>>,
    op: &OperationId,
    id: &InstanceId,
    port: u16,
) {
    let mut g = lock(inner);
    let msg = strings::ERR_PORT_IN_USE.to_owned();
    g.set_failed(id, CODE_PORT_IN_USE, msg.clone(), StartFailure::PortInUse);
    g.push_log(id, format!("[error] port {port} is already in use"));
    let _ = tx.send(ServiceEvent::OperationFinished {
        id: op.clone(),
        result: Err(UserFacingError::from(&DblError::PortInUse { port })
            .with_diagnosis(StartFailure::PortInUse)),
    });
}

/// Rolls back a half-created instance: the row is removed and
/// `InstanceRemoved` is emitted, exactly as the real service does when a
/// create fails before the instance exists.
fn roll_back_create(
    tx: &broadcast::Sender<ServiceEvent>,
    inner: &Arc<std::sync::Mutex<Inner>>,
    op: &OperationId,
    id: &InstanceId,
) {
    let mut g = lock(inner);
    g.instances.remove(id);
    g.views.remove(id);
    g.credentials.remove(id);
    g.passwords.remove(id);
    g.logs.remove(id);
    g.watched.remove(id);
    g.emit(ServiceEvent::InstanceRemoved { id: id.clone() });
    let _ = tx.send(ServiceEvent::OperationFinished {
        id: op.clone(),
        result: Err(UserFacingError::from(&DblError::Engine {
            reason: "create rolled back".to_owned(),
        })),
    });
}

fn emit_download_failure(
    tx: &broadcast::Sender<ServiceEvent>,
    inner: &Arc<std::sync::Mutex<Inner>>,
    op: &OperationId,
    id: &InstanceId,
) {
    let mut g = lock(inner);
    let msg = strings::ERR_NETWORK.to_owned();
    g.set_failed(id, CODE_NETWORK, msg.clone(), StartFailure::Unknown);
    g.push_log(id, "[error] download failed".to_owned());
    let _ = tx.send(ServiceEvent::OperationFinished {
        id: op.clone(),
        result: Err(UserFacingError::from(&DblError::Network {
            reason: "download interrupted".to_owned(),
        })),
    });
}

fn finish_cancelled(
    tx: &broadcast::Sender<ServiceEvent>,
    inner: &Arc<std::sync::Mutex<Inner>>,
    op: &OperationId,
    id: &InstanceId,
) {
    let mut g = lock(inner);
    g.emit(ServiceEvent::OperationFinished {
        id: op.clone(),
        result: Err(UserFacingError::from(&DblError::Cancelled {
            operation: "operation".to_owned(),
        })),
    });
    let should_fail = g.instances.get(id).is_some_and(|inst| {
        matches!(
            inst.status(),
            InstanceStatus::Provisioning | InstanceStatus::Initializing | InstanceStatus::Starting
        )
    });
    if should_fail {
        if let Some(inst) = g.instances.get(id).cloned() {
            g.instances
                .insert(id.clone(), inst.with_status(InstanceStatus::Failed));
        }
        g.push_log(id, "[warn] cancelled".to_owned());
        g.sync_view(id);
    }
    let _ = tx;
}

impl DblService for MockService {
    fn list_instances(&self) -> Result<Vec<crate::service::InstanceView>> {
        Ok(lock(&self.inner).views.values().cloned().collect())
    }

    fn catalog(&self) -> Result<crate::service::CatalogView> {
        Ok(lock(&self.inner).catalog_view())
    }

    fn refresh_catalog(&self, _force: bool) -> Result<OperationId> {
        let (op, delay, tx, inner) = {
            let mut g = lock(&self.inner);
            let op = g.alloc_op(super::OpKind::RefreshCatalog);
            g.emit(ServiceEvent::OperationProgress {
                id: op.clone(),
                stage: OperationStage::Queued,
            });
            (op, g.step_delay, g.tx.clone(), Arc::clone(&self.inner))
        };
        let op_task = op.clone();
        self.spawn(&op, async move {
            MockService::step(delay).await;
            let _ = tx.send(ServiceEvent::OperationProgress {
                id: op_task.clone(),
                stage: OperationStage::Starting,
            });
            MockService::step(delay).await;
            if MockService::cancelled(&inner, &op_task) {
                emit_cancelled(&tx, &op_task);
                return;
            }
            {
                let mut g = lock(&inner);
                g.catalog_status = crate::model::CatalogStatus::Fresh;
            }
            let _ = tx.send(ServiceEvent::CatalogUpdated {
                status: crate::model::CatalogStatus::Fresh,
            });
            let _ = tx.send(ServiceEvent::OperationFinished {
                id: op_task,
                result: Ok(()),
            });
        });
        Ok(op)
    }

    fn validate_spec(&self, spec: &InstanceSpec) -> Result<()> {
        let g = lock(&self.inner);
        // Mirror the real service's platform gate (D-26): an engine with no
        // build for this OS is `DBL-6001`, so `create valkey` on a Windows mock
        // reports exactly what a Windows user sees.
        if super::catalog::unavailable_reason(spec.engine(), g.platform.os, g.platform.arch)
            .is_some()
        {
            return Err(DblError::Unsupported {
                reason: format!(
                    "{} has no build for {} {}",
                    spec.engine().display_name(),
                    g.platform.os,
                    g.platform.arch
                ),
            });
        }
        let name = spec.name().as_str();
        for view in g.views.values() {
            if view.name == name {
                return Err(DblError::AlreadyExists {
                    kind: "instance",
                    name: name.to_owned(),
                });
            }
            if let Some(port) = spec.port()
                && view.port == port
            {
                return Err(DblError::InvalidInput {
                    field: "port",
                    reason: format!(
                        "port {} is already allocated to instance {}",
                        port.get(),
                        view.name
                    ),
                });
            }
        }
        Ok(())
    }

    fn suggest_port(&self, engine: EngineKey) -> Result<Port> {
        let g = lock(&self.inner);
        let used: std::collections::HashSet<u16> = g.views.values().map(|v| v.port.get()).collect();
        let mut candidate = engine.canonical_port();
        while used.contains(&candidate) {
            candidate = candidate.saturating_add(1);
            if candidate < crate::consts::MIN_PORT {
                candidate = crate::consts::MIN_PORT;
            }
        }
        Port::new(candidate)
    }

    fn create_instance(&self, spec: InstanceSpec) -> Result<OperationId> {
        self.validate_spec(&spec)?;
        let (op, id, needs_download, delay, tx, inner, scripted) = {
            let mut g = lock(&self.inner);
            let id = InstanceId::new();
            let op = g.alloc_op(super::OpKind::Create);
            let scripted = g.take_failure(|f| {
                matches!(
                    f,
                    ScriptedFailure::DownloadFailed
                        | ScriptedFailure::PortInUse { .. }
                        | ScriptedFailure::CreateRolledBack
                )
            });
            let needs_download = !g
                .runtimes
                .contains(&(spec.engine(), spec.version().to_owned()));
            let created = g.det.now();
            let initial = if needs_download {
                InstanceStatus::Provisioning
            } else {
                InstanceStatus::Initializing
            };
            let instance = Instance::new(&spec, id.clone(), initial, created);
            if spec.auth() == AuthMode::Password {
                g.credentials
                    .insert(id.clone(), crate::model::CredentialBackend::Keychain);
                let pwd = SecretString::new(format!("pwd-{:08x}", g.det.next_u64() as u32));
                g.passwords.insert(id.clone(), pwd);
            }
            g.logs.insert(
                id.clone(),
                std::collections::VecDeque::from([format!(
                    "[info] creating instance {} ({})",
                    spec.name(),
                    spec.version()
                )]),
            );
            g.instances.insert(id.clone(), instance);
            g.sync_view(&id);
            g.emit(ServiceEvent::OperationProgress {
                id: op.clone(),
                stage: OperationStage::Queued,
            });
            (
                op,
                id,
                needs_download,
                g.step_delay,
                g.tx.clone(),
                Arc::clone(&self.inner),
                scripted,
            )
        };

        let op_task = op.clone();
        let iid = id;
        let spec_engine = spec.engine();
        let spec_version = spec.version().to_owned();
        self.spawn(&op, async move {
            MockService::step(delay).await;
            if MockService::cancelled(&inner, &op_task) {
                finish_cancelled(&tx, &inner, &op_task, &iid);
                return;
            }
            if let Some(ScriptedFailure::CreateRolledBack) = scripted {
                roll_back_create(&tx, &inner, &op_task, &iid);
                return;
            }
            if let Some(ScriptedFailure::PortInUse { port }) = scripted {
                emit_port_failure(&tx, &inner, &op_task, &iid, port);
                return;
            }
            if needs_download {
                let _ = tx.send(ServiceEvent::OperationProgress {
                    id: op_task.clone(),
                    stage: OperationStage::Downloading,
                });
                let total = 52 * 1024 * 1024u64;
                let bps = 13 * 1024 * 1024u64;
                for step in 1..=5u64 {
                    MockService::step(delay).await;
                    if MockService::cancelled(&inner, &op_task) {
                        finish_cancelled(&tx, &inner, &op_task, &iid);
                        return;
                    }
                    let done = (total * step) / 5;
                    let eta = Duration::from_secs(5u64.saturating_sub(step));
                    let _ = tx.send(ServiceEvent::DownloadProgress {
                        id: op_task.clone(),
                        progress: DownloadProgress::new(done, total, bps, Some(eta)),
                    });
                    lock(&inner).push_log(&iid, format!("[info] downloading {done}/{total} bytes"));
                }
                let _ = tx.send(ServiceEvent::OperationProgress {
                    id: op_task.clone(),
                    stage: OperationStage::Verifying,
                });
                MockService::step(delay).await;
                if scripted == Some(ScriptedFailure::DownloadFailed) {
                    emit_download_failure(&tx, &inner, &op_task, &iid);
                    return;
                }
                let _ = tx.send(ServiceEvent::OperationProgress {
                    id: op_task.clone(),
                    stage: OperationStage::Extracting,
                });
                MockService::step(delay).await;
                {
                    let mut g = lock(&inner);
                    g.runtimes.insert((spec_engine, spec_version.clone()));
                    g.apply(&iid, InstanceEvent::DownloadVerified);
                    g.push_log(&iid, "[info] runtime installed and verified".to_owned());
                    g.sync_view(&iid);
                }
            }
            let _ = tx.send(ServiceEvent::OperationProgress {
                id: op_task.clone(),
                stage: OperationStage::Initializing,
            });
            MockService::step(delay).await;
            if MockService::cancelled(&inner, &op_task) {
                finish_cancelled(&tx, &inner, &op_task, &iid);
                return;
            }
            {
                let mut g = lock(&inner);
                if g.apply(&iid, InstanceEvent::InitSucceeded) {
                    g.push_log(&iid, "[info] initialization finished".to_owned());
                    g.sync_view(&iid);
                }
            }
            let _ = tx.send(ServiceEvent::OperationFinished {
                id: op_task,
                result: Ok(()),
            });
        });
        Ok(op)
    }

    fn start_instance(&self, id: &InstanceId) -> Result<OperationId> {
        let (op, delay, tx, inner, scripted) = {
            let mut g = lock(&self.inner);
            let inst = g
                .instances
                .get(id)
                .cloned()
                .ok_or_else(|| not_found("instance", id))?;
            if !matches!(
                inst.status(),
                InstanceStatus::Stopped | InstanceStatus::Failed
            ) {
                return Err(invalid_transition(inst.status(), "starting"));
            }
            let op = g.alloc_op(super::OpKind::Start);
            let scripted = g.take_failure(|f| {
                matches!(
                    f,
                    ScriptedFailure::StartTimeout | ScriptedFailure::PortInUse { .. }
                )
            });
            if let Ok(next) = inst.status().apply(InstanceEvent::StartRequested) {
                g.instances.insert(id.clone(), inst.with_status(next));
            }
            g.push_log(id, "[info] starting the engine".to_owned());
            g.sync_view(id);
            (
                op,
                g.step_delay,
                g.tx.clone(),
                Arc::clone(&self.inner),
                scripted,
            )
        };
        let op_task = op.clone();
        let iid = id.clone();
        self.spawn(&op, async move {
            MockService::step(delay).await;
            let _ = tx.send(ServiceEvent::OperationProgress {
                id: op_task.clone(),
                stage: OperationStage::Starting,
            });
            MockService::step(delay).await;
            if MockService::cancelled(&inner, &op_task) {
                finish_cancelled(&tx, &inner, &op_task, &iid);
                return;
            }
            match scripted {
                Some(ScriptedFailure::PortInUse { port }) => {
                    emit_port_failure(&tx, &inner, &op_task, &iid, port);
                    return;
                }
                Some(ScriptedFailure::StartTimeout) => {
                    let mut g = lock(&inner);
                    let msg = strings::ERR_ENGINE.to_owned();
                    g.set_failed(&iid, CODE_ENGINE, msg.clone(), StartFailure::Unknown);
                    g.push_log(&iid, "[error] health check timeout 30s".to_owned());
                    let _ = tx.send(ServiceEvent::OperationFinished {
                        id: op_task.clone(),
                        result: Err(UserFacingError {
                            code: CODE_ENGINE.to_owned(),
                            message: msg,
                            diagnosis: Some(StartFailure::Unknown),
                        }),
                    });
                    return;
                }
                _ => {}
            }
            {
                let mut g = lock(&inner);
                if g.apply(&iid, InstanceEvent::HealthCheckPassed) {
                    g.push_log(
                        &iid,
                        "[info] health check passed, the engine is running".to_owned(),
                    );
                    g.sync_view(&iid);
                }
            }
            let _ = tx.send(ServiceEvent::OperationFinished {
                id: op_task,
                result: Ok(()),
            });
        });
        Ok(op)
    }

    fn stop_instance(&self, id: &InstanceId) -> Result<OperationId> {
        let (op, delay, tx, inner, forced) = {
            let mut g = lock(&self.inner);
            let inst = g
                .instances
                .get(id)
                .cloned()
                .ok_or_else(|| not_found("instance", id))?;
            if inst.status() != InstanceStatus::Running {
                return Err(invalid_transition(inst.status(), "stopping"));
            }
            let op = g.alloc_op(super::OpKind::Stop);
            // A scripted forced stop models an engine that ignores the graceful
            // shutdown until the grace period elapses (F-INS-03 AC3).
            let forced = g
                .take_failure(|f| matches!(f, ScriptedFailure::StopNeedsForce))
                .is_some();
            if let Ok(next) = inst.status().apply(InstanceEvent::StopRequested) {
                g.instances.insert(id.clone(), inst.with_status(next));
            }
            g.push_log(id, "[info] stopping the engine".to_owned());
            g.sync_view(id);
            (
                op,
                g.step_delay,
                g.tx.clone(),
                Arc::clone(&self.inner),
                forced,
            )
        };
        let op_task = op.clone();
        let iid = id.clone();
        self.spawn(&op, async move {
            MockService::step(delay).await;
            if forced {
                // `StopForced` goes out before the final `InstanceChanged` that
                // carries the `Stopped` view, so the UI can toast and still end
                // in the right state.
                let _ = tx.send(ServiceEvent::StopForced { id: iid.clone() });
                lock(&inner).push_log(
                    &iid,
                    "[warn] shutdown did not finish within 20 seconds, forcing a kill".to_owned(),
                );
            }
            {
                let mut g = lock(&inner);
                if g.apply(&iid, InstanceEvent::ShutdownClean) {
                    g.push_log(&iid, "[info] engine stopped".to_owned());
                    g.sync_view(&iid);
                }
            }
            let _ = tx.send(ServiceEvent::OperationFinished {
                id: op_task,
                result: Ok(()),
            });
        });
        Ok(op)
    }

    fn delete_instance(&self, id: &InstanceId, mode: DeleteMode) -> Result<OperationId> {
        let (op, delay, tx, inner) = {
            let mut g = lock(&self.inner);
            let inst = g
                .instances
                .get(id)
                .cloned()
                .ok_or_else(|| not_found("instance", id))?;
            if !matches!(
                inst.status(),
                InstanceStatus::Stopped | InstanceStatus::Failed
            ) {
                return Err(invalid_transition(inst.status(), "deleting"));
            }
            let op = g.alloc_op(super::OpKind::Delete);
            if let Ok(next) = inst.status().apply(InstanceEvent::DeleteRequested) {
                g.instances.insert(id.clone(), inst.with_status(next));
            }
            g.push_log(
                id,
                format!("[info] deleting the instance ({})", mode.as_str()),
            );
            g.sync_view(id);
            (op, g.step_delay, g.tx.clone(), Arc::clone(&self.inner))
        };
        let op_task = op.clone();
        let iid = id.clone();
        self.spawn(&op, async move {
            MockService::step(delay).await;
            {
                let mut g = lock(&inner);
                if mode == DeleteMode::ArchiveData
                    && let Some(inst) = g.instances.get(&iid).cloned()
                {
                    let archive_id = g.det.gen_id("archive");
                    let archived = crate::model::ArchivedData::new(
                        archive_id,
                        inst.name().clone(),
                        inst.engine(),
                        inst.version().to_owned(),
                        std::path::PathBuf::from(format!("/tmp/archive/{}", inst.name())),
                        41 * 1024 * 1024,
                        g.det.now(),
                    );
                    g.archives.insert(archived.id().to_owned(), archived);
                }
                g.instances.remove(&iid);
                g.views.remove(&iid);
                g.credentials.remove(&iid);
                g.passwords.remove(&iid);
                g.logs.remove(&iid);
                g.watched.remove(&iid);
                // The row is gone; `InstanceChanged` cannot carry that, so tell
                // consumers explicitly before the operation finishes.
                g.emit(ServiceEvent::InstanceRemoved { id: iid.clone() });
            }
            let _ = tx.send(ServiceEvent::OperationFinished {
                id: op_task,
                result: Ok(()),
            });
        });
        Ok(op)
    }

    fn change_port(&self, id: &InstanceId, port: Port) -> Result<()> {
        let mut g = lock(&self.inner);
        let inst = g
            .instances
            .get(id)
            .cloned()
            .ok_or_else(|| not_found("instance", id))?;
        for view in g.views.values() {
            if &view.id != id && view.port == port {
                return Err(DblError::InvalidInput {
                    field: "port",
                    reason: format!(
                        "port {} is already allocated to instance {}",
                        port.get(),
                        view.name
                    ),
                });
            }
        }
        g.instances.insert(id.clone(), inst.with_port(port));
        g.push_log(id, format!("[info] port changed to {}", port.get()));
        g.sync_view(id);
        Ok(())
    }

    fn data_size(&self, id: &InstanceId) -> Result<u64> {
        let g = lock(&self.inner);
        if g.instances.contains_key(id) {
            Ok(41 * 1024 * 1024)
        } else {
            Err(not_found("instance", id))
        }
    }

    fn set_autostart(&self, id: &InstanceId, enabled: bool) -> Result<()> {
        let mut g = lock(&self.inner);
        let inst = g
            .instances
            .get(id)
            .cloned()
            .ok_or_else(|| not_found("instance", id))?;
        g.instances.insert(id.clone(), inst.with_autostart(enabled));
        g.sync_view(id);
        Ok(())
    }

    fn cancel_operation(&self, id: &OperationId) -> Result<()> {
        let mut g = lock(&self.inner);
        match g.operations.get_mut(id) {
            Some(record) => {
                record.cancelled = true;
                Ok(())
            }
            None => Err(not_found("operation", id)),
        }
    }

    fn connection_uri(&self, id: &InstanceId, format: UriFormat) -> Result<SecretString> {
        let g = lock(&self.inner);
        let inst = g
            .instances
            .get(id)
            .ok_or_else(|| not_found("instance", id))?;
        // Same rule as the real service (item 10): a no-password instance has a
        // URI without a password; a password instance without a stored secret is
        // a typed `credential` error, never a silent empty password. The mock
        // renders through its own copy of the format (`super::uri`) so it stays
        // an independent oracle for the core builder.
        let password = if inst.auth() == AuthMode::Password {
            g.passwords
                .get(id)
                .ok_or_else(|| not_found("credential", id))?
                .expose()
                .to_owned()
        } else {
            String::new()
        };
        let rendered = match format {
            UriFormat::Uri => super::uri::uri(
                inst.engine(),
                inst.port(),
                inst.superuser(),
                &password,
                inst.auth(),
            ),
            UriFormat::DotEnv => super::uri::dotenv(
                inst.engine(),
                inst.port(),
                inst.superuser(),
                &password,
                inst.auth(),
            ),
        };
        Ok(SecretString::new(rendered))
    }

    fn tail_logs(&self, id: &InstanceId, lines: usize) -> Result<Vec<String>> {
        let g = lock(&self.inner);
        if !g.instances.contains_key(id) {
            return Err(not_found("instance", id));
        }
        Ok(g.tail(id, lines))
    }

    fn watch_logs(&self, id: &InstanceId, enabled: bool) -> Result<()> {
        let mut g = lock(&self.inner);
        if !g.instances.contains_key(id) {
            return Err(not_found("instance", id));
        }
        if enabled {
            g.watched.insert(id.clone());
        } else {
            g.watched.remove(id);
        }
        Ok(())
    }

    fn list_runtimes(&self) -> Result<Vec<crate::model::Runtime>> {
        let g = lock(&self.inner);
        Ok(g.runtimes
            .iter()
            .map(|(engine, version)| {
                crate::model::Runtime::new(
                    format!("rt-{}-{}", engine.as_str(), version),
                    *engine,
                    version.clone(),
                    crate::platform::Arch::current(),
                    std::path::PathBuf::from(format!("/tmp/runtime/{}", engine.as_str())),
                    40 * 1024 * 1024,
                    g.det.now(),
                    crate::model::RuntimeState::Installed,
                )
            })
            .collect())
    }

    fn remove_runtime(&self, runtime: &str) -> Result<()> {
        let mut g = lock(&self.inner);
        let target = g
            .runtimes
            .iter()
            .find(|(engine, version)| format!("rt-{}-{}", engine.as_str(), version) == runtime)
            .cloned();
        match target {
            Some(target) => {
                g.runtimes.remove(&target);
                Ok(())
            }
            None => Err(not_found("runtime", runtime)),
        }
    }

    fn list_archives(&self) -> Result<Vec<crate::model::ArchivedData>> {
        Ok(lock(&self.inner).archives.values().cloned().collect())
    }

    fn purge_archive(&self, archive: &str) -> Result<()> {
        let mut g = lock(&self.inner);
        match g.archives.remove(archive) {
            Some(_) => Ok(()),
            None => Err(not_found("archive", archive)),
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<ServiceEvent> {
        lock(&self.inner).tx.subscribe()
    }

    // --- Database & user administration (F-DBU) -----------------------------

    fn list_databases(
        &self,
        id: &InstanceId,
    ) -> crate::service::ServiceResult<Vec<crate::admin::DatabaseInfo>> {
        super::admin::list_databases(&self.inner, id)
    }

    fn create_database(&self, id: &InstanceId, name: &str) -> crate::service::ServiceResult<()> {
        super::admin::create_database(&self.inner, id, name)
    }

    fn drop_database(
        &self,
        id: &InstanceId,
        name: &str,
        force: bool,
    ) -> crate::service::ServiceResult<()> {
        super::admin::drop_database(&self.inner, id, name, force)
    }

    fn list_users(
        &self,
        id: &InstanceId,
    ) -> crate::service::ServiceResult<Vec<crate::admin::UserInfo>> {
        super::admin::list_users(&self.inner, id)
    }

    fn create_user(
        &self,
        id: &InstanceId,
        name: &str,
        db: &str,
        level: crate::admin::PrivilegeLevel,
    ) -> crate::service::ServiceResult<SecretString> {
        super::admin::create_user(&self.inner, id, name, db, level)
    }

    fn reset_superuser_password(
        &self,
        id: &InstanceId,
    ) -> crate::service::ServiceResult<SecretString> {
        super::admin::reset_superuser_password(&self.inner, id)
    }

    // --- Config editor (F-CFG) ----------------------------------------------

    fn config_view(
        &self,
        id: &InstanceId,
    ) -> crate::service::ServiceResult<crate::config::ConfigView> {
        super::config::view(&self.inner, id)
    }

    fn config_apply(
        &self,
        id: &InstanceId,
        changes: Vec<(String, String)>,
    ) -> crate::service::ServiceResult<crate::config::ConfigApplied> {
        super::config::apply(&self.inner, id, &changes)
    }

    fn config_restore(&self, id: &InstanceId) -> crate::service::ServiceResult<()> {
        super::config::restore(&self.inner, id)
    }

    fn config_raw_path(
        &self,
        id: &InstanceId,
    ) -> crate::service::ServiceResult<std::path::PathBuf> {
        super::config::raw_path(&self.inner, id)
    }

    fn config_open_raw(
        &self,
        id: &InstanceId,
    ) -> crate::service::ServiceResult<crate::config::OpenCommand> {
        super::config::open_raw(&self.inner, id)
    }

    // --- Updates (F-PKG-05) -------------------------------------------------

    fn check_update(&self) -> crate::service::ServiceResult<crate::service::UpdateStatus> {
        super::update::check_update(&self.inner)
    }

    fn check_update_if_due(
        &self,
    ) -> crate::service::ServiceResult<Option<crate::service::UpdateStatus>> {
        super::update::check_update_if_due(&self.inner)
    }

    fn start_update(&self) -> crate::service::ServiceResult<OperationId> {
        super::update::start_update(&self.inner)
    }

    fn apply_update(&self) -> crate::service::ServiceResult<()> {
        super::update::apply_update(&self.inner)
    }

    fn restart_for_update(&self) -> crate::service::ServiceResult<()> {
        Ok(())
    }

    // --- Open in client (F-OPN) ---------------------------------------------

    fn detect_clients(
        &self,
        refresh: bool,
    ) -> crate::service::ServiceResult<Vec<crate::clients::DetectedClient>> {
        super::clients_mock::detect(&self.inner, refresh)
    }

    fn open_in_client(
        &self,
        id: &InstanceId,
        client: crate::clients::ClientKind,
        dry_run: bool,
    ) -> crate::service::ServiceResult<crate::clients::LaunchPlan> {
        super::clients_mock::open(&self.inner, id, client, dry_run)
    }
}
