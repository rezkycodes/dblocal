//! Small validated scalar types shared by the whole domain.

use std::fmt;
use std::str::FromStr;

use uuid::Uuid;

use crate::DblError;
use crate::consts;
use crate::error::Result;

/// Protocol family an engine belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Family {
    /// PostgreSQL wire protocol.
    Postgres,
    /// MySQL wire protocol, shared by MySQL and MariaDB.
    Mysql,
    /// RESP, used by Valkey.
    Redis,
}

impl Family {
    /// Stable identifier used in the catalog JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Redis => "redis",
        }
    }
}

/// The four engines DBLocal can manage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum EngineKey {
    /// PostgreSQL.
    Postgresql,
    /// MySQL.
    Mysql,
    /// MariaDB.
    Mariadb,
    /// Valkey.
    Valkey,
}

impl EngineKey {
    /// Every engine, in catalog order.
    pub const ALL: [Self; 4] = [Self::Postgresql, Self::Mysql, Self::Mariadb, Self::Valkey];

    /// Stable identifier used in paths, the catalog and SQLite.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgresql => "postgresql",
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
            Self::Valkey => "valkey",
        }
    }

    /// First port offered when no instance of this engine exists yet.
    pub const fn canonical_port(self) -> u16 {
        match self {
            Self::Postgresql => consts::PORT_POSTGRESQL,
            Self::Mysql => consts::PORT_MYSQL,
            Self::Mariadb => consts::PORT_MARIADB,
            Self::Valkey => consts::PORT_VALKEY,
        }
    }

    /// Name shown in the UI.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Postgresql => "PostgreSQL",
            Self::Mysql => "MySQL",
            Self::Mariadb => "MariaDB",
            Self::Valkey => "Valkey",
        }
    }

    /// Protocol family, used to pick drivers and config writers.
    pub const fn family(self) -> Family {
        match self {
            Self::Postgresql => Family::Postgres,
            Self::Mysql | Self::Mariadb => Family::Mysql,
            Self::Valkey => Family::Redis,
        }
    }

    /// Canonical superuser name for this engine.
    pub const fn default_superuser(self) -> &'static str {
        match self {
            Self::Postgresql => consts::SUPERUSER_POSTGRESQL,
            Self::Mysql | Self::Mariadb => consts::SUPERUSER_MYSQL,
            Self::Valkey => consts::SUPERUSER_VALKEY,
        }
    }
}

impl fmt::Display for EngineKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EngineKey {
    type Err = DblError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "postgresql" => Ok(Self::Postgresql),
            "mysql" => Ok(Self::Mysql),
            "mariadb" => Ok(Self::Mariadb),
            "valkey" => Ok(Self::Valkey),
            other => Err(DblError::InvalidInput {
                field: "engine",
                reason: format!("unknown engine: {other}"),
            }),
        }
    }
}

/// Stable identity of an instance, a UUID v4 in string form.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId(String);

impl InstanceId {
    /// Generates a new random identifier.
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Wraps an identifier read from storage.
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        Uuid::parse_str(&value).map_err(|_| DblError::InvalidInput {
            field: "instance_id",
            reason: "not a UUID".to_owned(),
        })?;
        Ok(Self(value))
    }

    /// Borrowed string form.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for InstanceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for InstanceId {
    type Err = DblError;

    fn from_str(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}

/// A validated instance name, also used as the data directory name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceName(String);

impl InstanceName {
    /// Validates `^[a-z0-9][a-z0-9-]{1,30}$`.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        let bytes = value.as_bytes();
        let len_ok =
            (consts::INSTANCE_NAME_MIN_LEN..=consts::INSTANCE_NAME_MAX_LEN).contains(&bytes.len());
        let chars_ok = bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
        let start_ok = bytes
            .first()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        let end_ok = bytes.last().is_some_and(|b| *b != b'-');
        if len_ok && chars_ok && start_ok && end_ok {
            Ok(Self(value))
        } else {
            Err(DblError::InvalidInput {
                field: "name",
                reason: "must match ^[a-z0-9][a-z0-9-]{1,30}$".to_owned(),
            })
        }
    }

    /// Borrowed string form.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InstanceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for InstanceName {
    type Err = DblError;

    fn from_str(value: &str) -> Result<Self> {
        Self::new(value)
    }
}

/// A TCP port an instance may listen on. Never below 1024.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Port(u16);

impl Port {
    /// Validates the usable range.
    pub fn new(value: u16) -> Result<Self> {
        if (consts::MIN_PORT..=consts::MAX_PORT).contains(&value) {
            Ok(Self(value))
        } else {
            Err(DblError::InvalidInput {
                field: "port",
                reason: format!(
                    "must be between {} and {}",
                    consts::MIN_PORT,
                    consts::MAX_PORT
                ),
            })
        }
    }

