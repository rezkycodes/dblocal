//! The read model the GUI Config tab and the CLI render (`F-CFG-01` AC1).
//!
//! Every type here is an immutable value type with no secret: the `requirepass`
//! key is not editable and never appears, and no config file content beyond a
//! setting's current value travels in a view.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::model::InstanceId;

use super::backup::BackupInfo;

/// One row of the Config form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSetting {
    /// Directive key as the engine spells it.
    pub key: String,
    /// Value the file currently assigns, or `None` when the file does not set it
    /// (the default is then in effect).
    pub current: Option<String>,
    /// Value the engine uses when the key is unset.
    pub default: String,
    /// Human unit (`""` for unitless values).
    pub unit: String,
    /// Whether a restart is needed (always true for this subset).
    pub restart_required: bool,
    /// One-line description.
    pub description: String,
    /// Whether the setting is read-only in this task (port, auth method).
    pub readonly: bool,
}

/// The whole Config read model for one instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigView {
    /// Instance the settings belong to.
    pub id: InstanceId,
    /// Engine the settings apply to.
    pub engine: crate::model::EngineKey,
    /// Editable and read-only rows, in display order.
    pub settings: Vec<ConfigSetting>,
    /// The newest backup, if a change has ever been made.
    pub backup_available: Option<BackupInfo>,
    /// The raw file the "open in editor" action opens.
    pub raw_path: PathBuf,
    /// Whether a restart is needed for an applied change to take effect.
    pub restart_required: bool,
}

/// What `config_apply` reports back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigApplied {
    /// Whether a restart is needed to apply the change (always true here).
    pub restart_required: bool,
    /// The backup taken before the write.
    pub backup: BackupInfo,
    /// The number of keys that changed.
    pub changed: usize,
}

impl ConfigView {
    /// Builds an empty view for an instance.
    pub fn new(id: InstanceId, engine: crate::model::EngineKey, raw_path: PathBuf) -> Self {
        Self {
            id,
            engine,
            settings: Vec::new(),
            backup_available: None,
            raw_path,
            restart_required: false,
        }
    }

    /// The ids of editable rows.
    pub fn editable_keys(&self) -> impl Iterator<Item = &str> {
        self.settings
            .iter()
            .filter(|s| !s.readonly)
            .map(|s| s.key.as_str())
    }
}

/// Convenience for a backup's timestamp as a `SystemTime`.
pub fn taken_at_time(backup: &BackupInfo) -> SystemTime {
    SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(backup.taken_at_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EngineKey;

    fn view() -> ConfigView {
        let mut view = ConfigView::new(
            InstanceId::new(),
            EngineKey::Postgresql,
            PathBuf::from("/data/postgresql.conf"),
        );
        view.settings.push(ConfigSetting {
            key: "max_connections".into(),
            current: Some("100".into()),
            default: "100".into(),
            unit: String::new(),
            restart_required: true,
            description: "connections".into(),
            readonly: false,
        });
        view.settings.push(ConfigSetting {
            key: "port".into(),
            current: Some("5432".into()),
            default: "5432".into(),
            unit: String::new(),
            restart_required: false,
            description: "port".into(),
            readonly: true,
        });
        view
    }

    #[test]
    fn editable_keys_excludes_read_only_rows() {
        let view = view();
        let keys: Vec<&str> = view.editable_keys().collect();
        assert_eq!(keys, vec!["max_connections"]);
    }

    #[test]
    fn taken_at_time_reads_the_backup_timestamp() {
        let backup = BackupInfo {
            taken_at: 1_700_000_000,
            taken_at_secs: 1_700_000_000,
            path: PathBuf::from("/x.bak"),
        };
        assert_eq!(
            taken_at_time(&backup),
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)
        );
    }
}
