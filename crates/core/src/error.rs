//! Stable error type for the whole crate.
//!
//! Every error carries a stable `DBL-NNNN` code that the UI and the docs can
//! reference, plus a friendly message that never contains a secret.

/// Code for [`DblError::InvalidInput`].
pub const CODE_INVALID_INPUT: &str = "DBL-1001";
/// Code for [`DblError::NotFound`].
pub const CODE_NOT_FOUND: &str = "DBL-1002";
/// Code for [`DblError::AlreadyExists`].
pub const CODE_ALREADY_EXISTS: &str = "DBL-1003";
/// Code for [`DblError::InvalidTransition`].
pub const CODE_INVALID_TRANSITION: &str = "DBL-1004";
/// Code for [`DblError::PortInUse`].
pub const CODE_PORT_IN_USE: &str = "DBL-1005";
/// Code for [`DblError::ChecksumMismatch`].
pub const CODE_CHECKSUM_MISMATCH: &str = "DBL-2001";
/// Code for [`DblError::UnsafeArchive`].
pub const CODE_UNSAFE_ARCHIVE: &str = "DBL-2002";
/// Code for [`DblError::Io`].
pub const CODE_IO: &str = "DBL-3001";
/// Code for [`DblError::Network`].
pub const CODE_NETWORK: &str = "DBL-3002";
/// Code for [`DblError::Storage`].
pub const CODE_STORAGE: &str = "DBL-3003";
/// Code for [`DblError::Engine`].
pub const CODE_ENGINE: &str = "DBL-4001";
/// Code for [`DblError::Secret`].
pub const CODE_SECRET: &str = "DBL-5001";
/// Code for [`DblError::Unsupported`].
pub const CODE_UNSUPPORTED: &str = "DBL-6001";
/// Code for [`DblError::Cancelled`].
pub const CODE_CANCELLED: &str = "DBL-7001";
/// Code for [`DblError::NotImplemented`].
pub const CODE_NOT_IMPLEMENTED: &str = "DBL-9001";

/// Convenient result alias used across the crate.
pub type Result<T, E = DblError> = std::result::Result<T, E>;

/// Every failure mode the core can report.
///
/// Payloads carry diagnostic detail for logs only. [`DblError::user_message`]
/// is built from static strings, so it can never leak a password.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DblError {
    /// A value failed validation at a system boundary.
    #[error("invalid input for {field}: {reason}")]
    InvalidInput {
        /// Name of the offending field.
        field: &'static str,
        /// Why the value was rejected. Must not contain secrets.
        reason: String,
    },

    /// The requested entity does not exist.
    #[error("{kind} not found: {name}")]
    NotFound {
        /// Entity kind, for example `instance`.
        kind: &'static str,
        /// Entity name or id.
        name: String,
    },

    /// The entity already exists.
    #[error("{kind} already exists: {name}")]
    AlreadyExists {
        /// Entity kind, for example `instance`.
        kind: &'static str,
        /// Entity name or id.
        name: String,
    },

    /// An operation is not allowed in the current instance status.
    #[error("invalid transition: {from} -> {to}")]
    InvalidTransition {
        /// Current status.
        from: &'static str,
        /// Requested next status.
        to: &'static str,
    },

    /// A TCP port is already bound by another process.
    #[error("port {port} is already in use")]
    PortInUse {
        /// The contested port.
        port: u16,
    },

    /// A downloaded artifact failed SHA-256 verification.
    #[error("checksum mismatch for {artifact}")]
    ChecksumMismatch {
        /// Artifact description, never a secret.
        artifact: String,
    },

    /// An archive entry would escape the extraction directory.
    #[error("unsafe archive entry: {entry}")]
    UnsafeArchive {
        /// The rejected entry path.
        entry: String,
    },

    /// Filesystem failure.
    #[error("io error: {source}")]
    Io {
        /// Underlying error.
        #[from]
        source: std::io::Error,
    },

    /// HTTP or TLS failure.
    #[error("network error: {reason}")]
    Network {
        /// Human readable cause.
        reason: String,
    },

    /// SQLite or state file failure.
    #[error("storage error: {reason}")]
    Storage {
        /// Human readable cause.
        reason: String,
    },

    /// The engine binary failed, or reported an error.
    #[error("engine error: {reason}")]
    Engine {
        /// Human readable cause.
        reason: String,
    },

    /// Credential storage failed.
    #[error("secret storage error: {reason}")]
    Secret {
        /// Human readable cause. Never the secret itself.
        reason: String,
    },

    /// The platform or build does not support the operation.
    #[error("unsupported: {reason}")]
    Unsupported {
        /// Human readable cause.
        reason: String,
    },

    /// The operation was cancelled by the user.
    #[error("cancelled: {operation}")]
    Cancelled {
        /// Operation that was cancelled.
        operation: String,
    },

    /// The feature is planned but not built yet.
    #[error("not implemented: {0}")]
    NotImplemented(String),
}

