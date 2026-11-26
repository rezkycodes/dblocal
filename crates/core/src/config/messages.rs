//! User-facing messages for the config editor (`DBL-86xx`).
//!
//! Every string is static, so a message can never scream a value the user typed
//! (which could be a password for `requirepass`, though that key is not editable
//! here). Descriptions name the setting's purpose, never its current value.

/// `DBL-8600`: the key is not editable for this engine.
pub const ERR_UNKNOWN_KEY: &str =
    "That setting is not in the list that can be changed for this engine.";
/// `DBL-8602`: the key is managed by DBLocal.
pub const ERR_MANAGED_KEY: &str =
    "That setting is managed by DBLocal itself. Use the matching action instead (for example, change the port).";
/// `DBL-8603`: the file could not be read or written.
pub const ERR_IO: &str = "The configuration file could not be read or written.";
/// `DBL-8604`: a lifecycle operation owns the instance.
pub const ERR_BUSY: &str = "The instance is busy with another operation. Try again when it finishes.";
/// `DBL-8605`: nothing to restore.
pub const ERR_NO_BACKUP: &str = "There is no configuration backup to restore yet.";
/// `DBL-8606`: the whole call was refused.
pub const ERR_REFUSED: &str =
    "All changes were rejected because one setting is not allowed.";
/// `DBL-8607`: the config file to open does not exist.
pub const ERR_MISSING_FILE: &str =
    "The configuration file does not exist yet. Start the instance once so the file is created.";
/// `DBL-8608`: the OS handler could not be started.
pub const ERR_LAUNCHER: &str =
    "The system's default editor could not be opened. Open the file manually.";

// --- One-line descriptions for the editable settings -------------------------

/// PostgreSQL `max_connections`.
pub const DESC_MAX_CONNECTIONS_PG: &str =
    "Maximum number of client connections the server accepts at once.";
/// MySQL `max_connections`.
pub const DESC_MAX_CONNECTIONS_MYSQL: &str =
    "Maximum number of client connections the server accepts at once.";
/// MariaDB `max_connections`.
pub const DESC_MAX_CONNECTIONS_MARIADB: &str =
    "Maximum number of client connections the server accepts at once.";
/// PostgreSQL `shared_buffers`.
pub const DESC_SHARED_BUFFERS: &str =
    "Shared memory for the page cache; about 25% of RAM is a common starting point.";
/// PostgreSQL `work_mem`.
pub const DESC_WORK_MEM: &str =
    "Memory limit per sort or hash operation before it uses temporary files.";
/// PostgreSQL `timezone`.
pub const DESC_TIMEZONE_PG: &str = "Time zone the server uses when it displays times.";
/// MySQL `default-time-zone`.
pub const DESC_TIME_ZONE_MYSQL: &str =
    "The server's default time zone; empty means the system time zone.";
/// MariaDB `default-time-zone`.
pub const DESC_TIME_ZONE_MARIADB: &str =
    "The server's default time zone; empty means the system time zone.";
/// MySQL/MariaDB `innodb_buffer_pool_size`.
pub const DESC_INNODB_BUFFER_POOL: &str =
    "Cache for InnoDB data and indexes; the largest part of database memory.";
/// MySQL `sort_buffer_size`.
pub const DESC_SORT_BUFFER_MYSQL: &str =
    "Memory per sort operation in MySQL; the value applies per connection.";
/// MariaDB `sort_buffer_size`.
pub const DESC_SORT_BUFFER_MARIADB: &str =
    "Memory per sort operation in MariaDB; the value applies per connection.";
/// MySQL/MariaDB `character-set-server`.
pub const DESC_CHARACTER_SET: &str = "The default character set for new data and connections.";
/// Valkey `maxmemory`.
pub const DESC_MAXMEMORY: &str = "Memory limit for Valkey; 0 means no limit.";
/// Valkey `maxmemory-policy`.
pub const DESC_MAXMEMORY_POLICY: &str = "How Valkey evicts keys when memory reaches the limit.";
/// Valkey `appendonly`.
pub const DESC_APPENDONLY: &str =
    "Writes every change to an append-only file so data survives a restart.";
/// Valkey `save`.
pub const DESC_SAVE: &str =
    "RDB snapshot schedule (pairs of seconds and number of changes); empty means disabled.";
/// Valkey `timeout`.
pub const DESC_TIMEOUT: &str =
    "Closes a client connection that idles longer than this many seconds; 0 means never.";

// --- CLI strings -------------------------------------------------------------

/// Shown after a successful `config set`.
pub const MSG_SAVED: &str = "Configuration saved. Changes take effect after a restart.";
/// Shown when a backup path is printed.
pub const MSG_BACKUP_AT: &str = "Copy from before the change:";
/// Shown when a restore succeeded.
pub const MSG_RESTORED: &str = "Configuration restored from the newest backup.";
/// Column header for the settings table.
pub const HEADER_SETTING: &str = "SETTING";
/// Column header for the current value.
pub const HEADER_CURRENT: &str = "CURRENT";
/// Column header for the default.
pub const HEADER_DEFAULT: &str = "DEFAULT";
/// Column header for the unit.
pub const HEADER_UNIT: &str = "UNIT";
/// Column header for restart-required.
pub const HEADER_RESTART: &str = "RESTART";
/// The word shown for "yes" in the restart column.
pub const YES: &str = "yes";
/// The word shown for "default" when a setting is unset in the file.
pub const DEFAULT_LABEL: &str = "default";
/// The word shown for a read-only row.
pub const READONLY_LABEL: &str = "read-only";
/// Prompt for a `key=value` pair the CLI cannot parse.
pub const ERR_SET_FORMAT: &str = "The format must be key=value.";
/// Unit for a byte-size setting: what the user actually types.
pub const UNIT_SIZE: &str = "size (MB/GB)";
/// Unit for a duration in seconds.
pub const UNIT_SECONDS: &str = "seconds";
/// Shown after a successful `config open`.
pub const MSG_OPENED: &str = "Configuration file opened in the system's default editor.";
