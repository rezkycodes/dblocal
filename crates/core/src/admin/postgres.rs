//! PostgreSQL admin execution.
//!
//! All connections go to loopback only. Passwords and SQL text must never appear
//! in logs, errors, or Debug output.

use std::net::IpAddr;
use std::time::Duration;

use crate::error::{DblError, Result};
use crate::secret::SecretString;

use super::sql::{self, Dialect, Identifier, PrivilegeLevel, SqlStatement};

/// Connection target for admin operations.
#[derive(Clone)]
pub struct AdminTarget {
    host: String,
    port: u16,
    superuser: String,
    password: SecretString,
    connect_timeout: Duration,
    statement_timeout: Duration,
}

impl AdminTarget {
    /// Default connect timeout.
    pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
    /// Default statement timeout.
    pub const DEFAULT_STATEMENT_TIMEOUT: Duration = Duration::from_secs(30);

    /// Creates a validated admin target.
    pub fn new(host: impl Into<String>, port: u16, superuser: impl Into<String>, password: SecretString) -> Result<Self> {
        let host = host.into();
        validate_loopback_host(&host)?;
        let superuser = superuser.into();
        if superuser.is_empty() {
            return Err(DblError::InvalidInput {
                field: "superuser",
                reason: "superuser must not be empty".to_owned(),
            });
        }
        Ok(Self {
            host,
            port,
            superuser,
            password,
            connect_timeout: Self::DEFAULT_CONNECT_TIMEOUT,
            statement_timeout: Self::DEFAULT_STATEMENT_TIMEOUT,
        })
    }

    /// Hostname/IP (loopback only).
    pub fn host(&self) -> &str {
        &self.host
    }

    /// TCP port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Superuser name.
    pub fn superuser(&self) -> &str {
        &self.superuser
    }

    /// Password as secret.
    pub fn password(&self) -> &SecretString {
        &self.password
    }

    /// Connect timeout.
    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    /// Statement timeout.
    pub fn statement_timeout(&self) -> Duration {
        self.statement_timeout
    }

    /// Returns a copy with custom timeouts.
    pub fn with_timeouts(mut self, connect_timeout: Duration, statement_timeout: Duration) -> Self {
        self.connect_timeout = connect_timeout;
        self.statement_timeout = statement_timeout;
        self
    }
}

impl std::fmt::Debug for AdminTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminTarget")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("superuser", &self.superuser)
            .field("password", &"[REDACTED]")
            .field("connect_timeout", &self.connect_timeout)
            .field("statement_timeout", &self.statement_timeout)
            .finish()
    }
}

fn validate_loopback_host(host: &str) -> Result<()> {
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" {
        return Ok(());
    }
    let ip: IpAddr = host.parse().map_err(|_| DblError::InvalidInput {
        field: "host",
        reason: "host must be a loopback address".to_owned(),
    })?;
    if ip.is_loopback() {
        Ok(())
    } else {
        Err(DblError::InvalidInput {
            field: "host",
            reason: "host must be a loopback address".to_owned(),
        })
    }
}

/// Database information returned by `list_databases`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseInfo {
    name: String,
    owner: String,
    size_bytes: i64,
}

impl DatabaseInfo {
    /// Creates a database info record.
    pub fn new(name: impl Into<String>, owner: impl Into<String>, size_bytes: i64) -> Self {
        Self {
            name: name.into(),
            owner: owner.into(),
            size_bytes,
        }
    }

    /// Database name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Owner role name.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Database size in bytes.
    pub fn size_bytes(&self) -> i64 {
        self.size_bytes
    }
}

/// Database user information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    name: String,
    can_login: bool,
    is_superuser: bool,
}

impl UserInfo {
    /// Creates a user info record.
    pub fn new(name: impl Into<String>, can_login: bool, is_superuser: bool) -> Self {
        Self {
            name: name.into(),
            can_login,
            is_superuser,
        }
    }

    /// Role name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the role can log in.
    pub fn can_login(&self) -> bool {
        self.can_login
    }

    /// Whether the role is a superuser.
    pub fn is_superuser(&self) -> bool {
        self.is_superuser
    }
}

/// Sink for persisting a newly applied credential.
///
/// The real implementation is the keychain; tests inject failures.
pub trait CredentialSink: Send + Sync {
    /// Stores the new password. Returns Err if persistence fails.
    fn store(&self, password: &SecretString) -> Result<()>;
}

/// Result of password reset operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordResetReport {
    /// Whether the engine password was changed.
    pub engine_changed: bool,
    /// Whether the sink stored the new password.
    pub sink_stored: bool,
    /// Whether rollback was attempted after sink failure.
    pub rollback_attempted: bool,
    /// Whether rollback succeeded.
    pub rollback_succeeded: bool,
}
