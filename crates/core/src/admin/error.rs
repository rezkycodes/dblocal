//! Typed admin failures with stable `DBL-81xx` codes.
//!
//! Every [`AdminError`] is convertible to the public
//! [`crate::service::UserFacingError`]. No variant carries a free-form string,
//! so a driver message, a password, a connection string or a failing statement
//! cannot travel through this type at all: payloads are limited to validated
//! identifiers, a five-character SQLSTATE and booleans describing rollback
//! state. `Debug` and `Display` are therefore safe by construction, which the
//! canary tests at the bottom prove.

use crate::service::UserFacingError;

use super::messages;

/// Range this module owns, inclusive on both ends.
pub const CODE_RANGE: std::ops::RangeInclusive<u16> = 8100..=8199;

/// Code for [`AdminError::Connect`].
pub const CODE_CONNECT: &str = "DBL-8100";
/// Code for [`AdminError::InvalidIdentifier`].
pub const CODE_INVALID_IDENTIFIER: &str = "DBL-8101";
/// Code for [`AdminError::NonLoopbackHost`].
pub const CODE_NON_LOOPBACK: &str = "DBL-8102";
/// Code for [`AdminError::InvalidTarget`].
pub const CODE_INVALID_TARGET: &str = "DBL-8103";
/// Code for [`AdminError::DatabaseExists`].
pub const CODE_DATABASE_EXISTS: &str = "DBL-8104";
/// Code for [`AdminError::DatabaseNotFound`].
pub const CODE_DATABASE_NOT_FOUND: &str = "DBL-8105";
/// Code for [`AdminError::DatabaseInUse`].
pub const CODE_DATABASE_IN_USE: &str = "DBL-8106";
/// Code for [`AdminError::UserExists`].
pub const CODE_USER_EXISTS: &str = "DBL-8107";
/// Code for [`AdminError::UserNotFound`].
pub const CODE_USER_NOT_FOUND: &str = "DBL-8108";
/// Code for [`AdminError::InsufficientPrivilege`].
pub const CODE_INSUFFICIENT_PRIVILEGE: &str = "DBL-8109";
/// Code for [`AdminError::QueryFailed`].
pub const CODE_QUERY_FAILED: &str = "DBL-8110";
/// Code for [`AdminError::Unsupported`].
pub const CODE_UNSUPPORTED: &str = "DBL-8111";
/// Code for [`AdminError::PasswordVerify`].
pub const CODE_PASSWORD_VERIFY: &str = "DBL-8112";
/// Code for [`AdminError::CredentialStore`].
pub const CODE_CREDENTIAL_STORE: &str = "DBL-8113";
/// Code for [`AdminError::RollbackFailed`].
pub const CODE_ROLLBACK: &str = "DBL-8114";
/// Code for [`AdminError::PasswordGenerate`].
pub const CODE_PASSWORD_GENERATE: &str = "DBL-8115";
/// Code for [`AdminError::AuthFailed`].
pub const CODE_AUTH_FAILED: &str = "DBL-8116";
/// Code for [`AdminError::NoPassword`].
///
/// Distinct from [`CODE_UNSUPPORTED`]: the engine is PostgreSQL (supported), but
/// this instance was created without a password, so there is no credential to
/// authenticate an admin session with. The fix is a different instance, not a
/// later engine adapter.
pub const CODE_NO_PASSWORD: &str = "DBL-8117";
/// Code for [`AdminError::SystemSchema`].
///
/// MySQL/MariaDB own `mysql`, `sys`, `information_schema` and
/// `performance_schema`; admin must never drop or manage them.
pub const CODE_SYSTEM_SCHEMA: &str = "DBL-8118";

/// Why an identifier was rejected. Static: it never echoes the rejected value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierReject {
    /// The name was empty.
    Empty,
    /// The name exceeded the engine limit.
    TooLong,
    /// The name began with a digit.
    StartsWithDigit,
    /// The name contained a character outside `[A-Za-z0-9_]`.
    BadCharacter,
    /// The name is a reserved engine name (`postgres`, `template0`, `template1`).
    Reserved,
}

