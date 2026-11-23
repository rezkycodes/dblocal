//! The editable setting subset per engine (`F-CFG-01` AC1) and its validation.
//!
//! Defaults come from the real engine, not from memory. The version each table
//! was checked against, and the command that produced it, are recorded in
//! [`DEFAULTS_PROVENANCE`] and re-checked by the ignored e2e
//! (`tests/config_engine_e2e.rs`), so a catalog bump that changes a default
//! fails the e2e instead of silently showing a stale number.
//!
//! Only the keys below are editable. Everything DBLocal manages itself — the
//! port, the socket paths, the log and pid paths, `requirepass`, `datadir`,
//! `basedir`, `bind`, `dir`, `listen_addresses`, `unix_socket_directories` — is
//! refused here: the port goes through [`change_port`](crate::service::DblService::change_port)
//! and the rest belong to the adapter.

use crate::model::EngineKey;

use super::error::ConfigError;

/// Engine versions the default table was verified against, and where each value
/// came from.
pub const DEFAULTS_PROVENANCE: &[(&str, &str, &str)] = &[
    (
        "postgresql",
        "18.7.0",
        "`postgres -D <initdb> -C <key>` after `initdb -A trust` (boot values)",
    ),
    ("mysql", "8.4.11", "`mysqld --no-defaults --verbose --help`"),
    (
        "mariadb",
        "11.8.10",
        "`mariadbd --no-defaults --verbose --help` (LD_LIBRARY_PATH with the libcrypt shim)",
    ),
    (
        "valkey",
        "9.2.0",
        "`valkey-server` then `CONFIG GET maxmemory maxmemory-policy appendonly save timeout`",
    ),
];

/// Whether a setting needs a restart to take effect.
///
/// Every key in this subset is `Restart` — the form tells the user so, and the
/// service always reports `restart_required = true` after an apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyTime {
    /// Takes effect on the next start only.
    Restart,
}

/// The shape of a value, with the range or set the validator enforces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueKind {
    /// A non-negative integer in `min..=max`.
    Integer {
        /// Lowest accepted value.
        min: u64,
        /// Highest accepted value.
        max: u64,
    },
    /// A byte size with a unit suffix (`kB`, `MB`, `GB`, or the engine's own
    /// words), between `min` and `max` bytes.
    Size {
        /// Lowest accepted size in bytes.
        min: u64,
        /// Highest accepted size in bytes.
        max: u64,
    },
    /// A boolean: `on`/`off` (PostgreSQL) or `yes`/`no` (MySQL, Valkey).
    Boolean {
        /// The engine's word for true.
        on: &'static str,
        /// The engine's word for false.
        off: &'static str,
    },
    /// One value from a fixed set.
    Enum {
        /// Accepted values, lowercase.
        allowed: &'static [&'static str],
    },
    /// A free-form token that must not contain whitespace, a comment character
    /// or a newline (for example a character set or a raw `save` schedule).
    Token {
        /// Lowest length.
        min_len: usize,
        /// Highest length.
        max_len: usize,
    },
    /// An IANA timezone name that exists (`Area/Location`, `GMT`, `UTC`, ...).
    TimeZone,
}

/// One editable setting's static description.
#[derive(Debug, Clone)]
pub struct SettingSpec {
    /// Directive key as the engine spells it.
    pub key: &'static str,
    /// Value shape and range.
    pub kind: ValueKind,
    /// Default value the engine reports when the key is unset.
    pub default: &'static str,
    /// Human unit shown next to the field (`""` when the value is unitless).
    pub unit: &'static str,
    /// Whether the setting needs a restart.
    pub apply: ApplyTime,
    /// Indonesian one-line description, from `config/messages.rs`.
    pub description: &'static str,
}

impl SettingSpec {
    /// The default as a number of bytes for a size, or `None`.
    pub fn default_bytes(&self) -> Option<u64> {
        match self.kind {
            ValueKind::Size { .. } => parse_size(self.default).ok(),
            _ => None,
        }
    }
}

