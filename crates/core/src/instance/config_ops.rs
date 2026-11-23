//! Config editor through `DblService` (`F-CFG-01`, `F-CFG-02`).
//!
//! The read model comes from the instance's config file and the pure
//! [`crate::config`] module; the write path takes the per-instance guard
//! ([`Ops::lock_instance`]), validates every requested key/value, takes one
//! backup of the file as it is now, and then performs one atomic write in which
//! every edit is applied to the same in-memory text. A refused key or a value
//! that fails validation aborts the whole call **before** anything is written.
//!
//! A running instance is allowed: the change is for the next start, so every
//! call reports `restart_required = true` and the caller is expected to offer a
//! restart (`F-CFG-01` AC2).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{
    BackupStore, ConfigApplied, ConfigError, ConfigSetting, ConfigView, Edit, apply_edit,
    config_file_name, dialect_for, insertion_for, is_managed_key, read_value_of, settings_for,
    validate_value,
};
use crate::model::{EngineKey, Instance, InstanceId};
use crate::service::{ServiceEvent, ServiceResult, UserFacingError};

use super::helpers::load;
use super::registry::InstanceLock;
use super::service::Inner;

/// The config file path for an instance.
pub fn config_path(inner: &Inner, instance: &Instance) -> PathBuf {
    match instance.engine() {
        EngineKey::Postgresql | EngineKey::Mariadb => inner
            .paths
            .data_dir(instance.name())
            .join(config_file_name(instance.engine())),
        EngineKey::Mysql | EngineKey::Valkey => inner
            .paths
            .run_dir(instance.name())
            .join(config_file_name(instance.engine())),
    }
}

/// Reads the current config text, or `None` when the file does not exist yet.
fn read_config(path: &Path) -> ServiceResult<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ConfigError::Io.into()),
    }
}

/// Builds the read model for `id` (`F-CFG-01` AC1).
pub(crate) fn view(inner: &Inner, id: &InstanceId) -> ServiceResult<ConfigView> {
    let instance = load(inner, id)?;
    let path = config_path(inner, &instance);
    let text = read_config(&path)?;
    let engine = instance.engine();
    let dialect = dialect_for(engine);

    let mut settings = Vec::new();
    for spec in settings_for(engine) {
        let current = text
            .as_deref()
            .and_then(|text| read_value_of(dialect, text, spec.key));
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
    // The read-only port row (and, for PostgreSQL, the authentication method).
    settings.push(ConfigSetting {
        key: "port".to_owned(),
        current: Some(instance.port().get().to_string()),
        default: engine.canonical_port().to_string(),
        unit: String::new(),
        restart_required: false,
        description: crate::config::messages::READONLY_LABEL.to_owned(),
        readonly: true,
    });
    if engine == EngineKey::Postgresql {
        let method = authentication_method(inner, &instance);
        settings.push(ConfigSetting {
            key: "authentication method".to_owned(),
            current: method,
            default: "scram-sha-256".to_owned(),
            unit: String::new(),
            restart_required: false,
            description: crate::config::messages::READONLY_LABEL.to_owned(),
            readonly: true,
        });
    }

    let store = BackupStore::for_instance(inner.paths.root(), instance.name().as_str());
    let backup_available = store.newest(config_file_name(engine));

    Ok(ConfigView {
        id: id.clone(),
        engine,
        settings,
        backup_available,
        raw_path: path,
        restart_required: false,
    })
}

/// Reads the authentication methods the DBLocal-managed `pg_hba.conf` host lines
/// use (read-only in this task).
fn authentication_method(inner: &Inner, instance: &Instance) -> Option<String> {
    let hba = inner.paths.data_dir(instance.name()).join("pg_hba.conf");
    let text = fs::read_to_string(hba).ok()?;
    let mut methods: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // `host all all 127.0.0.1/32 scram-sha-256`
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if let Some(method) = fields.last()
            && (fields.first() == Some(&"host"))
            && !methods.iter().any(|m| m == method)
        {
            methods.push((*method).to_owned());
        }
    }
    if methods.is_empty() {
        None
    } else {
        Some(methods.join(", "))
    }
}