/// A SQLSTATE, limited to five uppercase ASCII letters or digits.
///
/// This is the only engine-provided value the error type accepts, and it is
/// validated on construction so the raw driver text can never be carried.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SqlStateCode([u8; 5]);

impl SqlStateCode {
    /// Wraps a five-character SQLSTATE, rejecting anything else.
    pub fn parse(raw: &str) -> Option<Self> {
        let bytes = raw.as_bytes();
        if bytes.len() != 5 {
            return None;
        }
        let mut out = [0u8; 5];
        for (slot, byte) in out.iter_mut().zip(bytes) {
            if !(byte.is_ascii_digit() || byte.is_ascii_uppercase()) {
                return None;
            }
            *slot = *byte;
        }
        Some(Self(out))
    }

    /// The five-character SQLSTATE.
    pub fn as_str(&self) -> &str {
        // SAFETY of the index: the buffer is five ASCII bytes by construction.
        std::str::from_utf8(&self.0).unwrap_or("?????")
    }
}

impl std::fmt::Display for SqlStateCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::fmt::Debug for SqlStateCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SqlStateCode({})", self.as_str())
    }
}

/// Why a connection target was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetProblem {
    /// No host was given.
    EmptyHost,
    /// No superuser name was given.
    EmptySuperuser,
    /// The port was zero.
    ZeroPort,
}

/// How far a password reset got before it failed.
///
/// Carries only booleans: what the operator needs to recover without ever
/// exposing the old or new secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollbackState {
    /// The engine accepted the new password before the failure.
    pub engine_changed: bool,
    /// The credential sink failed, which triggered the rollback.
    pub sink_failed: bool,
    /// The rollback attempt itself failed, leaving the state inconsistent.
    pub rollback_failed: bool,
}

/// Every failure an admin operation can report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminError {
    /// The engine could not be reached over TCP.
    Connect,
    /// The engine rejected the stored credential (`28P01` / `28000`).
    AuthFailed,
    /// A name failed the allowlist.
    InvalidIdentifier(IdentifierReject),
    /// The host was not a loopback address.
    NonLoopbackHost,
    /// The target description was incomplete.
    InvalidTarget(TargetProblem),
    /// The database already exists.
    DatabaseExists,
    /// The database does not exist.
    DatabaseNotFound,
    /// The database still has connections; dropping it would fail.
    DatabaseInUse,
    /// The role already exists.
    UserExists,
    /// The role does not exist.
    UserNotFound,
    /// The server refused the command for lack of privilege.
    InsufficientPrivilege,
    /// The server rejected the command for another reason.
    QueryFailed(Option<SqlStateCode>),
    /// This engine does not support the operation.
    Unsupported,
    /// The new password did not authenticate on a fresh connection.
    PasswordVerify,
    /// The credential sink refused the new password.
    CredentialStore,
    /// The rollback to the old password failed.
    RollbackFailed(RollbackState),
    /// The operating system RNG failed while generating a password.
    PasswordGenerate,
    /// The instance was created without a password, so admin needs a different
    /// instance (`F-DBU`); the engine itself is supported.
    NoPassword,
    /// The requested database is one the MySQL/MariaDB engine owns (or a name
    /// reserved by the engine), and admin must not manage it.
    SystemSchema,
}