const PG_SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        key: "max_connections",
        kind: ValueKind::Integer {
            min: 1,
            max: 262143,
        },
        default: "100",
        unit: "",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_MAX_CONNECTIONS_PG,
    },
    SettingSpec {
        key: "shared_buffers",
        kind: ValueKind::Size {
            min: 128 * 1024,
            max: 1_073_741_823 * 1024,
        },
        default: "128MB",
        unit: "bytes",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_SHARED_BUFFERS,
    },
    SettingSpec {
        key: "work_mem",
        kind: ValueKind::Size {
            min: 64 * 1024,
            max: 2_147_483_647 * 1024,
        },
        default: "4MB",
        unit: "bytes",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_WORK_MEM,
    },
    SettingSpec {
        key: "timezone",
        kind: ValueKind::TimeZone,
        default: "GMT",
        unit: "",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_TIMEZONE_PG,
    },
];

const MYSQL_SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        key: "max_connections",
        kind: ValueKind::Integer {
            min: 1,
            max: 100_000,
        },
        default: "151",
        unit: "",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_MAX_CONNECTIONS_MYSQL,
    },
    SettingSpec {
        key: "innodb_buffer_pool_size",
        kind: ValueKind::Size {
            min: 5 * 1024 * 1024,
            max: u64::MAX,
        },
        default: "128MB",
        unit: "bytes",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_INNODB_BUFFER_POOL,
    },
    SettingSpec {
        key: "sort_buffer_size",
        kind: ValueKind::Size {
            min: 32 * 1024,
            max: u64::MAX,
        },
        default: "256KB",
        unit: "bytes",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_SORT_BUFFER_MYSQL,
    },
    SettingSpec {
        key: "default-time-zone",
        kind: ValueKind::TimeZone,
        default: "",
        unit: "",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_TIME_ZONE_MYSQL,
    },
    SettingSpec {
        key: "character-set-server",
        kind: ValueKind::Enum {
            allowed: &[
                "utf8mb4", "utf8mb3", "latin1", "ascii", "binary", "ucs2", "utf16", "utf32",
            ],
        },
        default: "utf8mb4",
        unit: "",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_CHARACTER_SET,
    },
];

const MARIADB_SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        key: "max_connections",
        kind: ValueKind::Integer {
            min: 1,
            max: 100_000,
        },
        default: "151",
        unit: "",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_MAX_CONNECTIONS_MARIADB,
    },
    SettingSpec {
        key: "innodb_buffer_pool_size",
        kind: ValueKind::Size {
            min: 5 * 1024 * 1024,
            max: u64::MAX,
        },
        default: "128MB",
        unit: "bytes",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_INNODB_BUFFER_POOL,
    },
    SettingSpec {
        key: "sort_buffer_size",
        kind: ValueKind::Size {
            min: 32 * 1024,
            max: u64::MAX,
        },
        default: "2MB",
        unit: "bytes",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_SORT_BUFFER_MARIADB,
    },
    SettingSpec {
        key: "default-time-zone",
        kind: ValueKind::TimeZone,
        default: "",
        unit: "",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_TIME_ZONE_MARIADB,
    },
    SettingSpec {
        key: "character-set-server",
        kind: ValueKind::Enum {
            allowed: &[
                "utf8mb4", "utf8mb3", "latin1", "ascii", "binary", "ucs2", "utf16", "utf32",
            ],
        },
        default: "utf8mb4",
        unit: "",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_CHARACTER_SET,
    },
];

const VALKEY_SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        key: "maxmemory",
        kind: ValueKind::Size {
            min: 0,
            max: u64::MAX,
        },
        default: "0",
        unit: "bytes",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_MAXMEMORY,
    },
    SettingSpec {
        key: "maxmemory-policy",
        kind: ValueKind::Enum {
            allowed: &[
                "noeviction",
                "allkeys-lru",
                "allkeys-lfu",
                "allkeys-random",
                "volatile-lru",
                "volatile-lfu",
                "volatile-random",
                "volatile-ttl",
            ],
        },
        default: "noeviction",
        unit: "",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_MAXMEMORY_POLICY,
    },
    SettingSpec {
        key: "appendonly",
        kind: ValueKind::Boolean {
            on: "yes",
            off: "no",
        },
        default: "no",
        unit: "",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_APPENDONLY,
    },
    SettingSpec {
        key: "save",
        kind: ValueKind::Token {
            min_len: 0,
            max_len: 128,
        },
        default: "3600 1 300 100 60 10000",
        unit: "",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_SAVE,
    },
    SettingSpec {
        key: "timeout",
        kind: ValueKind::Integer {
            min: 0,
            max: 2_147_483_647,
        },
        default: "0",
        unit: "seconds",
        apply: ApplyTime::Restart,
        description: super::messages::DESC_TIMEOUT,
    },
];

