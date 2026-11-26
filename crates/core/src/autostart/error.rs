//! Module-local errors for autostart (DBL-8500-8599).
//!
//! `docs/architecture/error-codes.md` reserves 8500-8599 for "packaging and
//! autostart helpers"; this module therefore owns its own enum instead of
//! adding a variant to the shared `DblError`.

use std::fmt;

use crate::service::UserFacingError;

use super::messages;

/// The module's code range, asserted by the tests in this file.
pub const CODE_RANGE: std::ops::RangeInclusive<u16> = 8500..=8599;

/// No usable login mechanism on this platform.
pub const CODE_UNSUPPORTED: &str = "DBL-8500";
/// Writing or removing a login-item file failed.
pub const CODE_WRITE_FAILED: &str = "DBL-8501";
/// A helper command (`systemctl --user`, `launchctl`, `reg`) failed.
pub const CODE_COMMAND_FAILED: &str = "DBL-8502";
/// The executable path cannot be used as a login command.
pub const CODE_INVALID_EXE: &str = "DBL-8503";
/// The login-item state could not be read.
pub const CODE_STATUS_FAILED: &str = "DBL-8504";
/// Starting the flagged instances could not be scheduled.
pub const CODE_PLAN_FAILED: &str = "DBL-8505";

/// Errors produced by the autostart backends and the instance plan.
#[derive(Clone, PartialEq, Eq)]
pub enum AutostartError {
    /// The platform has no user-level login item this build can write.
    Unsupported {
        /// Why, in a short English phrase for the log.
        reason: &'static str,
    },
    /// A file under the user's config root could not be written or removed.
    WriteFailed {
        /// Offending path, relative to the user's config root where possible.
        path: String,
    },
    /// A helper program exited non-zero or could not be started.
    CommandFailed {
        /// Program that failed, for example `systemctl`.
        program: &'static str,
        /// Short, non-secret detail from stderr.
        detail: String,
    },
    /// The current executable path is not an absolute, usable command.
    InvalidExe {
        /// Why the path was rejected.
        reason: &'static str,
    },
    /// Reading the current login-item state failed.
    StatusFailed {
        /// Short, non-secret detail.
        detail: String,
    },
    /// The bounded worker pool could not be created.
    PlanFailed {
        /// Short, non-secret detail.
        detail: String,
    },
}

impl AutostartError {
    /// Stable module error code, inside [`CODE_RANGE`].
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unsupported { .. } => CODE_UNSUPPORTED,
            Self::WriteFailed { .. } => CODE_WRITE_FAILED,
            Self::CommandFailed { .. } => CODE_COMMAND_FAILED,
            Self::InvalidExe { .. } => CODE_INVALID_EXE,
            Self::StatusFailed { .. } => CODE_STATUS_FAILED,
            Self::PlanFailed { .. } => CODE_PLAN_FAILED,
        }
    }

    /// User-safe message.
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::Unsupported { .. } => messages::ERR_UNSUPPORTED,
            Self::WriteFailed { .. } => messages::ERR_WRITE_FAILED,
            Self::CommandFailed { .. } => messages::ERR_COMMAND_FAILED,
            Self::InvalidExe { .. } => messages::ERR_INVALID_EXE,
            Self::StatusFailed { .. } => messages::ERR_STATUS_FAILED,
            Self::PlanFailed { .. } => messages::ERR_PLAN_FAILED,
        }
    }
}

impl fmt::Debug for AutostartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Payloads that can carry filesystem paths or helper stderr are omitted;
        // `reason` and `program` are `&'static str`, so they cannot leak input.
        match self {
            Self::Unsupported { reason } => f
                .debug_struct("Unsupported")
                .field("reason", reason)
                .finish(),
            Self::WriteFailed { .. } => f.write_str("WriteFailed { .. }"),
            Self::CommandFailed { program, .. } => f
                .debug_struct("CommandFailed")
                .field("program", program)
                .finish(),
            Self::InvalidExe { reason } => f
                .debug_struct("InvalidExe")
                .field("reason", reason)
                .finish(),
            Self::StatusFailed { .. } => f.write_str("StatusFailed { .. }"),
            Self::PlanFailed { .. } => f.write_str("PlanFailed { .. }"),
        }
    }
}