impl DblError {
    /// Stable, searchable error code shaped `DBL-NNNN`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput { .. } => CODE_INVALID_INPUT,
            Self::NotFound { .. } => CODE_NOT_FOUND,
            Self::AlreadyExists { .. } => CODE_ALREADY_EXISTS,
            Self::InvalidTransition { .. } => CODE_INVALID_TRANSITION,
            Self::PortInUse { .. } => CODE_PORT_IN_USE,
            Self::ChecksumMismatch { .. } => CODE_CHECKSUM_MISMATCH,
            Self::UnsafeArchive { .. } => CODE_UNSAFE_ARCHIVE,
            Self::Io { .. } => CODE_IO,
            Self::Network { .. } => CODE_NETWORK,
            Self::Storage { .. } => CODE_STORAGE,
            Self::Engine { .. } => CODE_ENGINE,
            Self::Secret { .. } => CODE_SECRET,
            Self::Unsupported { .. } => CODE_UNSUPPORTED,
            Self::Cancelled { .. } => CODE_CANCELLED,
            Self::NotImplemented(_) => CODE_NOT_IMPLEMENTED,
        }
    }

    /// Friendly message for the UI, safe to display and to copy into an issue.
    ///
    /// The text is static per variant: it never embeds a password, a token or
    /// any other field the caller passed in.
    pub fn user_message(&self) -> String {
        let text = match self {
            Self::InvalidInput { .. } => crate::strings::ERR_INVALID_INPUT,
            Self::NotFound { .. } => crate::strings::ERR_NOT_FOUND,
            Self::AlreadyExists { .. } => crate::strings::ERR_ALREADY_EXISTS,
            Self::InvalidTransition { .. } => crate::strings::ERR_INVALID_TRANSITION,
            Self::PortInUse { .. } => crate::strings::ERR_PORT_IN_USE,
            Self::ChecksumMismatch { .. } => crate::strings::ERR_CHECKSUM_MISMATCH,
            Self::UnsafeArchive { .. } => crate::strings::ERR_UNSAFE_ARCHIVE,
            Self::Io { .. } => crate::strings::ERR_IO,
            Self::Network { .. } => crate::strings::ERR_NETWORK,
            Self::Storage { .. } => crate::strings::ERR_STORAGE,
            Self::Engine { .. } => crate::strings::ERR_ENGINE,
            Self::Secret { .. } => crate::strings::ERR_SECRET,
            Self::Unsupported { .. } => crate::strings::ERR_UNSUPPORTED,
            Self::Cancelled { .. } => crate::strings::ERR_CANCELLED,
            Self::NotImplemented(_) => crate::strings::ERR_NOT_IMPLEMENTED,
        };
        text.to_owned()
    }

    /// Renders the stable code together with the friendly message.
    ///
    /// This is what the CLI prints, so every failure the user sees can be
    /// searched as `DBL-NNNN`.
    pub fn diagnostic(&self) -> String {
        format!("[{}] {}", self.code(), self.user_message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings;

    fn all_variants() -> Vec<DblError> {
        vec![
            DblError::InvalidInput {
                field: "port",
                reason: "too low".into(),
            },
            DblError::NotFound {
                kind: "instance",
                name: "nope".into(),
            },
            DblError::AlreadyExists {
                kind: "instance",
                name: "dup".into(),
            },
            DblError::InvalidTransition {
                from: "stopped",
                to: "running",
            },
            DblError::PortInUse { port: 5432 },
            DblError::ChecksumMismatch {
                artifact: "postgres.tar.gz".into(),
            },
            DblError::UnsafeArchive {
                entry: "../etc/passwd".into(),
            },
            DblError::Io {
                source: std::io::Error::other("boom"),
            },
            DblError::Network {
                reason: "timeout".into(),
            },
            DblError::Storage {
                reason: "locked".into(),
            },
            DblError::Engine {
                reason: "initdb failed".into(),
            },
            DblError::Secret {
                reason: "keychain locked".into(),
            },
            DblError::Unsupported {
                reason: "windows".into(),
            },
            DblError::Cancelled {
                operation: "install".into(),
            },
            DblError::NotImplemented("catalog".into()),
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
            assert!(!err.user_message().is_empty(), "empty message for {code}");
            assert_eq!(err.user_message(), strings_lookup(code));
        }
    }

    fn strings_lookup(code: &str) -> String {
        match code {
            CODE_INVALID_INPUT => strings::ERR_INVALID_INPUT.into(),
            CODE_NOT_FOUND => strings::ERR_NOT_FOUND.into(),
            CODE_ALREADY_EXISTS => strings::ERR_ALREADY_EXISTS.into(),
            CODE_INVALID_TRANSITION => strings::ERR_INVALID_TRANSITION.into(),
            CODE_PORT_IN_USE => strings::ERR_PORT_IN_USE.into(),
            CODE_CHECKSUM_MISMATCH => strings::ERR_CHECKSUM_MISMATCH.into(),
            CODE_UNSAFE_ARCHIVE => strings::ERR_UNSAFE_ARCHIVE.into(),
            CODE_IO => strings::ERR_IO.into(),
            CODE_NETWORK => strings::ERR_NETWORK.into(),
            CODE_STORAGE => strings::ERR_STORAGE.into(),
            CODE_ENGINE => strings::ERR_ENGINE.into(),
            CODE_SECRET => strings::ERR_SECRET.into(),
            CODE_UNSUPPORTED => strings::ERR_UNSUPPORTED.into(),
            CODE_CANCELLED => strings::ERR_CANCELLED.into(),
            CODE_NOT_IMPLEMENTED => strings::ERR_NOT_IMPLEMENTED.into(),
            other => panic!("unnamed code {other}"),
        }
    }

    #[test]
    fn display_never_contains_payload_secrets() {
        let err = DblError::Secret {
            reason: "keychain locked".into(),
        };
        let text = err.diagnostic();
        assert!(text.contains(CODE_SECRET));
        assert!(text.contains(crate::strings::ERR_SECRET));
        assert!(!text.contains("hunter2"));
    }
}