/// The editable settings for an engine, in display order.
pub fn settings_for(engine: EngineKey) -> &'static [SettingSpec] {
    match engine {
        EngineKey::Postgresql => PG_SETTINGS,
        EngineKey::Mysql => MYSQL_SETTINGS,
        EngineKey::Mariadb => MARIADB_SETTINGS,
        EngineKey::Valkey => VALKEY_SETTINGS,
    }
}

/// The read-only entries shown next to the editable ones.
///
/// `port` comes from the instance row; the authentication method is read from
/// the engine's own file and never editable here (see `docs/architecture/config.md`).
pub fn readonly_keys(engine: EngineKey) -> &'static [&'static str] {
    match engine {
        EngineKey::Postgresql => &["port", "authentication method"],
        EngineKey::Mysql | EngineKey::Mariadb => &["port"],
        EngineKey::Valkey => &["port"],
    }
}

/// Looks up a setting by key for an engine.
pub fn spec_for(engine: EngineKey, key: &str) -> Option<&'static SettingSpec> {
    settings_for(engine).iter().find(|spec| spec.key == key)
}

/// True when a key is DBLocal-managed and must go through its own command.
pub fn is_managed_key(key: &str) -> bool {
    const MANAGED: &[&str] = &[
        "port",
        "socket",
        "tmpdir",
        "log",
        "log-error",
        "logfile",
        "pid-file",
        "pidfile",
        "datadir",
        "data_directory",
        "basedir",
        "dir",
        "bind",
        "bind-address",
        "listen_addresses",
        "unix_socket_directories",
        "requirepass",
    ];
    MANAGED
        .iter()
        .any(|managed| managed.eq_ignore_ascii_case(key))
}

/// Parses a byte size the way the engines do (`128MB`, `1GB`, `256 kB`,
/// `134217728`).
///
/// PostgreSQL uses `kB`/`MB`/`GB`/`TB` (1024-based); MySQL and Valkey also
/// accept `K`/`M`/`G`/`T` and a bare number of bytes. The unit is case
/// insensitive. Returns the number of bytes, or a typed error for anything else.
pub fn parse_size(value: &str) -> Result<u64, ConfigError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ConfigError::InvalidValue("ukuran kosong"));
    }
    let (number, unit) = split_number_unit(trimmed)?;
    let digits: String = number.to_owned();
    let base: u64 = if digits.contains('.') {
        // A fractional size is unusual and engine details vary; refuse it rather
        // than guess a rounding.
        return Err(ConfigError::InvalidValue("ukuran harus bilangan bulat"));
    } else {
        digits
            .parse()
            .map_err(|_| ConfigError::InvalidValue("ukuran bukan angka"))?
    };
    let multiplier = match unit.to_ascii_lowercase().as_str() {
        "" | "b" | "bytes" => 1,
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        "t" | "tb" => 1024u64 * 1024 * 1024 * 1024,
        other => {
            let _ = other;
            return Err(ConfigError::InvalidValue("satuan ukuran tidak dikenal"));
        }
    };
    base.checked_mul(multiplier)
        .ok_or(ConfigError::InvalidValue("ukuran terlalu besar"))
}

/// Splits `128MB` into `("128", "MB")`.
fn split_number_unit(value: &str) -> Result<(&str, &str), ConfigError> {
    let split = value.find(|c: char| !(c.is_ascii_digit() || c == '.'));
    match split {
        Some(index) => Ok((&value[..index], value[index..].trim())),
        None => Ok((value, "")),
    }
}

/// Formats bytes back into the unit the engine prefers for display (`128MB`).
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes % GB == 0 && bytes >= GB {
        format!("{}GB", bytes / GB)
    } else if bytes % MB == 0 && bytes >= MB {
        format!("{}MB", bytes / MB)
    } else if bytes % KB == 0 && bytes >= KB {
        format!("{}kB", bytes / KB)
    } else {
        bytes.to_string()
    }
}

