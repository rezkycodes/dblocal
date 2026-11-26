//! Config editor in the mock service (`F-CFG-01`, `F-CFG-02`).
//!
//! The mock keeps a config text per instance in memory and reuses the production
//! [`crate::config`] editor, schema and backup logic on it: the GUI tab and the
//! CLI are exercised against the same line-editor rules the real service uses,
//! so a bug in the editor shows up in a mock test too. There is no real file.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::config::{
    self, BackupInfo, ConfigApplied, ConfigError, ConfigSetting, ConfigView, Edit, KEEP,
    apply_edit, config_file_name, dialect_for, insertion_for, is_managed_key, settings_for,
    validate_value,
};
use crate::model::InstanceId;
use crate::service::{ServiceEvent, ServiceResult, UserFacingError};

use super::Inner;

/// The in-memory config text and its backups, per instance.
#[derive(Debug, Default, Clone)]
pub(crate) struct ConfigState {
    text: Option<String>,
    backups: Vec<BackupInfo>,
    last_backup_secs: u64,
}

fn io() -> UserFacingError {
    UserFacingError::from(ConfigError::Io)
}

fn loaded(
    inner: &Inner,
    id: &InstanceId,
) -> ServiceResult<(ConfigState, crate::model::EngineKey, PathBuf)> {
    let instance = inner.instances.get(id).cloned().ok_or_else(|| {
        UserFacingError::from(crate::DblError::NotFound {
            kind: "instance",
            name: id.as_str().to_owned(),
        })
    })?;
    let state = inner.configs.get(id).cloned().unwrap_or_default();
    let path = PathBuf::from(format!(
        "/mock/{}/{}",
        instance.name(),
        config_file_name(instance.engine())
    ));
    Ok((state, instance.engine(), path))
}

pub(crate) fn view(inner: &Arc<Mutex<Inner>>, id: &InstanceId) -> ServiceResult<ConfigView> {
    let guard = inner.lock().map_err(|_| io())?;
    let (state, engine, path) = loaded(&guard, id)?;
    let dialect = dialect_for(engine);
    let text = state.text.as_deref();
    let mut settings = Vec::new();
    for spec in settings_for(engine) {
        let current = text.and_then(|text| config::editor::read_value(dialect, text, spec.key));
        settings.push(ConfigSetting {
            key: spec.key.to_owned(),
            current,
            default: spec.default.to_owned(),
            unit: spec.unit.to_owned(),
            restart_required: true,
            description: spec.description.to_owned(),
            readonly: false,
        });
    }
    settings.push(ConfigSetting {
        key: "port".to_owned(),
        current: Some(engine.canonical_port().to_string()),
        default: engine.canonical_port().to_string(),
        unit: String::new(),
        restart_required: false,
        description: config::messages::READONLY_LABEL.to_owned(),
        readonly: true,
    });
    Ok(ConfigView {
        id: id.clone(),
        engine,
        settings,
        backup_available: state.backups.last().cloned(),
        raw_path: path,
        restart_required: false,
    })
}

pub(crate) fn apply(
    inner: &Arc<Mutex<Inner>>,
    id: &InstanceId,
    changes: &[(String, String)],
) -> ServiceResult<ConfigApplied> {
    let mut guard = inner.lock().map_err(|_| io())?;
    let (mut state, engine, path) = loaded(&guard, id)?;
    let dialect = dialect_for(engine);
    let mut edits: Vec<Edit<'_>> = Vec::new();
    for (key, value) in changes {
        if is_managed_key(key) {
            return Err(UserFacingError::from(ConfigError::ManagedKey));
        }
        let Some(spec) = config::spec_for(engine, key) else {
            return Err(UserFacingError::from(ConfigError::UnknownKey));
        };
        validate_value(spec, value)?;
        edits.push(Edit {
            key: spec.key,
            value,
            insertion: insertion_for(engine),
        });
    }
    let mut text = state.text.clone().unwrap_or_default();
    for edit in &edits {
        text = apply_edit(dialect, &text, edit)
            .map_err(|_| UserFacingError::from(ConfigError::InvalidValue("edit rejected")))?;
    }
    state.text = Some(text);
    state.last_backup_secs += 1;
    let backup = BackupInfo {
        taken_at: state.last_backup_secs,
        taken_at_secs: state.last_backup_secs,
        path: path.with_extension(format!("{}.bak", state.last_backup_secs)),
    };
    state.backups.push(backup.clone());
    let excess = state.backups.len().saturating_sub(KEEP);
    state.backups.drain(0..excess);
    guard.configs.insert(id.clone(), state);
    guard.emit(ServiceEvent::ConfigChanged { id: id.clone() });
    Ok(ConfigApplied {
        restart_required: true,
        backup,
        changed: edits.len(),
    })
}

pub(crate) fn restore(inner: &Arc<Mutex<Inner>>, id: &InstanceId) -> ServiceResult<()> {
    let guard = inner.lock().map_err(|_| io())?;
    let (state, _engine, _path) = loaded(&guard, id)?;
    if state.backups.is_empty() {
        return Err(UserFacingError::from(ConfigError::NoBackup));
    }
    guard.emit(ServiceEvent::ConfigChanged { id: id.clone() });
    Ok(())
}

pub(crate) fn raw_path(inner: &Arc<Mutex<Inner>>, id: &InstanceId) -> ServiceResult<PathBuf> {
    let guard = inner.lock().map_err(|_| io())?;
    let (_state, _engine, path) = loaded(&guard, id)?;
    Ok(path)
}

/// The mock has no real file and never opens a window: it returns the command
/// the real service would run, so the CLI/GUI can render "would open <path>".
pub(crate) fn open_raw(
    inner: &Arc<Mutex<Inner>>,
    id: &InstanceId,
) -> ServiceResult<crate::config::OpenCommand> {
    let guard = inner.lock().map_err(|_| io())?;
    let (_state, _engine, path) = loaded(&guard, id)?;
    let os = crate::platform::Os::current();
    Ok(crate::config::OpenCommand::for_os(os, &path))
}
