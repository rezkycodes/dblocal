//! Typed config-editor failures with stable `DBL-86xx` codes.
//!
//! The range is owned by this module (`docs/architecture/error-codes.md`). No
//! variant carries a free-form string, a file path that may contain a secret or
//! a value the user typed that could be a password: a [`ConfigError`] is safe to
//! render in a log, an event and the CLI by construction. The one exception is
//! [`ConfigError::InvalidValue`], which carries a static reason — never the
//! rejected value.

use crate::service::UserFacingError;

use super::messages;

/// Range this module owns, inclusive.
pub const CODE_RANGE: std::ops::RangeInclusive<u16> = 8600..=8699;

/// `DBL-8600`: the setting key is not editable for this engine.
pub const CODE_UNKNOWN_KEY: &str = "DBL-8600";
/// `DBL-8601`: the value failed validation.
pub const CODE_INVALID_VALUE: &str = "DBL-8601";
/// `DBL-8602`: the key is DBLocal-managed and must use its own action.
pub const CODE_MANAGED_KEY: &str = "DBL-8602";
/// `DBL-8603`: the config file could not be read or written.
pub const CODE_IO: &str = "DBL-8603";
/// `DBL-8604`: the instance is busy with a lifecycle operation.
pub const CODE_BUSY: &str = "DBL-8604";
/// `DBL-8605`: there is no backup to restore.
pub const CODE_NO_BACKUP: &str = "DBL-8605";
/// `DBL-8606`: the apply was refused because it touched more than the subset.
pub const CODE_REFUSED: &str = "DBL-8606";

/// Why a config operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The key is not in the editable subset for this engine.
    UnknownKey,
    /// The key is managed by DBLocal and cannot be edited here.
    ManagedKey,
    /// The value failed validation; the reason is static.
    InvalidValue(&'static str),
    /// The config file could not be read or written.
    Io,
    /// A lifecycle operation owns the instance; the caller may retry later.
    Busy,
    /// There is no backup for this instance yet.
    NoBackup,
    /// The call was refused as a whole because one entry was not permitted.
    Refused,
}

impl ConfigError {
    /// Stable code in this module's range.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnknownKey => CODE_UNKNOWN_KEY,
            Self::ManagedKey => CODE_MANAGED_KEY,
            Self::InvalidValue(_) => CODE_INVALID_VALUE,
            Self::Io => CODE_IO,
            Self::Busy => CODE_BUSY,
            Self::NoBackup => CODE_NO_BACKUP,
            Self::Refused => CODE_REFUSED,
        }
    }

    /// Indonesian message, never containing the rejected value.
    pub const fn message(&self) -> &'static str {
        match self {
            Self::UnknownKey => messages::ERR_UNKNOWN_KEY,
            Self::ManagedKey => messages::ERR_MANAGED_KEY,
            Self::InvalidValue(reason) => reason_static(reason),
            Self::Io => messages::ERR_IO,
            Self::Busy => messages::ERR_BUSY,
            Self::NoBackup => messages::ERR_NO_BACKUP,
            Self::Refused => messages::ERR_REFUSED,
        }
    }
}

/// Maps a static validation reason to its user message.
///
/// The reasons in `schema.rs` are all Indonesian already; this returns the
/// static string so `message()` can be `const` and cannot echo a value.
const fn reason_static(reason: &'static str) -> &'static str {
    reason
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code(), self.message())
    }
}

impl From<ConfigError> for UserFacingError {
    fn from(err: ConfigError) -> Self {
        UserFacingError::new(err.code(), err.message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<ConfigError> {
        vec![
            ConfigError::UnknownKey,
            ConfigError::ManagedKey,
            ConfigError::InvalidValue("nilai salah"),
            ConfigError::Io,
            ConfigError::Busy,
            ConfigError::NoBackup,
            ConfigError::Refused,
        ]
    }

    #[test]
    fn every_code_is_in_range_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for err in all() {
            let code = err.code();
            assert!(code.len() == 8 && code.starts_with("DBL-"), "{code}");
            let number: u16 = code[4..].parse().expect("numeric");
            assert!(CODE_RANGE.contains(&number), "{code} out of range");
            assert!(seen.insert(code), "duplicate {code}");
            assert!(!err.message().is_empty());
        }
    }

    #[test]
    fn no_variant_echoes_a_secret() {
        let canary = "hunter2-SECRET";
        for err in all() {
            let text = format!("{err:?}");
            assert!(!text.contains(canary));
            assert!(!err.message().contains(canary));
            assert!(!err.to_string().contains(canary));
        }
    }

    #[test]
    fn conversion_carries_the_code_and_message() {
        let facing = UserFacingError::from(ConfigError::ManagedKey);
        assert_eq!(facing.code, CODE_MANAGED_KEY);
        assert_eq!(facing.message, messages::ERR_MANAGED_KEY);
    }
}
