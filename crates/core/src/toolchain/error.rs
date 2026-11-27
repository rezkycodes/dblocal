//! Toolchain-specific diagnostics.
//!
//! The rest of the core reports failures through [`crate::DblError`], whose
//! variants already carry fixed `DBL-NNNN` codes. The toolchain operations add a
//! few failure modes of their own that do not fit those variants cleanly — "this
//! tool has no active version", "the shims directory is not first on PATH", and
//! so on. They are modelled here, with their own `DBL-9xxx` codes, and converted
//! into a [`crate::DblError`] at the service boundary so the CLI and GUI can show
//! one message per failure.

use crate::DblError;

/// Stable code for [`ToolchainError::NoActiveVersion`].
pub const CODE_NO_ACTIVE_VERSION: &str = "DBL-9101";
/// Stable code for [`ToolchainError::NotInstalled`].
pub const CODE_NOT_INSTALLED: &str = "DBL-9102";
/// Stable code for [`ToolchainError::UnavailableOnPlatform`].
pub const CODE_UNAVAILABLE_ON_PLATFORM: &str = "DBL-9103";
/// Stable code for [`ToolchainError::ShimNeedsPhp`].
pub const CODE_SHIM_NEEDS_PHP: &str = "DBL-9104";
/// Stable code for [`ToolchainError::InvalidSetting`].
pub const CODE_INVALID_SETTING: &str = "DBL-9105";
/// Stable code for [`ToolchainError::PathNotApplied`].
pub const CODE_PATH_NOT_APPLIED: &str = "DBL-9106";
/// Stable code for [`ToolchainError::RcFileUnwritable`].
pub const CODE_RC_FILE_UNWRITABLE: &str = "DBL-9107";
/// Stable code for [`ToolchainError::ProcessRunning`].
pub const CODE_PROCESS_RUNNING: &str = "DBL-9108";

/// A toolchain failure that the module raises and later maps to a diagnostic.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ToolchainError {
    /// A tool has no active version, so it has no shim.
    #[error("no active version of {tool}; run `dblocal toolchain use {tool} <version>` first")]
    NoActiveVersion {
        /// Tool whose shim was requested.
        tool: &'static str,
    },

    /// The requested tool/version is not installed.
    #[error("{tool} {version} is not installed")]
    NotInstalled {
        /// Tool.
        tool: &'static str,
        /// Version that is missing.
        version: String,
    },

    /// The requested tool/version is not in the catalog for this platform.
    #[error("{tool} {version} is not available on this platform: {reason}")]
    UnavailableOnPlatform {
        /// Tool.
        tool: &'static str,
        /// Version requested.
        version: String,
        /// Why it is unavailable.
        reason: String,
    },

    /// The Composer shim was requested but no PHP is active.
    #[error("Composer needs an active PHP; run `dblocal toolchain use php <version>` first")]
    ShimNeedsPhp,

    /// A PHP setting value failed validation.
    #[error("invalid value for {key}: {reason}")]
    InvalidSetting {
        /// Setting key.
        key: &'static str,
        /// Why the value was rejected.
        reason: String,
    },

    /// A PATH edit was requested but DBLocal's block is not present to remove.
    #[error("the DBLocal PATH block is not present")]
    PathNotApplied,

    /// An rc file could not be read or written.
    #[error("could not update {path}: {reason}")]
    RcFileUnwritable {
        /// Path that failed.
        path: String,
        /// Why.
        reason: String,
    },

    /// A process started from the install is still running, so it may not be
    /// removed.
    #[error("{tool} {version} is still running (pid(s): {pids}); stop it before removing")]
    ProcessRunning {
        /// Tool.
        tool: &'static str,
        /// Version.
        version: String,
        /// The pids seen (best effort).
        pids: String,
    },
}

