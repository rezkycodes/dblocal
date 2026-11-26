//! Module-local errors for open-in-client (DBL-8300-8399).
//!
//! This module must not leak secrets in `Debug` or `Display`. User messages
//! are user-facing and live in [`super::messages`].

use std::fmt;

use crate::service::UserFacingError;

use super::messages;

pub const CODE_UNSUPPORTED_CLIENT: &str = "DBL-8300";
pub const CODE_CLIENT_NOT_FOUND: &str = "DBL-8301";
pub const CODE_LAUNCH_FAILED: &str = "DBL-8302";
pub const CODE_INVALID_TARGET: &str = "DBL-8303";
pub const CODE_NO_TERMINAL: &str = "DBL-8304";
pub const CODE_SCRIPT_ERROR: &str = "DBL-8305";

/// Errors produced by client detection/planning/launch.
#[derive(Clone, PartialEq, Eq)]
pub enum ClientError {
    /// The selected client cannot open the target engine.
    UnsupportedClient {
        client: &'static str,
        engine: &'static str,
    },
    /// A detected client disappeared before launch.
    ClientNotFound { name: String },
    /// Spawning the client failed.
    LaunchFailed { reason: String },
    /// A connection target field was invalid or unsafe.
    InvalidTarget { field: &'static str },
    /// No terminal emulator could be found.
    NoTerminal,
    /// Launcher script directory/file could not be prepared safely.
    ScriptError { reason: String },
}

impl ClientError {
    /// Stable module error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedClient { .. } => CODE_UNSUPPORTED_CLIENT,
            Self::ClientNotFound { .. } => CODE_CLIENT_NOT_FOUND,
            Self::LaunchFailed { .. } => CODE_LAUNCH_FAILED,
            Self::InvalidTarget { .. } => CODE_INVALID_TARGET,
            Self::NoTerminal => CODE_NO_TERMINAL,
            Self::ScriptError { .. } => CODE_SCRIPT_ERROR,
        }
    }

    /// User-safe message.
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::UnsupportedClient { .. } => messages::UNSUPPORTED_CLIENT,
            Self::ClientNotFound { .. } => messages::CLIENT_NOT_FOUND,
            Self::LaunchFailed { .. } => messages::LAUNCH_FAILED,
            Self::InvalidTarget { .. } => messages::INVALID_TARGET,
            Self::NoTerminal => messages::NO_TERMINAL,
            Self::ScriptError { .. } => messages::SCRIPT_ERROR,
        }
    }
}

impl fmt::Debug for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedClient { client, engine } => f
                .debug_struct("UnsupportedClient")
                .field("client", client)
                .field("engine", engine)
                .finish(),
            Self::ClientNotFound { .. } => f.write_str("ClientNotFound { .. }"),
            Self::LaunchFailed { .. } => f.write_str("LaunchFailed { .. }"),
            Self::InvalidTarget { field } => f
                .debug_struct("InvalidTarget")
                .field("field", field)
                .finish(),
            Self::NoTerminal => f.write_str("NoTerminal"),
            Self::ScriptError { .. } => f.write_str("ScriptError { .. }"),
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code(), self.user_message())
    }
}

impl std::error::Error for ClientError {}

impl From<ClientError> for UserFacingError {
    fn from(err: ClientError) -> Self {
        UserFacingError::new(err.code(), err.user_message())
    }
}

pub type ClientResult<T> = std::result::Result<T, ClientError>;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const CANARY: &str = "SUPER_SECRET_CANARY_123";

    fn all_variants() -> Vec<ClientError> {
        vec![
            ClientError::UnsupportedClient {
                client: "psql",
                engine: "Valkey",
            },
            ClientError::ClientNotFound {
                name: "/tmp/client".to_owned(),
            },
            ClientError::LaunchFailed {
                reason: "spawn failed".to_owned(),
            },
            ClientError::InvalidTarget { field: "database" },
            ClientError::NoTerminal,
            ClientError::ScriptError {
                reason: "bad dir".to_owned(),
            },
        ]
    }

    #[test]
    fn codes_are_unique_and_in_range() {
        let mut seen = BTreeSet::new();
        for err in all_variants() {
            let code = err.code();
            let num: u16 = code.strip_prefix("DBL-").unwrap().parse().unwrap();
            assert!((8300..=8399).contains(&num), "code out of range: {code}");
            assert!(seen.insert(code), "duplicate code {code}");
        }
    }

    #[test]
    fn user_messages_are_non_empty_and_safe() {
        for err in all_variants() {
            let msg = err.user_message();
            assert!(!msg.is_empty());
            assert!(!msg.contains(CANARY));
        }
    }

    #[test]
    fn debug_and_display_never_contain_canary() {
        let variants = vec![
            ClientError::UnsupportedClient {
                client: "psql",
                engine: "Valkey",
            },
            ClientError::ClientNotFound {
                name: CANARY.to_owned(),
            },
            ClientError::LaunchFailed {
                reason: CANARY.to_owned(),
            },
            ClientError::InvalidTarget { field: "database" },
            ClientError::NoTerminal,
            ClientError::ScriptError {
                reason: CANARY.to_owned(),
            },
        ];
        for err in variants {
            let dbg = format!("{err:?}");
            let disp = format!("{err}");
            assert!(!dbg.contains(CANARY), "debug leaked: {dbg}");
            assert!(!disp.contains(CANARY), "display leaked: {disp}");
            let facing: UserFacingError = err.into();
            let fdbg = format!("{facing:?}");
            let fdisp = format!("{facing}");
            assert!(!fdbg.contains(CANARY));
            assert!(!fdisp.contains(CANARY));
        }
    }

    #[test]
    fn converts_to_user_facing_error() {
        let err = ClientError::ClientNotFound {
            name: "/tmp/x".to_owned(),
        };
        let facing: UserFacingError = err.into();
        assert_eq!(facing.code, CODE_CLIENT_NOT_FOUND);
        assert_eq!(facing.message, messages::CLIENT_NOT_FOUND);
    }
}