impl fmt::Display for AutostartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code(), self.user_message())
    }
}

impl std::error::Error for AutostartError {}

impl From<AutostartError> for UserFacingError {
    fn from(err: AutostartError) -> Self {
        UserFacingError::new(err.code(), err.user_message())
    }
}

/// Result alias for this module.
pub type AutostartResult<T> = std::result::Result<T, AutostartError>;

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::collections::BTreeSet;

    use super::*;

    const CANARY: &str = "SUPER_SECRET_CANARY_123";

    fn all_variants() -> Vec<AutostartError> {
        vec![
            AutostartError::Unsupported { reason: "no bus" },
            AutostartError::WriteFailed {
                path: "~/.config/systemd/user/dblocal.service".to_owned(),
            },
            AutostartError::CommandFailed {
                program: "systemctl",
                detail: "Failed to connect to bus".to_owned(),
            },
            AutostartError::InvalidExe {
                reason: "not absolute",
            },
            AutostartError::StatusFailed {
                detail: "reg query returned 1".to_owned(),
            },
            AutostartError::PlanFailed {
                detail: "thread spawn failed".to_owned(),
            },
        ]
    }

    #[test]
    fn codes_are_unique_and_inside_the_module_range() {
        let mut seen = BTreeSet::new();
        for err in all_variants() {
            let code = err.code();
            let number: u16 = code
                .strip_prefix("DBL-")
                .expect("prefix")
                .parse()
                .expect("digits");
            assert!(CODE_RANGE.contains(&number), "code out of range: {code}");
            assert!(seen.insert(code), "duplicate code {code}");
        }
    }

    #[test]
    fn messages_are_non_empty_and_never_leak_a_payload() {
        for err in all_variants() {
            assert!(!err.user_message().is_empty());
        }
        let leaky = vec![
            AutostartError::WriteFailed {
                path: CANARY.to_owned(),
            },
            AutostartError::CommandFailed {
                program: "systemctl",
                detail: CANARY.to_owned(),
            },
            AutostartError::StatusFailed {
                detail: CANARY.to_owned(),
            },
            AutostartError::PlanFailed {
                detail: CANARY.to_owned(),
            },
        ];
        for err in leaky {
            assert!(
                !format!("{err:?}").contains(CANARY),
                "debug leaked: {err:?}"
            );
            assert!(!format!("{err}").contains(CANARY), "display leaked: {err}");
            let facing: UserFacingError = err.into();
            assert!(!format!("{facing:?}").contains(CANARY));
            assert!(!format!("{facing}").contains(CANARY));
        }
    }

    #[test]
    fn codes_are_constants_not_literals() {
        assert_eq!(
            AutostartError::Unsupported { reason: "x" }.code(),
            CODE_UNSUPPORTED
        );
        assert_eq!(
            AutostartError::WriteFailed {
                path: "p".to_owned()
            }
            .code(),
            CODE_WRITE_FAILED
        );
        assert_eq!(
            AutostartError::CommandFailed {
                program: "reg",
                detail: "d".to_owned()
            }
            .code(),
            CODE_COMMAND_FAILED
        );
        assert_eq!(
            AutostartError::InvalidExe { reason: "r" }.code(),
            CODE_INVALID_EXE
        );
        assert_eq!(
            AutostartError::StatusFailed {
                detail: "d".to_owned()
            }
            .code(),
            CODE_STATUS_FAILED
        );
        assert_eq!(
            AutostartError::PlanFailed {
                detail: "d".to_owned()
            }
            .code(),
            CODE_PLAN_FAILED
        );
    }

    #[test]
    fn converts_to_a_user_facing_error() {
        let facing: UserFacingError = AutostartError::Unsupported { reason: "x" }.into();
        assert_eq!(facing.code, CODE_UNSUPPORTED);
        assert_eq!(facing.message, messages::ERR_UNSUPPORTED);
        assert!(facing.diagnosis.is_none());
    }

    #[test]
    fn display_carries_code_and_message() {
        let err = AutostartError::Unsupported { reason: "x" };
        assert_eq!(
            format!("{err}"),
            format!("[{CODE_UNSUPPORTED}] {}", messages::ERR_UNSUPPORTED)
        );
    }
}
