//! Open-in-external-client through `DblService` (`F-OPN-01..02`).
//!
//! Detection is cached for the process and refreshable. Opening builds the plan
//! through [`plan_open`] and runs it through the injected [`Spawner`]; the
//! password only ever reaches a private launcher script, never the plan's
//! argv/URL/command string, and the service never returns it.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::clients::{
    ClientError, HostProbe, OpenTarget, OsKind, Spawner, TerminalChoice, detect_clients, plan_open,
    resolve_terminal, run,
};
use crate::model::{EngineKey, Instance};
use crate::service::{ClientKind, DetectedClient, LaunchPlan, ServiceResult};

use super::read_ops::load;
use super::service::Inner;

/// Returns the detected clients, re-running detection when `refresh`.
pub(crate) fn detect(inner: &Inner, refresh: bool) -> ServiceResult<Vec<DetectedClient>> {
    let mut cached = inner.client_cache.lock().map_err(|_| poisoned())?;
    if refresh || cached.is_none() {
        *cached = Some(detect_clients(inner.host_probe.as_ref()));
    }
    Ok(cached.clone().unwrap_or_default())
}

/// Plans and (unless `dry_run`) launches `client` against `instance`.
pub(crate) fn open(
    inner: &Inner,
    id: &crate::model::InstanceId,
    client: ClientKind,
    dry_run: bool,
) -> ServiceResult<LaunchPlan> {
    let instance = load(inner, id)?;
    let detected = detect(inner, false)?;
    let found = detected
        .into_iter()
        .find(|candidate| candidate.kind() == client)
        .ok_or_else(|| ClientError::ClientNotFound {
            name: client.as_str().to_owned(),
        })?;
    let target = open_target(&instance).map_err(facing)?;
    let terminal = terminal_for(inner, client).map_err(facing)?;
    let credential = launch_credential(inner, &instance);
    let plan = plan_open(
        &found,
        &target,
        credential.as_ref(),
        inner.os_kind(),
        &terminal,
    )
    .map_err(facing)?;
    if !dry_run {
        run(&plan, inner.spawner.as_ref()).map_err(facing)?;
    }
    Ok(plan)
}

/// Builds the validated target for `instance`.
fn open_target(instance: &Instance) -> Result<OpenTarget, ClientError> {
    OpenTarget::new(
        instance.engine(),
        "127.0.0.1",
        instance.port().get(),
        instance.superuser(),
        default_database(instance.engine()),
        instance.name().as_str(),
    )
}

/// The database a client should open: PostgreSQL's default database, empty for
/// the others (MySQL and Valkey have no fixed default name in a URI).
fn default_database(engine: EngineKey) -> &'static str {
    match engine {
        EngineKey::Postgresql => crate::consts::DEFAULT_DATABASE,
        _ => "",
    }
}

/// The terminal a CLI client launches in.
///
/// A GUI client never reads it, so a headless host still opens a GUI client; a
/// CLI client with no terminal fails with the module's typed `NoTerminal`.
fn terminal_for(inner: &Inner, client: ClientKind) -> Result<TerminalChoice, ClientError> {
    match resolve_terminal(inner.host_probe.as_ref(), inner.os_kind()) {
        Ok(terminal) => Ok(terminal),
        Err(err) if client.is_gui() => {
            // Unread by a GUI plan; the placeholder keeps the planner's shape.
            let _ = err;
            Ok(TerminalChoice::new(
                PathBuf::new(),
                crate::clients::TerminalKind::GenericArgv,
            ))
        }
        Err(err) => Err(err),
    }
}

/// Reads the instance password for a CLI launcher, when the engine has one.
///
/// The value never leaves the plan's launcher script; returning `None` for a
/// no-password instance is the same rule the connect URI follows.
fn launch_credential(inner: &Inner, instance: &Instance) -> Option<crate::secret::SecretString> {
    if instance.auth() != crate::model::AuthMode::Password {
        return None;
    }
    let reference = inner
        .db(|store| store.get_credential(instance.id()))
        .ok()??;
    inner.secrets.get(&reference).ok()
}

fn facing(err: ClientError) -> crate::service::UserFacingError {
    err.into()
}

fn poisoned() -> crate::service::UserFacingError {
    crate::service::UserFacingError::from(&crate::DblError::Storage {
        reason: "the client cache lock was poisoned".to_owned(),
    })
}

/// The cache slot type, named so `Inner` can hold it.
pub(crate) type ClientCache = Mutex<Option<Vec<DetectedClient>>>;

/// A production [`HostProbe`] over the real filesystem and `PATH`.
///
/// Kept beside the service so a front-end never has to supply its own host.
#[derive(Debug, Default)]
pub struct SystemHost;

impl HostProbe for SystemHost {
    fn which(&self, name: &str) -> Option<PathBuf> {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths).find_map(|dir| {
                let candidate = dir.join(name);
                candidate.is_file().then_some(candidate)
            })
        })
    }

    fn path_exists(&self, path: &std::path::Path) -> bool {
        path.exists()
    }

    fn desktop_entries(&self) -> Vec<String> {
        Vec::new()
    }

    fn macos_applications(&self) -> Vec<String> {
        Vec::new()
    }

    fn windows_app_paths(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Spawner seam used by the service.
pub(crate) type SpawnerRef = std::sync::Arc<dyn Spawner>;

/// Where the default spawner comes from.
pub(crate) fn default_spawner() -> SpawnerRef {
    std::sync::Arc::new(crate::clients::SystemSpawner::default())
}

/// Where the default host probe comes from.
pub(crate) fn default_host() -> std::sync::Arc<dyn HostProbe> {
    std::sync::Arc::new(SystemHost)
}

/// `OpenTarget`'s OS family for a plan.
pub(crate) fn os_kind_of(os: crate::platform::Os) -> OsKind {
    match os {
        crate::platform::Os::Linux => OsKind::Linux,
        crate::platform::Os::Macos => OsKind::Macos,
        crate::platform::Os::Windows => OsKind::Windows,
    }
}
