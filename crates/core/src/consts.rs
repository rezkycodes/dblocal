//! Every tunable value in one place.
//!
//! Nothing else in the crate hardcodes a port, timeout, size or path fragment.

/// Canonical PostgreSQL port.
pub const PORT_POSTGRESQL: u16 = 5432;
/// Canonical MySQL port.
pub const PORT_MYSQL: u16 = 3306;
/// Canonical MariaDB port.
pub const PORT_MARIADB: u16 = 3307;
/// Canonical Valkey port.
pub const PORT_VALKEY: u16 = 6379;
/// Canonical Redis port (port allocator avoids clashes when Valkey is on 6379).
pub const PORT_REDIS: u16 = 6379;
/// Canonical RustFS S3 API port.
pub const PORT_RUSTFS: u16 = 9000;
/// Canonical RustFS web console port when enabled.
pub const PORT_RUSTFS_CONSOLE: u16 = 9001;

/// Lowest port an instance may use; anything below needs privileges.
pub const MIN_PORT: u16 = 1024;
/// Highest port an instance may use.
pub const MAX_PORT: u16 = 65535;

/// Start timeout for PostgreSQL, MySQL and MariaDB.
pub const START_TIMEOUT_SECS: u64 = 30;
/// Start timeout for Valkey.
pub const START_TIMEOUT_VALKEY_SECS: u64 = 10;
/// Start timeout for Redis.
pub const START_TIMEOUT_REDIS_SECS: u64 = START_TIMEOUT_VALKEY_SECS;
/// Start timeout for RustFS.
pub const START_TIMEOUT_RUSTFS_SECS: u64 = 15;
/// Grace period before a stop escalates to a forced kill.
pub const STOP_GRACE_SECS: u64 = 20;
/// Grace given to the escalation after an engine that accepted its own shutdown
/// request still ran for the whole `STOP_GRACE_SECS`.
pub const STOP_ESCALATION_SECS: u64 = 5;
/// How often the instance supervisor polls process health.
pub const STATUS_POLL_SECS: u64 = 2;
/// How long a cached catalog snapshot stays fresh.
pub const CATALOG_TTL_HOURS: u64 = 24;
/// Largest catalog body accepted from the refresh source, in bytes (4 MiB).
pub const CATALOG_MAX_BODY_BYTES: u64 = 4 * 1024 * 1024;
/// Busy timeout applied to every state-database connection, in milliseconds.
pub const STORE_BUSY_TIMEOUT_MS: u32 = 5_000;

/// Maximum size of a single instance log file before it is rotated.
pub const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// How many rotated log files are kept.
pub const LOG_ROTATE_KEEP: u32 = 3;
/// Number of log lines shown by default after a failed start.
pub const LOG_TAIL_ON_FAILURE: usize = 20;
/// Number of log lines the GUI keeps in memory.
pub const LOG_TAIL_LINES: usize = 1000;

/// Length of a generated superuser password.
pub const PASSWORD_LEN: usize = 24;
/// Length of a generated RustFS S3 access key.
pub const ACCESS_KEY_LEN: usize = 20;

/// Environment variable that relocates the whole state tree.
pub const ENV_DBLOCAL_HOME: &str = "DBLOCAL_HOME";
/// Directory name used inside the user's home.
pub const HOME_DIR_NAME: &str = ".dblocal";
/// File name of the SQLite state database.
pub const STATE_DB_FILE: &str = "state.db";
/// File name of the application log.
pub const APP_LOG_FILE: &str = "app.log";

/// Sub-directory holding installed engine runtimes.
pub const DIR_RUNTIMES: &str = "runtimes";
/// Sub-directory holding in-flight `.part` downloads.
pub const DIR_DOWNLOADS: &str = "downloads";
/// Sub-directory holding instances.
pub const DIR_INSTANCES: &str = "instances";
/// Sub-directory holding archived data directories.
pub const DIR_ARCHIVE: &str = "archive";
/// Sub-directory holding config backups (`F-CFG-01` AC3).
pub const DIR_BACKUPS: &str = "dblocal-backups";
/// Sub-directory holding the credential file fallback.
pub const DIR_SECRETS: &str = "secrets";
/// Per-instance data directory.
pub const DIR_DATA: &str = "data";
/// Per-instance log directory.
pub const DIR_LOGS: &str = "logs";
/// Per-instance run directory (pidfile, socket).
pub const DIR_RUN: &str = "run";

/// Shortest valid instance name.
pub const INSTANCE_NAME_MIN_LEN: usize = 2;
/// Longest valid instance name.
pub const INSTANCE_NAME_MAX_LEN: usize = 31;
/// Instance names must match this shape; documented here and enforced in `model`.
pub const INSTANCE_NAME_PATTERN: &str = "^[a-z0-9][a-z0-9-]{1,30}$";

/// Canonical superuser name for PostgreSQL.
pub const SUPERUSER_POSTGRESQL: &str = "postgres";
/// Canonical superuser name for MySQL and MariaDB.
pub const SUPERUSER_MYSQL: &str = "root";
/// Canonical user name for Valkey.
pub const SUPERUSER_VALKEY: &str = "default";
/// Canonical user name for Redis.
pub const SUPERUSER_REDIS: &str = "default";
/// Placeholder superuser name for RustFS (real instances generate a random access key).
pub const SUPERUSER_RUSTFS: &str = "rustfs";

/// Default database name used in connection URIs.
pub const DEFAULT_DATABASE: &str = "postgres";

/// Capacity of the broadcast channel carrying service events.
pub const SERVICE_EVENT_CHANNEL_CAPACITY: usize = 256;

/// Number of log lines kept in a start-failure diagnosis.
pub const FAILURE_LOG_TAIL_LINES: usize = LOG_TAIL_ON_FAILURE;

/// Poll interval for readiness probes while starting an engine.
pub const HEALTH_CHECK_POLL_MS: u64 = 250;

/// GitHub repository the self-update check queries (`F-PKG-05`).
pub const UPDATE_REPO: &str = "rezkycodes/dblocal";
/// Sub-directory of the home where a downloaded update is staged before apply.
pub const DIR_UPDATE_STAGING: &str = "update";
/// Timeout for the update check request, in seconds.
pub const UPDATE_CHECK_TIMEOUT_SECS: u64 = 15;
/// How often background update checks refresh from GitHub (6 hours).
pub const UPDATE_CHECK_INTERVAL_SECS: u64 = 6 * 60 * 60;
/// SQLite `settings` key for whether automatic update checks are enabled.
pub const SETTING_AUTO_UPDATE_CHECK: &str = "update.auto_check";
/// SQLite `settings` key for the Unix timestamp of the last update check.
pub const SETTING_UPDATE_LAST_CHECKED_AT: &str = "update.last_checked_at";
/// SQLite `settings` key for the cached JSON of the last update check result.
pub const SETTING_UPDATE_CACHED_RELEASE: &str = "update.cached_release";
/// Lock file name inside [`DIR_UPDATE_STAGING`] preventing concurrent updates.
pub const UPDATE_LOCK_FILE: &str = "update.lock";
/// Hidden CLI flag used by the detached Windows update helper after exit.
pub const APPLY_UPDATE_FLAG: &str = "--apply-update";
/// Connect timeout for admin operations against a running instance, in seconds.
pub const ADMIN_CONNECT_TIMEOUT_SECS: u64 = 5;
/// Statement timeout for admin operations, in seconds.
pub const ADMIN_STATEMENT_TIMEOUT_SECS: u64 = 30;
