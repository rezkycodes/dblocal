//! Every tunable value the toolchain module needs, in one place.
//!
//! Nothing else in `toolchain` hardcodes a timeout, a size or a path fragment.
//! Paths derived from [`crate::paths::DblPaths`] live there; values that are not
//! paths live here.

/// Schema version of `catalog/toolchains-v1.json` this build understands.
pub const CATALOG_SCHEMA_VERSION: u32 = 1;

/// File name of the active-version registry inside `toolchains/`.
pub const ACTIVE_REGISTRY_FILE: &str = "active.json";

/// Schema version of [`ACTIVE_REGISTRY_FILE`].
pub const ACTIVE_REGISTRY_SCHEMA_VERSION: u32 = 1;

/// Per-install marker written next to the payload; the install scan trusts it.
pub const INSTALL_MANIFEST_FILE: &str = "dblocal-install.json";

/// Sub-directory of `toolchains/` holding in-flight downloads and staging dirs.
pub const TMP_DIR_NAME: &str = ".tmp";

/// Sub-directory of `toolchains/php/` holding the managed ini fragments.
pub const PHP_CONF_DIR_NAME: &str = "conf.d";

/// File name of the DBLocal-managed PHP ini fragment.
pub const PHP_MANAGED_INI_FILE: &str = "99-dblocal.ini";

/// File name of the DBLocal-managed base `php.ini` (Windows only).
pub const PHP_BASE_INI_FILE: &str = "php.ini";

/// Directory name of the shims inside the data dir.
pub const SHIMS_DIR_NAME: &str = "bin";

/// File name of the refreshed toolchain catalog cache.
pub const CATALOG_CACHE_FILE: &str = "toolchains.json";

/// How long a refreshed toolchain catalog stays fresh.
pub const CATALOG_TTL_HOURS: u64 = 24;

/// Timeout for one toolchain catalog refresh request.
pub const CATALOG_REFRESH_TIMEOUT_SECS: u64 = 15;

/// Largest toolchain catalog body accepted from the refresh source (2 MiB).
pub const CATALOG_MAX_BODY_BYTES: u64 = 2 * 1024 * 1024;

/// Largest toolchain artifact download accepted, in bytes (2 GiB).
pub const MAX_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// TCP connect timeout for single-file (PHAR) downloads, matching the engine downloader.
pub const PHAR_CONNECT_TIMEOUT_SECS: u64 = 15;

/// Overall request timeout for single-file (PHAR) downloads.
pub const PHAR_OVERALL_TIMEOUT_SECS: u64 = 120;

/// Maximum number of HTTPS redirects followed for single-file (PHAR) downloads.
pub const PHAR_MAX_REDIRECTS: usize = 5;

/// How long an install waits for another operation on the same tool/version.
pub const LOCK_WAIT_SECS: u64 = 30;

/// Poll interval while waiting for the toolchain lock.
pub const LOCK_POLL_MS: u64 = 100;

/// Marker line every generated shim carries so it can be identified safely.
pub const SHIM_MARKER: &str = "# DBLocal shim";
pub const SHIM_MARKER_CMD: &str = "rem DBLocal managed shim";

/// Environment variable a debug build may honour to reach a loopback fixture
/// server during the documented local end-to-end check. Release builds ignore it.
pub const ENV_ALLOW_LOOPBACK_HTTP: &str = "DBLOCAL_ALLOW_LOOPBACK_HTTP";

/// Begin marker of the managed PATH block in a POSIX shell rc file.
pub const PATH_BLOCK_BEGIN: &str = "# >>> dblocal toolchains >>>";

/// End marker of the managed PATH block in a POSIX shell rc file.
pub const PATH_BLOCK_END: &str = "# <<< dblocal toolchains <<<";

/// Version of the PATH block, so a rewritten block is recognisable.
pub const PATH_BLOCK_TAG: &str = "dblocal toolchains";

/// Suffix of the backup file written before an rc file is edited.
pub const RC_BACKUP_SUFFIX: &str = ".dblocal-backup";

/// Default PHP `memory_limit`.
pub const PHP_DEFAULT_MEMORY_LIMIT: &str = "512M";
/// Default PHP `upload_max_filesize`.
pub const PHP_DEFAULT_UPLOAD_MAX_FILESIZE: &str = "64M";
/// Default PHP `post_max_size`.
pub const PHP_DEFAULT_POST_MAX_SIZE: &str = "64M";
/// Default PHP `max_execution_time`, in seconds.
pub const PHP_DEFAULT_MAX_EXECUTION_TIME: i64 = 120;
/// Default PHP `date.timezone`.
pub const PHP_DEFAULT_DATE_TIMEZONE: &str = "UTC";
/// Default PHP `display_errors`.
pub const PHP_DEFAULT_DISPLAY_ERRORS: bool = true;

/// Default PHP extensions enabled in the managed base `php.ini` on Windows.
pub const PHP_WINDOWS_DEFAULT_EXTENSIONS: &[&str] = &[
    "curl",
    "fileinfo",
    "intl",
    "mbstring",
    "opcache",
    "openssl",
    "pdo_mysql",
    "pdo_pgsql",
    "pdo_sqlite",
    "sodium",
    "zip",
];