/// Applies a batch of edits atomically (`F-CFG-01` AC2/AC3).
///
/// Every key must be editable and every value must validate before the file is
/// touched; one unknown/unmanaged key or one bad value refuses the whole call,
/// leaving the file and the backups unchanged.
pub(crate) fn apply(
    inner: &Inner,
    id: &InstanceId,
    changes: &[(String, String)],
) -> ServiceResult<ConfigApplied> {
    let instance = load(inner, id)?;
    let engine = instance.engine();
    let dialect = dialect_for(engine);

    // Validate everything first, so a single bad entry cannot half-write.
    let mut edits: Vec<Edit<'_>> = Vec::with_capacity(changes.len());
    for (key, value) in changes {
        if is_managed_key(key) {
            return Err(ConfigError::ManagedKey.into());
        }
        let Some(spec) = crate::config::spec_for(engine, key) else {
            return Err(ConfigError::UnknownKey.into());
        };
        validate_value(spec, value)?;
        edits.push(Edit {
            key: spec.key,
            value,
            insertion: insertion_for(engine),
        });
    }

    let _lock = lock_for(inner, id)?;
    let path = config_path(inner, &instance);
    let original = read_config(&path)?.ok_or(ConfigError::Io)?;

    // One backup of the file as it is now, then one atomic write.
    let store = BackupStore::for_instance(inner.paths.root(), instance.name().as_str());
    let backup = store
        .take(&path, now_secs())
        .map_err(|_| UserFacingError::from(ConfigError::Io))?;

    let mut text = original;
    for edit in &edits {
        text = apply_edit(dialect, &text, edit)
            .map_err(|_| UserFacingError::from(ConfigError::InvalidValue("edit ditolak")))?;
    }
    crate::fsutil::write_private_file_atomic(&path, text.as_bytes())
        .map_err(|_| UserFacingError::from(ConfigError::Io))?;

    inner.emit(ServiceEvent::ConfigChanged { id: id.clone() });
    Ok(ConfigApplied {
        restart_required: true,
        backup,
        changed: edits.len(),
    })
}

/// Restores the newest backup, keeping the file it replaces as a backup too.
pub(crate) fn restore(inner: &Inner, id: &InstanceId) -> ServiceResult<()> {
    let instance = load(inner, id)?;
    let _lock = lock_for(inner, id)?;
    let path = config_path(inner, &instance);
    let store = BackupStore::for_instance(inner.paths.root(), instance.name().as_str());
    let file = config_file_name(instance.engine());
    let newest = store
        .newest(file)
        .ok_or_else(|| UserFacingError::from(ConfigError::NoBackup))?;

    // Back up the file we are about to replace, so a restore is itself undoable.
    if path.exists() {
        let _ = store.take(&path, now_secs().saturating_add(1));
    }
    let bytes = fs::read(&newest.path).map_err(|_| UserFacingError::from(ConfigError::Io))?;
    crate::fsutil::write_private_file_atomic(&path, &bytes)
        .map_err(|_| UserFacingError::from(ConfigError::Io))?;
    inner.emit(ServiceEvent::ConfigChanged { id: id.clone() });
    Ok(())
}

/// Returns the config file path (`F-CFG-02` AC1).
pub(crate) fn raw_path(inner: &Inner, id: &InstanceId) -> ServiceResult<PathBuf> {
    let instance = load(inner, id)?;
    Ok(config_path(inner, &instance))
}

/// Opens the raw config file in the OS default handler (`F-CFG-02` AC1).
///
/// Resolves the path, refuses when the file does not exist yet (the instance was
/// never initialised), then launches the OS handler detached, argv only, no
/// shell. The launch goes through [`Inner::open_launcher`], a seam a test injects
/// a fake into. Nothing here reads or rewrites the file, so no content can leak.
pub(crate) fn open_raw(
    inner: &Inner,
    id: &InstanceId,
) -> ServiceResult<crate::config::OpenCommand> {
    let instance = load(inner, id)?;
    let path = config_path(inner, &instance);
    if !path.is_file() {
        return Err(ConfigError::MissingFile.into());
    }
    let command = crate::config::OpenCommand::for_os(inner.os, &path);
    inner
        .open_launcher
        .launch(&command)
        .map_err(|_| UserFacingError::from(ConfigError::LauncherFailed))?;
    Ok(command)
}

/// Takes the per-instance guard so a start/stop/delete in flight refuses this.
fn lock_for(inner: &Inner, id: &InstanceId) -> ServiceResult<InstanceLock> {
    inner.ops.lock_instance(id).map_err(|_| {
        // `lock_instance` reports busy as an invalid transition; map it to the
        // module's stable busy code so the GUI can say "try again".
        UserFacingError::from(ConfigError::Busy)
    })
}

/// Records the config file hash at a successful start, so a later failure can
/// offer a restore. Called by the start path when the instance reaches Running.
pub(crate) fn record_last_good(inner: &Inner, instance: &Instance) {
    let path = config_path(inner, instance);
    let Some(hash) = crate::config::backup::file_sha256(&path) else {
        return;
    };
    let store = BackupStore::for_instance(inner.paths.root(), instance.name().as_str());
    if store
        .record_last_good(config_file_name(instance.engine()), &hash)
        .is_err()
    {
        tracing::warn!(
            instance = %instance.name(),
            "could not record the last-good config hash"
        );
    }
}

/// Whether a start failure should offer "restore the previous configuration".
///
/// True when a backup exists and the file changed since the last successful
/// start (the recorded hash differs from the current one).
pub(crate) fn restore_available(inner: &Inner, instance: &Instance) -> bool {
    let path = config_path(inner, instance);
    let Some(current) = crate::config::backup::file_sha256(&path) else {
        return false;
    };
    let store = BackupStore::for_instance(inner.paths.root(), instance.name().as_str());
    let file = config_file_name(instance.engine());
    if store.newest(file).is_none() {
        return false;
    }
    match store.last_good(file) {
        Some(recorded) => recorded != current,
        // No marker yet: a backup exists, so offering a restore is honest.
        None => true,
    }
}

/// Current Unix time in seconds.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