impl ToolchainError {
    /// Stable code for this failure.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NoActiveVersion { .. } => CODE_NO_ACTIVE_VERSION,
            Self::NotInstalled { .. } => CODE_NOT_INSTALLED,
            Self::UnavailableOnPlatform { .. } => CODE_UNAVAILABLE_ON_PLATFORM,
            Self::ShimNeedsPhp => CODE_SHIM_NEEDS_PHP,
            Self::InvalidSetting { .. } => CODE_INVALID_SETTING,
            Self::PathNotApplied => CODE_PATH_NOT_APPLIED,
            Self::RcFileUnwritable { .. } => CODE_RC_FILE_UNWRITABLE,
            Self::ProcessRunning { .. } => CODE_PROCESS_RUNNING,
        }
    }

    /// Maps the failure onto the closest [`DblError`] for the service boundary.
    ///
    /// The message is preserved in the diagnostic so the CLI prints the specific
    /// reason, not a generic one. The mapping picks the [`DblError`] variant whose
    /// CLI exit code and user-facing text fit best.
    pub fn into_dbl_error(self) -> DblError {
        let reason = self.to_string();
        match self {
            Self::NoActiveVersion { tool } => DblError::NotFound {
                kind: "active toolchain",
                name: tool.to_owned(),
            },
            Self::NotInstalled { tool, version } => DblError::NotFound {
                kind: tool,
                name: version,
            },
            Self::UnavailableOnPlatform { .. } => DblError::Unsupported { reason },
            Self::ShimNeedsPhp => DblError::InvalidInput {
                field: "php",
                reason,
            },
            Self::InvalidSetting { key, reason } => DblError::InvalidInput {
                field: key,
                reason,
            },
            Self::PathNotApplied => DblError::NotFound {
                kind: "PATH block",
                name: "dblocal".to_owned(),
            },
            Self::RcFileUnwritable { path, reason } => DblError::Io {
                source: std::io::Error::other(format!("{path}: {reason}")),
            },
            Self::ProcessRunning { .. } => DblError::RuntimeInUse {
                users: vec![reason],
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn all_variants() -> Vec<ToolchainError> {
        vec![
            ToolchainError::NoActiveVersion { tool: "php" },
            ToolchainError::NotInstalled {
                tool: "php",
                version: "8.4.15".into(),
            },
            ToolchainError::UnavailableOnPlatform {
                tool: "php",
                version: "8.4.15".into(),
                reason: "windows-aarch64".into(),
            },
            ToolchainError::ShimNeedsPhp,
            ToolchainError::InvalidSetting {
                key: "memory_limit",
                reason: "not a size".into(),
            },
            ToolchainError::PathNotApplied,
            ToolchainError::RcFileUnwritable {
                path: "/home/u/.bashrc".into(),
                reason: "read-only".into(),
            },
            ToolchainError::ProcessRunning {
                tool: "php",
                version: "8.4.15".into(),
                pids: "1234".into(),
            },
        ]
    }

    #[test]
    fn codes_are_unique_and_well_formed() {
        let mut seen = std::collections::BTreeSet::new();
        for err in all_variants() {
            let code = err.code();
            assert!(
                code.len() == 8
                    && code.starts_with("DBL-")
                    && code[4..].chars().all(|c| c.is_ascii_digit()),
                "malformed code {code}"
            );
            assert!(seen.insert(code), "duplicate code {code}");
            assert!(!err.to_string().is_empty());
        }
    }

    #[test]
    fn every_variant_maps_to_a_dbl_error() {
        for err in all_variants() {
            let code = err.code();
            let mapped = err.into_dbl_error();
            assert!(!mapped.code().is_empty(), "{code} produced no code");
            assert!(!mapped.diagnostic().is_empty(), "{code} produced no message");
        }
    }

    #[test]
    fn composer_needs_php_is_bad_input() {
        let mapped = ToolchainError::ShimNeedsPhp.into_dbl_error();
        assert!(matches!(mapped, DblError::InvalidInput { field: "php", .. }));
    }
}