impl AdminError {
    /// Stable, searchable code inside `DBL-8100..=8199`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Connect => CODE_CONNECT,
            Self::AuthFailed => CODE_AUTH_FAILED,
            Self::InvalidIdentifier(_) => CODE_INVALID_IDENTIFIER,
            Self::NonLoopbackHost => CODE_NON_LOOPBACK,
            Self::InvalidTarget(_) => CODE_INVALID_TARGET,
            Self::DatabaseExists => CODE_DATABASE_EXISTS,
            Self::DatabaseNotFound => CODE_DATABASE_NOT_FOUND,
            Self::DatabaseInUse => CODE_DATABASE_IN_USE,
            Self::UserExists => CODE_USER_EXISTS,
            Self::UserNotFound => CODE_USER_NOT_FOUND,
            Self::InsufficientPrivilege => CODE_INSUFFICIENT_PRIVILEGE,
            Self::QueryFailed(_) => CODE_QUERY_FAILED,
            Self::Unsupported => CODE_UNSUPPORTED,
            Self::PasswordVerify => CODE_PASSWORD_VERIFY,
            Self::CredentialStore => CODE_CREDENTIAL_STORE,
            Self::RollbackFailed(_) => CODE_ROLLBACK,
            Self::PasswordGenerate => CODE_PASSWORD_GENERATE,
            Self::NoPassword => CODE_NO_PASSWORD,
            Self::SystemSchema => CODE_SYSTEM_SCHEMA,
        }
    }

    /// User-facing message for the UI. Static per variant, so it can never embed
    /// a password, a connection string or the text of a failing statement.
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::Connect => messages::ERR_CONNECT,
            Self::AuthFailed => messages::ERR_AUTH_FAILED,
            Self::InvalidIdentifier(_) => messages::ERR_INVALID_IDENTIFIER,
            Self::NonLoopbackHost => messages::ERR_NON_LOOPBACK,
            Self::InvalidTarget(_) => messages::ERR_INVALID_TARGET,
            Self::DatabaseExists => messages::ERR_DATABASE_EXISTS,
            Self::DatabaseNotFound => messages::ERR_DATABASE_NOT_FOUND,
            Self::DatabaseInUse => messages::ERR_DATABASE_IN_USE,
            Self::UserExists => messages::ERR_USER_EXISTS,
            Self::UserNotFound => messages::ERR_USER_NOT_FOUND,
            Self::InsufficientPrivilege => messages::ERR_INSUFFICIENT_PRIVILEGE,
            Self::QueryFailed(_) => messages::ERR_QUERY_FAILED,
            Self::Unsupported => messages::ERR_UNSUPPORTED,
            Self::PasswordVerify => messages::ERR_PASSWORD_VERIFY,
            Self::CredentialStore => messages::ERR_CREDENTIAL_STORE,
            Self::RollbackFailed(_) => messages::ERR_ROLLBACK,
            Self::PasswordGenerate => messages::ERR_PASSWORD_GENERATE,
            Self::NoPassword => messages::ERR_NO_PASSWORD,
            Self::SystemSchema => messages::ERR_SYSTEM_SCHEMA,
        }
    }

    /// Short English label for developer logs. Never carries a payload.
    fn label(&self) -> &'static str {
        match self {
            Self::Connect => "connect failed",
            Self::AuthFailed => "authentication failed",
            Self::InvalidIdentifier(reason) => match reason {
                IdentifierReject::Empty => "identifier empty",
                IdentifierReject::TooLong => "identifier too long",
                IdentifierReject::StartsWithDigit => "identifier starts with a digit",
                IdentifierReject::BadCharacter => "identifier has a forbidden character",
                IdentifierReject::Reserved => "identifier is reserved",
            },
            Self::NonLoopbackHost => "host is not loopback",
            Self::InvalidTarget(problem) => match problem {
                TargetProblem::EmptyHost => "target host empty",
                TargetProblem::EmptySuperuser => "target superuser empty",
                TargetProblem::ZeroPort => "target port zero",
            },
            Self::DatabaseExists => "database exists",
            Self::DatabaseNotFound => "database not found",
            Self::DatabaseInUse => "database in use",
            Self::UserExists => "user exists",
            Self::UserNotFound => "user not found",
            Self::InsufficientPrivilege => "insufficient privilege",
            Self::QueryFailed(_) => "query failed",
            Self::Unsupported => "unsupported",
            Self::PasswordVerify => "new password did not authenticate",
            Self::CredentialStore => "credential store refused the password",
            Self::RollbackFailed(_) => "rollback failed, state inconsistent",
            Self::PasswordGenerate => "password generation failed",
            Self::NoPassword => "instance has no password",
            Self::SystemSchema => "database is an engine-owned system schema",
        }
    }
}

impl std::fmt::Display for AdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.code(), self.label())
    }
}

