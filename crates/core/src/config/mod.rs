//! Config editor core (PRD `F-CFG-01`, `F-CFG-02`).
//!
//! Four layers, each testable without a server:
//!
//! - [`editor`] is the pure line-level writer. It changes only the one line of
//!   the key it is asked to change, appends a missing key inside the right
//!   section, and never rewrites a comment, an unknown key, a blank line, a line
//!   ending or a trailing-newline-less last line.
//! - [`schema`] is the editable subset per engine, its value kinds, ranges and
//!   the real engine defaults it was checked against, plus the functions that
//!   validate a value before anything is written.
//! - [`backup`] copies a config to a private, timestamped backup and records the
//!   hash of the file at the last successful start, so a failed start can offer
//!   "restore the previous configuration".
//! - [`view`] is the read model the GUI tab and the CLI render.
//!
//! The service glue lives in `instance::config_ops`; this module knows nothing
//! about `DblService`, the store or a running engine.

pub mod backup;
pub mod editor;
pub mod error;
pub mod messages;
pub mod schema;
pub mod view;

pub use backup::{BackupInfo, BackupStore};
pub use editor::{Dialect, Edit, EditError, Insertion, apply_edit};
pub use error::{CODE_RANGE, ConfigError};
pub use schema::{
    ApplyTime, DEFAULTS_PROVENANCE, SettingSpec, ValueKind, format_size, is_managed_key,
    parse_size, readonly_keys, settings_for, spec_for, validate_value,
};
pub use view::{ConfigApplied, ConfigSetting, ConfigView};

/// The file name of an engine's config, as it appears on disk.
pub fn config_file_name(engine: crate::model::EngineKey) -> &'static str {
    match engine {
        crate::model::EngineKey::Postgresql => "postgresql.conf",
        crate::model::EngineKey::Mysql | crate::model::EngineKey::Mariadb => "my.cnf",
        crate::model::EngineKey::Valkey => "valkey.conf",
    }
}

/// The syntax family an engine's config uses.
pub fn dialect_for(engine: crate::model::EngineKey) -> Dialect {
    match engine {
        crate::model::EngineKey::Postgresql => Dialect::Postgresql,
        crate::model::EngineKey::Mysql | crate::model::EngineKey::Mariadb => Dialect::Ini,
        crate::model::EngineKey::Valkey => Dialect::Valkey,
    }
}

/// Where a missing key is appended for an engine.
pub fn insertion_for(engine: crate::model::EngineKey) -> Insertion {
    match engine {
        crate::model::EngineKey::Mysql | crate::model::EngineKey::Mariadb => {
            Insertion::Section("mysqld")
        }
        _ => Insertion::End,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EngineKey;

    #[test]
    fn file_names_and_dialects_per_engine() {
        assert_eq!(config_file_name(EngineKey::Postgresql), "postgresql.conf");
        assert_eq!(config_file_name(EngineKey::Mysql), "my.cnf");
        assert_eq!(config_file_name(EngineKey::Mariadb), "my.cnf");
        assert_eq!(config_file_name(EngineKey::Valkey), "valkey.conf");
        assert_eq!(dialect_for(EngineKey::Postgresql), Dialect::Postgresql);
        assert_eq!(dialect_for(EngineKey::Mysql), Dialect::Ini);
        assert_eq!(dialect_for(EngineKey::Valkey), Dialect::Valkey);
        assert_eq!(
            insertion_for(EngineKey::Mysql),
            Insertion::Section("mysqld")
        );
        assert_eq!(insertion_for(EngineKey::Postgresql), Insertion::End);
    }
}
