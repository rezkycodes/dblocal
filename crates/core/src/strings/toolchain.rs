//! User-facing strings for the Toolchains GUI (v0.3.0).

/// Sidebar section heading for toolchains.
pub const UI_DOCK_TOOLCHAINS: &str = "TOOLCHAINS";
/// Title-bar breadcrumb section name for a toolchain page.
pub const UI_TOOLCHAIN_BREADCRUMB: &str = "Toolchains";

/// Toolchain header pill when no version is active.
pub const UI_TOOLCHAIN_NO_ACTIVE: &str = "No active version";
/// Toolchain header refresh button.
pub const UI_TOOLCHAIN_REFRESH: &str = "Refresh";

/// Composer banner title when no PHP version is active.
pub const UI_TOOLCHAIN_COMPOSER_NEEDS_PHP_TITLE: &str = "Composer requires an active PHP version";
/// Composer banner body when no PHP version is active.
pub const UI_TOOLCHAIN_COMPOSER_NEEDS_PHP_BODY: &str = "The composer shim runs composer.phar through your active PHP shim. \
     Install and activate a PHP version so composer can execute.";
/// Composer banner shortcut button.
pub const UI_TOOLCHAIN_GO_TO_PHP: &str = "Go to PHP";

/// Offline / empty catalog banner title.
pub const UI_TOOLCHAIN_CATALOG_UNAVAILABLE: &str = "Catalog unavailable";
/// Retry button label.
pub const UI_TOOLCHAIN_RETRY: &str = "Retry";

/// PHP extension profile section heading.
pub const UI_TOOLCHAIN_EXTENSION_PROFILE: &str = "Extension profile";
/// Linux PHP Common profile button.
pub const UI_TOOLCHAIN_PROFILE_COMMON: &str = "Common (default)";
/// Linux PHP Bulk profile button.
pub const UI_TOOLCHAIN_PROFILE_BULK: &str = "Bulk";

/// Versions section heading.
pub const UI_TOOLCHAIN_VERSIONS: &str = "Versions";
/// Empty versions fallback text.
pub const UI_TOOLCHAIN_NO_VERSIONS: &str = "No versions available for this platform.";
/// Version status pill: active.
pub const UI_TOOLCHAIN_STATUS_ACTIVE: &str = "Active";
/// Version status pill: installed.
pub const UI_TOOLCHAIN_STATUS_INSTALLED: &str = "Installed";
/// Version status pill: available.
pub const UI_TOOLCHAIN_STATUS_AVAILABLE: &str = "Available";
/// Node.js LTS chip label.
pub const UI_TOOLCHAIN_LTS: &str = "LTS";
/// PHP 8.0 common missing pdo_pgsql note.
pub const UI_TOOLCHAIN_NO_PDO_PGSQL: &str = "· pdo_pgsql not included in this build";

/// Row action: install.
pub const UI_TOOLCHAIN_INSTALL: &str = "Install";
/// Row action: update.
pub const UI_TOOLCHAIN_UPDATE: &str = "Update";
/// Row action: make active.
pub const UI_TOOLCHAIN_MAKE_ACTIVE: &str = "Make active";
/// Row action tooltip: remove installed version.
pub const UI_TOOLCHAIN_REMOVE_VERSION: &str = "Remove version";
/// Row action tooltip: remove active version.
pub const UI_TOOLCHAIN_REMOVE_ACTIVE: &str = "Remove active version";

/// PATH panel section heading.
pub const UI_TOOLCHAIN_SHELL_PATH: &str = "Shell PATH";
/// PATH panel row title.
pub const UI_TOOLCHAIN_SHIMS_DIR: &str = "Shims directory";
/// PATH panel status chip: applied and first.
pub const UI_TOOLCHAIN_PATH_ON_FIRST: &str = "On PATH (first)";
/// PATH panel status chip: not on PATH.
pub const UI_TOOLCHAIN_PATH_NOT_ON: &str = "Not on PATH";
/// PATH panel status chip: needs attention.
pub const UI_TOOLCHAIN_PATH_ATTENTION: &str = "Needs attention";
/// PATH panel button: add to PATH.
pub const UI_TOOLCHAIN_PATH_ADD: &str = "Add to PATH";
/// PATH panel button: remove from PATH.
pub const UI_TOOLCHAIN_PATH_REMOVE: &str = "Remove from PATH";

/// PHP settings section heading.
pub const UI_TOOLCHAIN_PHP_SETTINGS: &str = "PHP settings";
/// PHP settings disabled note when no PHP version is active.
pub const UI_TOOLCHAIN_PHP_SETTINGS_INACTIVE: &str = "Activate a PHP version to edit PHP settings.";
/// PHP settings footer button: open managed conf.d folder.
pub const UI_TOOLCHAIN_PHP_OPEN_FOLDER: &str = "Open config folder";
/// PHP settings footer button: reset to defaults.
pub const UI_TOOLCHAIN_PHP_RESET: &str = "Reset to defaults";
/// PHP settings footer button: save.
pub const UI_TOOLCHAIN_PHP_SAVE: &str = "Save";

/// Remove toolchain version dialog title.
pub const UI_TOOLCHAIN_REMOVE_TITLE: &str = "Remove toolchain version";
/// Remove dialog disk space label.
pub const UI_TOOLCHAIN_REMOVE_SPACE: &str = "Disk space to free:";
/// Remove dialog confirm button.
pub const UI_TOOLCHAIN_REMOVE_CONFIRM: &str = "Remove";

/// Add-to-PATH consent dialog title.
pub const UI_TOOLCHAIN_PATH_CONSENT_TITLE: &str = "Add DBLocal shims to PATH";
/// Add-to-PATH consent dialog subtitle.
pub const UI_TOOLCHAIN_PATH_CONSENT_SUBTITLE: &str =
    "Review the exact changes before DBLocal edits your shell files";
/// Add-to-PATH consent dialog description.
pub const UI_TOOLCHAIN_PATH_CONSENT_DESC: &str = "DBLocal will prepend its shims directory to your PATH so php, composer, \
     node, npm and npx resolve from any new terminal. A .dblocal-backup copy \
     of each file is written first, and you can undo this cleanly at any time.";
/// Add-to-PATH consent dialog files list heading.
pub const UI_TOOLCHAIN_PATH_CONSENT_FILES: &str = "Files that will be updated";
/// Add-to-PATH consent dialog empty rc files fallback.
pub const UI_TOOLCHAIN_PATH_CONSENT_NO_FILES: &str =
    "No existing shell startup files were detected in your home directory.";
/// Add-to-PATH consent dialog preview heading.
pub const UI_TOOLCHAIN_PATH_CONSENT_LINES: &str = "Exact lines to append";
/// Add-to-PATH consent dialog confirm button.
pub const UI_TOOLCHAIN_PATH_CONSENT_CONFIRM: &str = "Confirm and add to PATH";
