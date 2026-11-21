//! Synchronous reads and small mutations.

use std::time::SystemTime;

use crate::DblError;
use crate::catalog::InstalledVersions;
use crate::engine::Credential;
use crate::error::Result;
use crate::model::{EngineKey, Instance, InstanceId, InstanceSpec, Port};
use crate::ports::{PortChecker, suggest_port_with};
use crate::secret::SecretString;
use crate::service::{CatalogView, InstanceView, OperationId, UriFormat};

use super::helpers::{
    dir_size, emit_changed, ensure_port_is_free, log_file_for, reserved_ports, run_context,
    validate_spec_boundary, view_for,
};
use super::service::Inner;

pub(crate) fn list_instances(inner: &Inner) -> Result<Vec<InstanceView>> {
    let instances = inner.db(|store| store.list_instances())?;
    instances
        .iter()
        .map(|instance| view_for(inner, instance))
        .collect()
}

pub(crate) fn catalog(inner: &Inner) -> Result<CatalogView> {
    let state = inner
        .catalog
        .lock()
        .map_err(|_| DblError::Storage {
            reason: "the catalog lock was poisoned".to_owned(),
        })?
        .clone();
    let installed = InstalledVersions::from_pairs(
        inner
            .db(|store| store.list_runtimes())?
            .into_iter()
            .map(|runtime| (runtime.engine(), runtime.version().to_owned())),
    );
    Ok(crate::catalog::build_catalog_view(
        state.snapshot(),
        inner.os,
        inner.arch,
        &installed,
        inner.host_glibc,
    ))
}

pub(crate) fn validate_spec(inner: &Inner, spec: &InstanceSpec) -> Result<()> {
    validate_spec_boundary(spec)?;
    let existing = inner.db(|store| store.get_instance_by_name(spec.name().as_str()))?;
    if existing.is_some() {
        return Err(DblError::AlreadyExists {
            kind: "instance name",
            name: spec.name().as_str().to_owned(),
        });
    }
    if let Some(port) = spec.port() {
        ensure_port_is_free(inner, port, None)?;
    }
    Ok(())
}

/// Adapter that lets `ports::suggest_port_with` consult the service's own
/// probe, so tests can make port availability deterministic.
struct ProbeChecker<'a>(&'a Inner);

impl PortChecker for ProbeChecker<'_> {
    fn is_free(&self, port: Port) -> bool {
        (self.0.port_free)(port.get())
    }
}

pub(crate) fn suggest_port(inner: &Inner, engine: EngineKey) -> Result<Port> {
    let reserved = reserved_ports(inner)?;
    Ok(suggest_port_with(engine, &reserved, &ProbeChecker(inner))?.port())
}

pub(crate) fn change_port(inner: &Inner, id: &InstanceId, port: Port) -> Result<()> {
    let instance = load(inner, id)?;
    if instance.status().holds_process() {
        return Err(DblError::InvalidTransition {
            from: instance.status().as_str(),
            to: "stopped",
        });
    }
    ensure_port_is_free(inner, port, Some(id))?;

    // Rewrite the engine config before persisting the new port. If the rewrite
    // fails, neither the file nor the store is changed. The config only exists
    // after initialisation; before that (Provisioning/initialising edge cases are
    // not reachable because holds_process() is false only for Stopped/Failed and
    // create never leaves a port-bearing stopped row without a config) the sync
    // is best treated as required when the config exists.
    let name = instance.name();
    let config_exists = match instance.engine() {
        crate::model::EngineKey::Postgresql => {
            inner.paths.data_dir(name).join("postgresql.conf").exists()
        }
        crate::model::EngineKey::Valkey => inner.paths.run_dir(name).join("valkey.conf").exists(),
        crate::model::EngineKey::Mysql | crate::model::EngineKey::Mariadb => false,
    };
    if config_exists {
        let updated = instance.with_port(port);
        super::helpers::sync_instance_port(&inner.paths, &updated)?;
    }

    inner.db(|store| store.update_port(id, port))?;
    emit_changed(inner, id);
    Ok(())
}

pub(crate) fn data_size(inner: &Inner, id: &InstanceId) -> Result<u64> {
    let instance = load(inner, id)?;
    Ok(dir_size(&inner.paths.data_dir(instance.name())))
}

pub(crate) fn set_autostart(inner: &Inner, id: &InstanceId, enabled: bool) -> Result<()> {
    load(inner, id)?;
    inner.db(|store| store.set_autostart(id, enabled))?;
    emit_changed(inner, id);
    Ok(())
}

pub(crate) fn cancel_operation(inner: &Inner, id: &OperationId) -> Result<()> {
    if inner.ops.cancel(id) {
        Ok(())
    } else {
        Err(DblError::NotFound {
            kind: "operation",
            name: id.as_str().to_owned(),
        })
    }
}

pub(crate) fn connection_uri(
    inner: &Inner,
    id: &InstanceId,
    format: UriFormat,
) -> Result<SecretString> {
    let instance = load(inner, id)?;
    let reference =
        inner
            .db(|store| store.get_credential(id))?
            .ok_or_else(|| DblError::NotFound {
                kind: "credential",
                name: id.as_str().to_owned(),
            })?;
    let password = inner.secrets.get(&reference)?;
    let credential = Credential::new(instance.superuser(), password);
    let ctx = run_context(&inner.paths, &instance, Some(credential.clone()));
    Ok(inner
        .adapter(instance.engine())
        .connection_uri(&ctx, &credential, format))
}

pub(crate) fn tail_logs(inner: &Inner, id: &InstanceId, lines: usize) -> Result<Vec<String>> {
    let instance = load(inner, id)?;
    let path = log_file_for(&inner.paths, &instance);
    crate::process::logs::tail_lines(&path, lines)
}

pub(crate) fn watch_logs(inner: &Inner, id: &InstanceId, enabled: bool) -> Result<()> {
    let instance = load(inner, id)?;
    if enabled {
        inner.log_watchers.enable(
            id.clone(),
            log_file_for(&inner.paths, &instance),
            inner.events.clone(),
            &inner.handle(),
        );
    } else {
        inner.log_watchers.disable(id);
    }
    Ok(())
}

/// Loads an instance or reports a typed `NotFound`.
pub(crate) fn load(inner: &Inner, id: &InstanceId) -> Result<Instance> {
    inner
        .db(|store| store.get_instance(id))?
        .ok_or_else(|| DblError::NotFound {
            kind: "instance",
            name: id.as_str().to_owned(),
        })
}

#[allow(dead_code)]
fn _unused(_: SystemTime) {}