    /// Wraps a port that is already known to be valid.
    ///
    /// Only for compile-time constants such as the canonical ports, which are
    /// all above [`crate::consts::MIN_PORT`] by construction.
    pub(crate) const fn assume_valid(value: u16) -> Self {
        debug_assert!(value >= consts::MIN_PORT);
        Self(value)
    }

    /// Raw port number.
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Port {
    type Err = DblError;

    fn from_str(value: &str) -> Result<Self> {
        let parsed = value.parse::<u16>().map_err(|_| DblError::InvalidInput {
            field: "port",
            reason: "not a number".to_owned(),
        })?;
        Self::new(parsed)
    }
}

/// How an instance authenticates clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AuthMode {
    /// Random password generated at create time, stored in the keychain.
    Password,
    /// Trust authentication, only selectable at create time.
    None,
}

impl AuthMode {
    /// Stable identifier used in SQLite.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::None => "none",
        }
    }

    /// Whether a credential is generated for this mode.
    pub const fn requires_password(self) -> bool {
        matches!(self, Self::Password)
    }
}

impl FromStr for AuthMode {
    type Err = DblError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "password" => Ok(Self::Password),
            "none" => Ok(Self::None),
            other => Err(DblError::InvalidInput {
                field: "auth_mode",
                reason: format!("unknown auth mode: {other}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_key_round_trips() {
        for engine in EngineKey::ALL {
            assert_eq!(engine.as_str().parse::<EngineKey>().ok(), Some(engine));
            assert!(!engine.display_name().is_empty());
            assert!(!engine.family().as_str().is_empty());
            assert_eq!(engine.to_string(), engine.as_str());
        }
        assert!(matches!(
            "oracle".parse::<EngineKey>(),
            Err(DblError::InvalidInput { .. })
        ));
    }

    #[test]
    fn canonical_ports_match_prd() {
        assert_eq!(EngineKey::Postgresql.canonical_port(), 5432);
        assert_eq!(EngineKey::Mysql.canonical_port(), 3306);
        assert_eq!(EngineKey::Mariadb.canonical_port(), 3307);
        assert_eq!(EngineKey::Valkey.canonical_port(), 6379);
    }

    #[test]
    fn families_are_grouped() {
        assert_eq!(EngineKey::Postgresql.family(), Family::Postgres);
        assert_eq!(EngineKey::Mysql.family(), Family::Mysql);
        assert_eq!(EngineKey::Mariadb.family(), Family::Mysql);
        assert_eq!(EngineKey::Valkey.family(), Family::Redis);
    }

    #[test]
    fn instance_name_boundaries() {
        assert!(InstanceName::new("ab").is_ok());
        assert!(InstanceName::new("a2").is_ok());
        assert!(InstanceName::new("app-dev-1").is_ok());
        assert!(InstanceName::new("a1").is_ok());
        assert!(InstanceName::new("a".repeat(31)).is_ok());

        assert!(InstanceName::new("a").is_err(), "one char is too short");
        assert!(
            InstanceName::new("a".repeat(32)).is_err(),
            "32 chars is too long"
        );
        assert!(InstanceName::new("").is_err());
        assert!(InstanceName::new("-ab").is_err());
        assert!(InstanceName::new("ab-").is_err());
        assert!(InstanceName::new("App").is_err());
        assert!(InstanceName::new("app_dev").is_err());
        assert!(InstanceName::new("app dev").is_err());
        assert!(InstanceName::new("../etc").is_err());
        assert!(InstanceName::new("app/../x").is_err());
    }

    #[test]
    fn port_boundaries() {
        assert!(Port::new(1024).is_ok());
        assert!(Port::new(65535).is_ok());
        assert_eq!(Port::new(5432).map(Port::get).ok(), Some(5432));

        assert!(Port::new(1023).is_err());
        assert!(Port::new(0).is_err());
        assert!("70000".parse::<Port>().is_err());
        assert!("abc".parse::<Port>().is_err());
        assert_eq!("5432".parse::<Port>().map(Port::get).ok(), Some(5432));
    }

    #[test]
    fn instance_id_generates_and_parses() {
        let id = InstanceId::new();
        assert_eq!(id.as_str().len(), 36);
        assert_eq!(InstanceId::parse(id.as_str()).ok(), Some(id.clone()));
        assert!(InstanceId::parse("not-a-uuid").is_err());
    }

    #[test]
    fn auth_mode_round_trips() {
        for mode in [AuthMode::Password, AuthMode::None] {
            assert_eq!(mode.as_str().parse::<AuthMode>().ok(), Some(mode));
        }
        assert!(AuthMode::Password.requires_password());
        assert!(!AuthMode::None.requires_password());
        assert!("md5".parse::<AuthMode>().is_err());
    }
}