/// Validates `value` against `spec`, returning it unchanged on success.
///
/// The check is strict on purpose: a value that would be read as a comment, a
/// second directive or a new directive line is refused here, before any file is
/// touched (see [`super::editor::apply_edit`]).
pub fn validate_value(spec: &SettingSpec, value: &str) -> Result<(), ConfigError> {
    // Reject control characters and a newline for every kind.
    if value.contains('\n') || value.contains('\r') || value.contains('\0') {
        return Err(ConfigError::InvalidValue("nilai berisi karakter kontrol"));
    }
    if is_comment_injection(value) {
        return Err(ConfigError::InvalidValue("nilai berisi komentar"));
    }
    match &spec.kind {
        ValueKind::Integer { min, max } => {
            let parsed: u64 = value
                .trim()
                .parse()
                .map_err(|_| ConfigError::InvalidValue("nilai harus bilangan bulat"))?;
            if parsed < *min || parsed > *max {
                return Err(ConfigError::InvalidValue("nilai di luar rentang"));
            }
        }
        ValueKind::Size { min, max } => {
            let bytes = parse_size(value)?;
            if bytes < *min || bytes > *max {
                return Err(ConfigError::InvalidValue("ukuran di luar rentang"));
            }
        }
        ValueKind::Boolean { on, off } => {
            let lower = value.trim().to_ascii_lowercase();
            let accepted = match (on.as_ref(), off.as_ref()) {
                ("on", "off") => ["on", "off"],
                _ => ["yes", "no"],
            };
            if !accepted.contains(&lower.as_str()) {
                return Err(ConfigError::InvalidValue("nilai harus ya/tidak"));
            }
        }
        ValueKind::Enum { allowed } => {
            if !allowed
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(value.trim()))
            {
                return Err(ConfigError::InvalidValue("nilai tidak ada di daftar"));
            }
        }
        ValueKind::Token { min_len, max_len } => {
            let len = value.chars().count();
            if len < *min_len || len > *max_len {
                return Err(ConfigError::InvalidValue("panjang nilai di luar rentang"));
            }
            // A `save` schedule is pairs of integers; other tokens may not contain
            // whitespace-joined junk that would become extra arguments, but the
            // `save` key legitimately has several numbers. Accept only digits and
            // spaces so an operator cannot inject a second key.
            if !value
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_whitespace())
            {
                return Err(ConfigError::InvalidValue("nilai tidak valid"));
            }
        }
        ValueKind::TimeZone => {
            validate_timezone(value)?;
        }
    }
    Ok(())
}

fn is_comment_injection(value: &str) -> bool {
    let mut previous: Option<char> = None;
    for ch in value.chars() {
        if (ch == '#' || ch == ';' || ch == '=') && previous.is_none_or(char::is_whitespace) {
            return true;
        }
        previous = Some(ch);
    }
    false
}

/// Accepts `UTC`, `GMT`, `Area/Location` and rejects anything else.
fn validate_timezone(value: &str) -> Result<(), ConfigError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ConfigError::InvalidValue("zona waktu kosong"));
    }
    // A MySQL time zone may be an UTC offset (`+07:00`, `-08:00`); accept that
    // form, which is better than a named zone for a server that must not depend
    // on an installed tzdata.
    if let Some(offset) = trimmed.strip_prefix(['+', '-']) {
        return validate_offset(offset)
            .then_some(())
            .ok_or(ConfigError::InvalidValue("offset zona waktu tidak valid"));
    }
    match trimmed.parse::<chrono_tz::Tz>() {
        Ok(_) => Ok(()),
        Err(_) => Err(ConfigError::InvalidValue("zona waktu tidak dikenal")),
    }
}

/// Validates `HH:MM` (or `HH`) for a UTC offset.
fn validate_offset(offset: &str) -> bool {
    let mut parts = offset.split(':');
    let hours = parts.next().and_then(|h| h.parse::<u8>().ok());
    let minutes = match parts.next() {
        Some(m) => m.parse::<u8>().ok(),
        None => Some(0),
    };
    if parts.next().is_some() {
        return false;
    }
    matches!((hours, minutes), (Some(h), Some(m)) if h <= 14 && m < 60)
}

#[cfg(test)]
mod tests;