impl From<AdminError> for UserFacingError {
    fn from(err: AdminError) -> Self {
        UserFacingError::new(err.code(), err.user_message())
    }
}

/// Result alias for admin work.
pub type AdminResult<T> = std::result::Result<T, AdminError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant, with a representative payload.
    fn all_variants() -> Vec<AdminError> {
        vec![
            AdminError::Connect,
            AdminError::AuthFailed,
            AdminError::InvalidIdentifier(IdentifierReject::Empty),
            AdminError::InvalidIdentifier(IdentifierReject::TooLong),
            AdminError::InvalidIdentifier(IdentifierReject::StartsWithDigit),
            AdminError::InvalidIdentifier(IdentifierReject::BadCharacter),
            AdminError::InvalidIdentifier(IdentifierReject::Reserved),
            AdminError::NonLoopbackHost,
            AdminError::InvalidTarget(TargetProblem::EmptyHost),
            AdminError::InvalidTarget(TargetProblem::EmptySuperuser),
            AdminError::InvalidTarget(TargetProblem::ZeroPort),
            AdminError::DatabaseExists,
            AdminError::DatabaseNotFound,
            AdminError::DatabaseInUse,
            AdminError::UserExists,
            AdminError::UserNotFound,
            AdminError::InsufficientPrivilege,
            AdminError::QueryFailed(None),
            AdminError::QueryFailed(SqlStateCode::parse("42P04")),
            AdminError::Unsupported,
            AdminError::PasswordVerify,
            AdminError::CredentialStore,
            AdminError::RollbackFailed(RollbackState {
                engine_changed: true,
                sink_failed: true,
                rollback_failed: true,
            }),
            AdminError::PasswordGenerate,
            AdminError::NoPassword,
            AdminError::SystemSchema,
        ]
    }

    fn code_number(code: &str) -> Option<u16> {
        let digits = code.strip_prefix("DBL-")?;
        if digits.len() != 4 {
            return None;
        }
        digits.parse().ok()
    }

    #[test]
    fn every_code_is_inside_the_range_and_unique_per_variant() {
        let mut seen = std::collections::BTreeSet::new();
        let mut per_variant = std::collections::BTreeMap::new();
        for err in all_variants() {
            let code = err.code();
            let number = code_number(code).unwrap_or_else(|| panic!("malformed code {code}"));
            assert!(
                CODE_RANGE.contains(&number),
                "{code} is outside DBL-8100..=8199"
            );
            let label = err.label();
            if let Some(previous) = per_variant.insert(label, code) {
                assert_eq!(
                    previous, code,
                    "the same label must not carry two codes: {label}"
                );
            }
            seen.insert(code);
        }
        // Every declared code constant appears in the variant list exactly once.
        let declared = [
            CODE_CONNECT,
            CODE_AUTH_FAILED,
            CODE_INVALID_IDENTIFIER,
            CODE_NON_LOOPBACK,
            CODE_INVALID_TARGET,
            CODE_DATABASE_EXISTS,
            CODE_DATABASE_NOT_FOUND,
            CODE_DATABASE_IN_USE,
            CODE_USER_EXISTS,
            CODE_USER_NOT_FOUND,
            CODE_INSUFFICIENT_PRIVILEGE,
            CODE_QUERY_FAILED,
            CODE_UNSUPPORTED,
            CODE_PASSWORD_VERIFY,
            CODE_CREDENTIAL_STORE,
            CODE_ROLLBACK,
            CODE_PASSWORD_GENERATE,
            CODE_NO_PASSWORD,
            CODE_SYSTEM_SCHEMA,
        ];
        for code in declared {
            assert!(
                seen.contains(code),
                "declared code {code} is never produced"
            );
        }
        assert_eq!(seen.len(), declared.len());
    }

    #[test]
    fn codes_map_to_distinct_constant_values() {
        let mut numbers: Vec<u16> = [
            CODE_CONNECT,
            CODE_AUTH_FAILED,
            CODE_INVALID_IDENTIFIER,
            CODE_NON_LOOPBACK,
            CODE_INVALID_TARGET,
            CODE_DATABASE_EXISTS,
            CODE_DATABASE_NOT_FOUND,
            CODE_DATABASE_IN_USE,
            CODE_USER_EXISTS,
            CODE_USER_NOT_FOUND,
            CODE_INSUFFICIENT_PRIVILEGE,
            CODE_QUERY_FAILED,
            CODE_UNSUPPORTED,
            CODE_PASSWORD_VERIFY,
            CODE_CREDENTIAL_STORE,
            CODE_ROLLBACK,
            CODE_PASSWORD_GENERATE,
            CODE_NO_PASSWORD,
            CODE_SYSTEM_SCHEMA,
        ]
        .iter()
        .map(|code| code_number(code).expect("well formed"))
        .collect();
        let before = numbers.len();
        numbers.sort_unstable();
        numbers.dedup();
        assert_eq!(before, numbers.len(), "two constants share a number");
    }

    #[test]
    fn user_messages_are_static_and_nonempty() {
        for err in all_variants() {
            let message = err.user_message();
            assert!(!message.is_empty(), "{} has an empty message", err.code());
            assert!(
                !message.contains("DBL-"),
                "the message must not repeat the code"
            );
        }
    }

    #[test]
    fn display_and_debug_are_stable() {
        let err = AdminError::DatabaseInUse;
        assert_eq!(err.to_string(), "DBL-8106 (database in use)");
        assert!(format!("{err:?}").contains("DatabaseInUse"));
    }

    #[test]
    fn conversion_to_user_facing_keeps_code_and_message() {
        let err = AdminError::NonLoopbackHost;
        let facing = UserFacingError::from(err.clone());
        assert_eq!(facing.code, CODE_NON_LOOPBACK);
        assert_eq!(facing.message, err.user_message());
        assert!(facing.diagnosis.is_none());
    }

    #[test]
    fn sql_state_accepts_only_five_uppercase_alphanumerics() {
        assert_eq!(
            SqlStateCode::parse("42P04").expect("valid").as_str(),
            "42P04"
        );
        assert_eq!(
            SqlStateCode::parse("08006").expect("valid").as_str(),
            "08006"
        );
        for bad in ["", "42P0", "42P044", "42p04", "42P0-", "42P0\n", "42 04"] {
            assert!(SqlStateCode::parse(bad).is_none(), "accepted {bad:?}");
        }
    }

    /// A password and a connection string fed to every constructor that takes
    /// text must be refused, and must never appear in any rendered surface of
    /// any variant.
    #[test]
    fn canary_secret_never_reaches_any_surface() {
        const PASSWORD: &str = "canary-password-9f3a";
        const URI: &str = "postgresql://postgres:canary-password-9f3a@127.0.0.1:5432/postgres";

        // The only two types that accept text are validated and reject both.
        for canary in [PASSWORD, URI] {
            assert!(
                SqlStateCode::parse(canary).is_none(),
                "SqlStateCode accepted the canary"
            );
        }

        for err in all_variants() {
            let surfaces = [
                format!("{err:?}"),
                format!("{err}"),
                err.code().to_owned(),
                err.user_message().to_owned(),
                UserFacingError::from(err.clone()).to_string(),
            ];
            for surface in surfaces {
                assert!(
                    !surface.contains(PASSWORD),
                    "password leaked through {surface:?}"
                );
                assert!(
                    !surface.contains(URI),
                    "connection string leaked through {surface:?}"
                );
            }
        }
    }

    /// Proves the error type has no slot for driver text: the renderings of a
    /// query failure with a SQLSTATE contain the code and nothing else.
    #[test]
    fn query_failure_carries_only_a_scrubbed_sqlstate() {
        let state = SqlStateCode::parse("42501").expect("valid");
        let err = AdminError::QueryFailed(Some(state));
        let rendered = format!("{err:?} {err} {}", err.user_message());
        assert!(rendered.contains("42501"));
        assert!(!rendered.to_lowercase().contains("select"));
        assert!(!rendered.to_lowercase().contains("password"));
    }
}
