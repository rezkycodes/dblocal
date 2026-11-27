//! User-facing English strings.
//!
//! Every string the user can see lives here. Workers append to this module;
//! nothing else in the crate hardcodes a user-facing sentence.

/// Environment variable that relocates the whole `~/.dblocal` tree.
pub const ENV_DBLOCAL_HOME: &str = "DBLOCAL_HOME";

// --- Error messages, one per `DblError` variant -------------------------------

/// `DblError::InvalidInput`.
pub const ERR_INVALID_INPUT: &str = "That value is not valid. Check the highlighted field.";
/// `DblError::NotFound`.
pub const ERR_NOT_FOUND: &str = "The requested item was not found.";
/// `DblError::AlreadyExists`.
pub const ERR_ALREADY_EXISTS: &str = "An item with that name already exists.";
/// `DblError::InvalidTransition`.
pub const ERR_INVALID_TRANSITION: &str =
    "That action is not available in the instance's current status.";
/// `DblError::PortInUse`.
pub const ERR_PORT_IN_USE: &str = "That port is already used by another process.";
/// `DblError::ChecksumMismatch`.
pub const ERR_CHECKSUM_MISMATCH: &str =
    "The download is corrupt or does not match its checksum. Try downloading again.";
/// `DblError::UnsafeArchive`.
pub const ERR_UNSAFE_ARCHIVE: &str = "The archive was rejected because it contains an unsafe path.";
/// `DblError::Io`.
pub const ERR_IO: &str = "Could not read or write the file.";
/// `DblError::Network`.
pub const ERR_NETWORK: &str = "The network connection failed. Check your connection and try again.";
/// `DblError::Storage`.
pub const ERR_STORAGE: &str = "Could not access the local DBLocal database.";
/// `DblError::Engine`.
pub const ERR_ENGINE: &str = "The database engine failed to run. See the log for details.";
/// `DblError::Secret`.
pub const ERR_SECRET: &str = "Could not save or read the credentials.";
/// `DblError::Unsupported`.
pub const ERR_UNSUPPORTED: &str = "This feature is not supported on this platform yet.";
/// `DblError::Cancelled`.
pub const ERR_CANCELLED: &str = "The action was cancelled.";
/// `DblError::NotImplemented`.
pub const ERR_NOT_IMPLEMENTED: &str = "This feature is not available in this version yet.";
/// [`crate::DblError::RuntimeInUse`].
pub const ERR_RUNTIME_IN_USE: &str =
    "This engine runtime is still used by an instance. Delete the instances that use it first.";
/// [`crate::DblError::RuntimeBusy`].
pub const ERR_RUNTIME_BUSY: &str = "This engine runtime is being installed or removed by another operation. Try again when it finishes.";
/// [`crate::DblError::UnsafeRuntimePath`].
pub const ERR_UNSAFE_RUNTIME_PATH: &str = "This engine runtime points outside its DBLocal folder or through a link, so nothing was removed. Remove it by hand.";
/// [`crate::DblError::RuntimeRemoval`].
pub const ERR_RUNTIME_REMOVAL: &str =
    "Could not remove the engine runtime. Close any program that uses its files, then try again.";

// --- Catalog status banner ---------------------------------------------------

/// Shown when the cached catalog is older than its TTL.
pub const CATALOG_STALE: &str = "The catalog may be out of date";
/// Shown when the catalog could not be refreshed at all.
pub const CATALOG_OFFLINE: &str = "Catalog offline, using the last known data";

/// Shown when an engine has no installable artifacts for the current platform.
pub const CATALOG_ENGINE_UNAVAILABLE_PLATFORM: &str = "Not available on this platform.";

// --- Instance and credential strings ----------------------------------------

/// Badge for an instance created without a password.
pub const BADGE_NO_AUTH: &str = "No auth";
/// Note shown when credentials live in a file instead of the OS keychain.
pub const CREDENTIALS_IN_FILE: &str = "Stored in a file, not the keychain";
/// Delete action that removes the instance and its data.
pub const DELETE_PURGE: &str = "Delete instance and data";
/// Delete action that keeps an archive copy of the data.
pub const DELETE_ARCHIVE: &str = "Delete instance, keep data";
/// Diagnostics hint when the data dir was written by another engine version.
pub const HINT_VERSION_MISMATCH: &str = "Version mismatch, a new instance is required";
/// Diagnostics hint when the runtime binary is missing or corrupt.
pub const HINT_REINSTALL_RUNTIME: &str = "Reinstall the runtime";

// --- UI chrome ---------------------------------------------------------------

/// App name shown in the title bar.
pub const APP_NAME: &str = "DBLocal";
/// Title bar button to create a new instance.
pub const UI_NEW_INSTANCE: &str = "New instance";
/// Welcome heading when no tab is open.
pub const UI_WELCOME_TITLE: &str = "Welcome to DBLocal";
/// Welcome subtitle.
pub const UI_WELCOME_SUBTITLE: &str = "Create your first local database instance to get started.";
/// Call to action on the welcome screen.
pub const UI_CREATE_FIRST: &str = "Create your first instance";
/// Shown when instances exist but no tab is open.
pub const UI_SELECT_INSTANCE_TITLE: &str = "Select an instance";
/// Subtitle for the select-instance empty state.
pub const UI_SELECT_INSTANCE_SUBTITLE: &str =
    "Select an instance in the left panel to see its details.";
/// Launcher card: heading above the engine quick-create grid.
pub const UI_LAUNCHER_QUICK_CREATE: &str = "Quick create";
/// Launcher card: heading above the instance shortcuts.
pub const UI_LAUNCHER_INSTANCES: &str = "Open an instance";
/// Launcher card: the command palette row.
pub const UI_LAUNCHER_PALETTE: &str = "Command palette";
/// Launcher card: footer hint under the card.
pub const UI_LAUNCHER_FOOTER: &str =
    "Every runtime is verified with SHA-256 and listens on loopback only.";
/// Title bar: the command search button.
pub const UI_TITLE_SEARCH: &str = "Search instances and commands";
/// Overview stat strip: version tile.
pub const UI_STAT_VERSION: &str = "Version";
/// Overview stat strip: status tile.
pub const UI_STAT_STATUS: &str = "Status";
/// Settings dialog subtitle.
pub const UI_SETTINGS_SUBTITLE: &str = "Updates and how DBLocal starts with your session";
/// Instances panel header.
pub const UI_INSTANCES: &str = "INSTANCES";
/// Instances panel footer button.
pub const UI_NEW_INSTANCE_FOOTER: &str = "New instance";
/// Overview tab label.
pub const UI_TAB_OVERVIEW: &str = "Overview";
/// Logs tab label.
pub const UI_TAB_LOGS: &str = "Logs";
/// Status bar: running count label fragment.
pub const UI_RUNNING: &str = "running";
/// Status bar: total label fragment.
pub const UI_TOTAL: &str = "total";
/// Status bar: catalog label.
pub const UI_CATALOG: &str = "Catalog";
/// Status bar: downloads label.
pub const UI_DOWNLOADS: &str = "Downloads";
/// Status bar: no active downloads.
pub const UI_NO_DOWNLOADS: &str = "No active downloads";
/// Status bar: tray unavailable note.
pub const UI_TRAY_UNAVAILABLE: &str = "Tray unavailable";
/// Status bar: tray available note.
pub const UI_TRAY_ACTIVE: &str = "Tray active";
/// Version prefix.
pub const UI_VERSION_PREFIX: &str = "v";
/// Command palette placeholder.
pub const UI_PALETTE_PLACEHOLDER: &str = "Search instances or run a command...";
/// Generic close label.
pub const UI_CLOSE: &str = "Close";
/// Generic cancel label.
pub const UI_CANCEL: &str = "Cancel";
/// Generic confirm label.
pub const UI_CONFIRM: &str = "Confirm";

// --- Instance actions -------------------------------------------------------

/// Start action.
pub const UI_START: &str = "Start";
/// Stop action.
pub const UI_STOP: &str = "Stop";
/// Copy URI action.
pub const UI_COPY_URI: &str = "Copy URI";
/// Copy .env action.
pub const UI_COPY_DOTENV: &str = "Copy .env";
/// Delete action label.
pub const UI_DELETE: &str = "Delete";
/// Retry action.
pub const UI_RETRY: &str = "Retry";
/// Change port action.
pub const UI_CHANGE_PORT: &str = "Change port";
/// Reveal password toggle.
pub const UI_SHOW_PASSWORD: &str = "Show password";
/// Hide password toggle.
pub const UI_HIDE_PASSWORD: &str = "Hide password";

// --- Toasts -----------------------------------------------------------------

/// Toast shown after a URI is copied.
pub const UI_TOAST_URI_COPIED: &str = "URI copied";
/// Toast shown after a .env block is copied.
pub const UI_TOAST_DOTENV_COPIED: &str = ".env copied";
/// Toast shown after log lines are copied.
pub const UI_TOAST_LOGS_COPIED: &str = "Logs copied";

// --- Instance status labels (shown with the status dot) ---------------------

/// Status label for a running instance.
pub const STATUS_RUNNING: &str = "running";
/// Status label for a stopped instance.
pub const STATUS_STOPPED: &str = "stopped";
/// Status label for a failed instance.
pub const STATUS_FAILED: &str = "failed";
/// Status label while downloading/provisioning.
pub const STATUS_PROVISIONING: &str = "downloading";
/// Status label while initializing.
pub const STATUS_INITIALIZING: &str = "initializing";
/// Status label while starting.
pub const STATUS_STARTING: &str = "starting";
/// Status label while stopping.
pub const STATUS_STOPPING: &str = "stopping";
/// Status label while being deleted.
pub const STATUS_DELETING: &str = "deleting";
/// Status label for a draft instance.
pub const STATUS_DRAFT: &str = "draft";

// --- Create instance dialog -------------------------------------------------

/// Dialog title for creating an instance.
pub const UI_CREATE_TITLE: &str = "New instance";
/// Label for the engine field.
pub const UI_FIELD_ENGINE: &str = "Engine";
/// Label for the version field.
pub const UI_FIELD_VERSION: &str = "Version";
/// Label for the name field.
pub const UI_FIELD_NAME: &str = "Name";
/// Label for the port field.
pub const UI_FIELD_PORT: &str = "Port";
/// Label for the auth mode field.
pub const UI_FIELD_AUTH: &str = "Authentication";
/// Auth mode: password (default).
pub const UI_AUTH_PASSWORD: &str = "Password (recommended)";
/// Auth mode: no password.
pub const UI_AUTH_NONE: &str = "No password";
/// Warning shown when "no password" auth is selected.
pub const UI_AUTH_NONE_WARNING: &str =
    "Other processes and users on this machine can connect without credentials.";
/// Create button label.
pub const UI_CREATE_SUBMIT: &str = "Create instance";
/// Version badge: already installed locally.
pub const UI_VERSION_INSTALLED: &str = "Installed";
/// Version badge: end of life.
pub const UI_VERSION_EOL: &str = "EOL";
/// Version badge: long-term support line.
pub const UI_VERSION_LTS: &str = "LTS";
/// Version badge: MySQL Innovation release.
pub const UI_VERSION_INNOVATION: &str = "Innovation";
/// Version badge: MariaDB rolling stable release.
pub const UI_VERSION_ROLLING: &str = "Rolling";
/// Create dialog: the "latest only" version-list checkbox.
pub const UI_CREATE_LATEST_ONLY: &str = "Only show latest versions";

// --- Delete dialog (D-18) ---------------------------------------------------

/// Delete dialog title.
pub const UI_DELETE_TITLE: &str = "Delete instance";
/// Delete dialog prompt showing the instance name.
pub const UI_DELETE_PROMPT: &str = "Choose what happens to this instance's data.";
/// Data size label in the delete dialog.
pub const UI_DELETE_SIZE: &str = "Data size";
/// Instruction to type the name to confirm purge.
pub const UI_DELETE_TYPE_NAME: &str = "Type the instance name to confirm deleting the data.";
/// Placeholder for the name confirmation field.
pub const UI_DELETE_NAME_PLACEHOLDER: &str = "Instance name";

// --- Failure / diagnostics (F-LOG-02) ---------------------------------------

/// Heading shown in a failure state.
pub const UI_FAILURE_HEADING: &str = "Instance failed to start";
/// Label for the stable error code.
pub const UI_FAILURE_CODE: &str = "Code";
/// Label for the diagnosis section.
pub const UI_FAILURE_DIAGNOSIS: &str = "Diagnosis";
/// Label for the recent log tail.
pub const UI_FAILURE_LOG_TAIL: &str = "Recent log";

// --- Logs -------------------------------------------------------------------

/// Follow toggle label when live.
pub const UI_LOGS_FOLLOWING: &str = "Following";
/// Follow toggle label when paused.
pub const UI_LOGS_PAUSED: &str = "Paused";
/// Copy logs action.
pub const UI_LOGS_COPY: &str = "Copy logs";
/// Empty logs placeholder.
pub const UI_LOGS_EMPTY: &str = "No logs yet.";

// --- Overview ---------------------------------------------------------------

/// Host label.
pub const UI_OVERVIEW_HOST: &str = "Host";
/// Port label.
pub const UI_OVERVIEW_PORT: &str = "Port";
/// Version label.
pub const UI_OVERVIEW_VERSION: &str = "Version";
/// URI label.
pub const UI_OVERVIEW_URI: &str = "URI";
/// Status label.
pub const UI_OVERVIEW_STATUS: &str = "Status";

// --- Palette actions --------------------------------------------------------

/// Palette: create a new instance.
pub const UI_PALETTE_NEW: &str = "New instance";
/// Palette: start an instance (prefix).
pub const UI_PALETTE_START: &str = "Start";
/// Palette: stop an instance (prefix).
pub const UI_PALETTE_STOP: &str = "Stop";
/// Palette: copy URI for an instance (prefix).
pub const UI_PALETTE_COPY_URI: &str = "Copy URI";
/// Palette: open an instance tab (prefix).
pub const UI_PALETTE_OPEN: &str = "Open";
/// Palette: delete an instance (prefix), opening the delete dialog (`F-INS-05`).
pub const UI_PALETTE_DELETE: &str = "Delete";
/// Palette: why "Start" cannot run (the instance is already running).
pub const UI_PALETTE_REASON_ALREADY_RUNNING: &str = "already running";
/// Palette: why "Stop" cannot run (the instance is not running).
pub const UI_PALETTE_REASON_NOT_RUNNING: &str = "not running";
/// Palette: why an action cannot run during a transition.
pub const UI_PALETTE_REASON_BUSY: &str = "busy, wait for it to finish";

// --- Validation -------------------------------------------------------------

/// Validation: the chosen port is below the allowed minimum.
pub const UI_VALIDATE_PORT_LOW: &str =
    "Ports below 1024 require privileges; DBLocal never asks for sudo.";
/// Validation: instance name is invalid.
pub const UI_VALIDATE_NAME: &str = "The name must be 2-31 characters: lowercase letters, digits or hyphens, starting with a letter or digit.";

/// Validation: the typed port already belongs to another instance.
///
/// `{port}` and `{name}` are placeholders the GUI fills in.
pub const UI_VALIDATE_PORT_TAKEN: &str = "Port {port} is already used by instance {name}";

/// Formats [`UI_VALIDATE_PORT_TAKEN`] without adding a second copy of the
/// sentence to the GUI.
pub fn validate_port_taken(port: u16, name: &str) -> String {
    UI_VALIDATE_PORT_TAKEN
        .replace("{port}", &port.to_string())
        .replace("{name}", name)
}

// --- Vulkan pre-flight (terminal, before any window exists) -------------------

/// Vulkan pre-flight headline.
pub const VULKAN_HEADLINE: &str =
    "DBLocal cannot start because Vulkan is not available on this system.";
/// Vulkan pre-flight detected-reason prefix.
pub const VULKAN_DETECTED: &str = "Detected";
/// Vulkan pre-flight explanation.
pub const VULKAN_WHY: &str =
    "Reason: the DBLocal interface uses GPUI, which requires a Vulkan driver/ICD.";
/// Vulkan pre-flight fix heading.
pub const VULKAN_FIX_HEADING: &str = "How to fix it (install a Vulkan driver for your GPU):";
/// Vulkan pre-flight verification heading.
pub const VULKAN_VERIFY: &str = "Verify after installing";
/// Vulkan pre-flight help link prefix.
pub const VULKAN_MORE_HELP: &str = "More help";
/// Vulkan pre-flight closing hint.
pub const VULKAN_HINT: &str = "If the driver is installed but DBLocal still fails, make sure an ICD JSON file exists in \
     /usr/share/vulkan/icd.d and that VK_ICD_FILENAMES/VK_DRIVER_FILES do not point at a wrong path.";
/// Vulkan pre-flight exit-code line prefix.
pub const VULKAN_EXIT_CODE: &str = "Exit code";

// --- Human descriptions for machine slugs shown in the UI ---------------------

/// Catalog freshness: fresh.
///
/// Every freshness word names the catalog explicitly: the title bar also
/// carries the app-update icon, so a bare "Up to date" reads as a claim about
/// DBLocal itself (`F-PKG-05`).
pub const CATALOG_FRESH_LABEL: &str = "Catalog up to date";
/// Catalog freshness: stale.
pub const CATALOG_STALE_LABEL: &str = "Catalog outdated";
/// Catalog freshness: offline.
pub const CATALOG_OFFLINE_LABEL: &str = "Catalog offline";

/// Start diagnosis: the port was already bound.
pub const DIAGNOSIS_PORT_IN_USE: &str = "The port is already used by another process";
/// Start diagnosis: the data directory belongs to another engine version.
pub const DIAGNOSIS_VERSION_MISMATCH: &str = "Version mismatch, a new instance is required";
/// Start diagnosis: a path could not be read or written.
pub const DIAGNOSIS_PERMISSION_DENIED: &str = "Permission denied on the data path";
/// Start diagnosis: the engine binary is missing or corrupt.
pub const DIAGNOSIS_BINARY_MISSING: &str = "The engine binary is missing or corrupt";
/// Start diagnosis: nothing the app recognises (see the log).
pub const DIAGNOSIS_UNKNOWN: &str = "Unrecognized cause, see the log";

/// Toast shown when a stop had to escalate to a forced kill (F-INS-03 AC3):
/// the engine ignored the graceful shutdown within the grace period.
pub const UI_TOAST_STOP_FORCED: &str =
    "The instance ignored a normal stop, so it was force-stopped";

// --- Workspace chrome (D-25 prototype port) ---------------------------------

/// Breadcrumb separator between the engine and the instance name.
pub const UI_BREADCRUMB_SEPARATOR: &str = "/";
/// Dock section header above the engine groups.
pub const UI_DOCK_INSTANCES: &str = "INSTANCES";
/// Dock section header above the installed runtimes.
pub const UI_DOCK_RUNTIMES: &str = "RUNTIMES";
/// Filter input placeholder in the dock.
pub const UI_DOCK_FILTER: &str = "Filter…";
/// Placeholder when the dock filter matches nothing.
pub const UI_DOCK_NO_MATCHES: &str = "No matching instances.";
/// Runtimes list: this runtime is referenced by no instance (F-RTM-04).
pub const UI_RUNTIME_UNUSED: &str = "unused";
/// Runtimes list: how many instances share a runtime.
pub const UI_RUNTIME_INSTANCE_COUNT: &str = "inst.";
/// Section heading on the Overview tab for the connection card.
pub const UI_OVERVIEW_CONNECTION: &str = "Connection";
/// Section heading on the Overview tab for the detail card.
pub const UI_OVERVIEW_DETAIL: &str = "Details";
/// Section heading on the Overview tab for the process card.
pub const UI_OVERVIEW_PROCESS: &str = "Process";
/// Section heading on the Overview tab for the recent-log card.
pub const UI_OVERVIEW_RECENT_LOGS: &str = "Recent logs";
/// Acknowledge the local-only bind (F-INS-02 AC4).
pub const UI_OVERVIEW_LOOPBACK_ONLY: &str = "Listens on 127.0.0.1 and ::1 only";
/// Note that the connection URI is available although the instance is stopped.
pub const UI_OVERVIEW_URI_STOPPED_NOTE: &str =
    "The URI can still be copied, but the instance is not running.";
/// Label for the `.env` block preview.
pub const UI_OVERVIEW_DOTENV: &str = "# .env";
/// Detail row label: engine.
pub const UI_OVERVIEW_ENGINE: &str = "Engine";
/// Detail row label: superuser.
pub const UI_OVERVIEW_SUPERUSER: &str = "Superuser";
/// Detail row label: data directory.
pub const UI_OVERVIEW_DATA_DIR: &str = "Data directory";
/// Detail row label: data size.
pub const UI_OVERVIEW_DATA_SIZE: &str = "Data size";
/// Marker shown next to an instance on the engine's canonical port.
pub const UI_OVERVIEW_CANONICAL_PORT: &str = "(canonical)";
/// Process row label: PID.
pub const UI_OVERVIEW_PID: &str = "PID";
/// Process row label: health check.
pub const UI_OVERVIEW_HEALTH: &str = "Health check";
/// Health value when the protocol probe passed.
pub const UI_OVERVIEW_HEALTH_OK: &str = "OK · protocol handshake";
/// Health value when the last start failed.
pub const UI_OVERVIEW_HEALTH_FAILED: &str = "Failed — see Logs";
/// Health value when the instance is not running.
pub const UI_OVERVIEW_HEALTH_OFFLINE: &str = "Not running";
/// Process row label: process mode.
pub const UI_OVERVIEW_PROCESS_MODE: &str = "Process mode";
/// Process mode value: spawned detached, survives an app quit (D-10).
pub const UI_OVERVIEW_PROCESS_DETACHED: &str = "Detached · keeps running when the app quits";
/// Process row label: credential reference.
pub const UI_OVERVIEW_CREDENTIAL: &str = "Credential";
/// Autostart checkbox label.
pub const UI_OVERVIEW_AUTOSTART: &str = "Run automatically at login";
/// Link from the Overview card to the Logs tab.
pub const UI_OVERVIEW_GOTO_LOGS: &str = "Open the Logs tab";
/// Placeholder for a value that has no answer yet.
pub const UI_VALUE_UNKNOWN: &str = "—";
/// Sub-tabs of an instance.
pub const UI_SUBTAB_OVERVIEW: &str = "Overview";
/// Logs sub-tab.
pub const UI_SUBTAB_LOGS: &str = "Logs";
/// Databases sub-tab.
pub const UI_SUBTAB_DATABASES: &str = "Databases";
/// Config sub-tab.
pub const UI_SUBTAB_CONFIG: &str = "Config";
/// Header action: open the instance in another client (F-OPN, Phase 2).
pub const UI_ACTION_OPEN_IN: &str = "Open in…";
/// Header action: restart a running instance.
pub const UI_ACTION_RESTART: &str = "Restart";
/// Header action: close a tab.
pub const UI_ACTION_CLOSE_TAB: &str = "Close tab";

// --- Status bar (D-25) -------------------------------------------------------

/// Status bar: stopped count fragment.
pub const UI_STOPPED: &str = "stopped";
/// Status bar: failed count fragment.
pub const UI_FAILED: &str = "failed";
/// Status bar: the data directory label.
pub const UI_DATA_DIR: &str = "Data directory";
/// Status bar: the credential backend label.
pub const UI_CREDENTIAL_BACKEND: &str = "Credential";
/// Status bar: credential backend name when the OS keychain is in use.
pub const UI_CREDENTIAL_KEYCHAIN: &str = "Keychain";
/// Status bar: credential backend name when the `0600` file fallback is in use.
pub const UI_CREDENTIAL_FILE: &str = "File (0600)";

// --- Downloads (F-RTM-01) ----------------------------------------------------

/// Active-download banner heading.
pub const UI_DOWNLOAD_HEADING: &str = "Preparing the runtime";
/// Cancel an active download.
pub const UI_DOWNLOAD_CANCEL: &str = "Cancel";
/// Copy logs from the log viewer.
pub const UI_LOGS_SHOW_FILE: &str = "engine.log";
/// Log footer: rotation policy (F-LOG-01 AC2).
pub const UI_LOGS_ROTATION: &str = "rotation up to 3 × 10 MB";
/// Log line count suffix.
pub const UI_LOGS_LINE_COUNT: &str = "lines";
/// Logs tab: follow-mode toggle hint.
pub const UI_LOGS_FOLLOW_HINT: &str = "Follow new lines";

// --- Diagnose banner (F-LOG-02) ---------------------------------------------

/// One-click fix heading for a port conflict (F-PRT-02 AC1).
pub const UI_DIAGNOSIS_PORT_HEADING: &str = "Port {port} is already in use";
/// Explanation under the port-conflict heading.
pub const UI_DIAGNOSIS_PORT_BODY: &str = "Binding 127.0.0.1:{port} failed before the engine was started. A process outside DBLocal holds the port. The nearest free port is {suggested}.";
/// One-click fix action: move to the next port and start (F-PRT-02 AC2).
pub const UI_DIAGNOSIS_MOVE_AND_START: &str = "Move to {port} and start";
/// Report-issue action for an unrecognised failure (F-LOG-02 AC2).
pub const UI_DIAGNOSIS_REPORT: &str = "Report an issue";
/// Toast after the issue template was prepared.
pub const UI_DIAGNOSIS_REPORT_READY: &str =
    "Issue template prepared without the password and without the home path";

/// Formats the port-conflict heading.
pub fn diagnosis_port_heading(port: u16) -> String {
    UI_DIAGNOSIS_PORT_HEADING.replace("{port}", &port.to_string())
}

/// Formats the port-conflict explanation.
pub fn diagnosis_port_body(port: u16, suggested: u16) -> String {
    UI_DIAGNOSIS_PORT_BODY
        .replace("{port}", &port.to_string())
        .replace("{suggested}", &suggested.to_string())
}

/// Formats the "move to port and start" action label.
pub fn diagnosis_move_and_start(port: u16) -> String {
    UI_DIAGNOSIS_MOVE_AND_START.replace("{port}", &port.to_string())
}

// --- Databases tab (demo content, mock build only) --------------------------

/// Databases table heading.
pub const UI_DATABASES_HEADING: &str = "Database";
/// Databases table column: name.
pub const UI_DATABASES_COLUMN_NAME: &str = "Name";
/// Databases table column: encoding.
pub const UI_DATABASES_COLUMN_ENCODING: &str = "Encoding";
/// Databases table column: size.
pub const UI_DATABASES_COLUMN_SIZE: &str = "Size";
/// Databases table: default encoding values.
pub const UI_DATABASES_ENCODING: &str = "UTF8 · C.UTF-8";
/// Databases tab: create action (F-DBU-01).
pub const UI_DATABASES_CREATE: &str = "Create database";
/// Databases tab: the name field placeholder.
pub const UI_DATABASES_NAME_PLACEHOLDER: &str = "database_name";
/// Databases tab: delete action.
pub const UI_DATABASES_DELETE: &str = "Delete database (type the name to confirm)";
/// Databases tab: validation failure for a bad name (F-DBU-01 AC4).
pub const UI_DATABASES_INVALID_NAME: &str =
    "The name may contain only letters, digits and underscores.";
/// Databases tab: heading above the user list.
pub const UI_DATABASES_USERS: &str = "Users and privileges";
/// Databases tab: new user action.
pub const UI_DATABASES_NEW_USER: &str = "New user";
/// Databases tab: the boundary note that DBLocal manages servers, not data.
pub const UI_DATABASES_BOUNDARY: &str =
    "DBLocal manages the server, not the data. To browse tables, open the instance in your client.";
/// Databases tab: superuser password card heading.
pub const UI_DATABASES_PASSWORD_HEADING: &str = "Superuser password";
/// Databases tab: superuser password card body.
pub const UI_DATABASES_PASSWORD_BODY: &str =
    "Generates a new password and updates the URI. If it fails, the old password stays in effect.";
/// Databases tab: reset-password action (F-DBU-03).
pub const UI_DATABASES_RESET: &str = "Reset password";
/// Databases tab: toast after a database is created.
pub const UI_DATABASES_CREATED: &str = "Database {name} created (UTF-8)";
/// Databases tab: toast after the superuser password is reset.
pub const UI_DATABASES_PASSWORD_RESET: &str = "New password saved in the keychain";
/// Databases tab: why the list is empty while the instance is not running.
pub const UI_DATABASES_STOPPED: &str = "Start the instance to view and manage databases.";

/// Formats the database-created toast.
pub fn databases_created(name: &str) -> String {
    UI_DATABASES_CREATED.replace("{name}", name)
}

// --- Config tab: shared strings the live screen (`W4-H`) reuses -------------
//
// The card title, apply label and the rest of the live chrome live in
// `strings/config.rs`; these four are the prototype's words the live screen
// keeps (`UI_CONFIG_COLUMNS`, the restart banner and the open-file action).

/// Config table column note.
pub const UI_CONFIG_COLUMNS: &str = "Current value · Default";
/// Config banner: restart later.
pub const UI_CONFIG_LATER: &str = "Later";
/// Config banner: restart now.
pub const UI_CONFIG_RESTART_NOW: &str = "Restart now";
/// Config footer: open the config file (F-CFG-02).
pub const UI_CONFIG_OPEN_FILE: &str = "Open config file";

// --- New instance dialog (D-25 prototype layout) ----------------------------

/// Intro line at the top of the create dialog.
pub const UI_CREATE_INTRO: &str =
    "The runtime is downloaded from upstream and verified with SHA-256 before it is installed.";
/// Section heading for the version list.
pub const UI_CREATE_VERSIONS: &str = "Versions · {engine}";
/// Badge on a catalog version that is not installed yet.
pub const UI_CREATE_DOWNLOAD_BADGE: &str = "Download, {size}";
/// Inline hint under the name field.
pub const UI_CREATE_NAME_HINT: &str = "lowercase letters, digits, hyphens";
/// Inline hint under the port field.
pub const UI_CREATE_PORT_HINT: &str = "next free port";
/// Inline hint under the superuser field.
pub const UI_CREATE_SUPERUSER_HINT: &str = "password → keychain";
/// Footer note when the chosen runtime is already installed.
pub const UI_CREATE_READY_NOTE: &str = "Runtime already installed — ready in under 10 seconds";
/// Footer note when a download is needed (F-INS-01 AC2).
pub const UI_CREATE_DOWNLOAD_NOTE: &str = "Will download the runtime, then verify SHA-256";
/// Submit button label in the create dialog (prototype wording).
pub const UI_CREATE_SUBMIT_PROTOTYPE: &str = "Create";
/// The no-password option label.
pub const UI_CREATE_NO_PASSWORD: &str = "Allow connections without a password";
/// The full warning under the no-password option (F-CRD-03 AC1).
pub const UI_CREATE_NO_PASSWORD_WARNING: &str = "Other processes and users on this machine can connect without credentials. This cannot be changed after creation.";

/// Formats the version-list heading.
pub fn create_versions(engine: &str) -> String {
    UI_CREATE_VERSIONS.replace("{engine}", engine)
}

/// Formats the download badge with its size.
pub fn create_download_badge(size: &str) -> String {
    UI_CREATE_DOWNLOAD_BADGE.replace("{size}", size)
}

// --- Theme and appearance ----------------------------------------------------

/// Label of the dark theme in a picker.
pub const UI_THEME_DARK: &str = "DBLocal Dark";
/// Label of the light theme in a picker.
pub const UI_THEME_LIGHT: &str = "DBLocal Light";

// --- Delete dialog confirmations (D-18) -------------------------------------

/// Delete dialog: archive mode keeps the data (the default).
pub const UI_DELETE_ARCHIVE_HINT: &str =
    "Data is moved to the archive and the instance is deleted.";
/// Delete dialog: purge mode removes the data too.
pub const UI_DELETE_PURGE_HINT: &str = "The instance and all of its data are permanently deleted.";

// --- Palette hints -----------------------------------------------------------

/// Palette row: focus an open instance tab.
pub const UI_PALETTE_FOCUS: &str = "Focus";

// --- Toast after an operation that changed state -----------------------------

/// Toast after a start request was accepted.
pub const UI_TOAST_START_REQUESTED: &str = "Start request sent";
/// Toast after a stop request was accepted.
pub const UI_TOAST_STOP_REQUESTED: &str = "Stop request sent";
/// Toast after an instance was deleted.
pub const UI_TOAST_DELETED: &str = "Instance deleted";
/// Toast after an autostart toggle.
pub const UI_TOAST_AUTOSTART_ON: &str = "The instance will run at login";
/// Toast after an autostart toggle was cleared.
pub const UI_TOAST_AUTOSTART_OFF: &str = "Autostart turned off";

// --- Window controls (per platform, as the prototype draws them) -----------

/// Title bar: minimise the window.
pub const UI_WINDOW_MINIMIZE: &str = "Minimize";
/// Title bar: maximise the window.
pub const UI_WINDOW_MAXIMIZE: &str = "Maximize";
/// Title bar: close the window.
pub const UI_WINDOW_CLOSE: &str = "Close window";

// --- Tray and quit (F-TRY-01, F-TRY-02) ------------------------------------

/// Tray menu item that brings the main window to the front.
pub const UI_TRAY_OPEN: &str = "Open DBLocal";
/// Tray menu item that quits the app.
pub const UI_TRAY_QUIT: &str = "Quit";
/// Status-bar note shown once when no tray host is available.
pub const UI_TRAY_UNAVAILABLE_NOTE: &str = "The tray is not available in this desktop environment.";
/// Quit dialog heading.
pub const UI_QUIT_TITLE: &str = "Quit DBLocal?";
/// Quit dialog body shown when instances are still running.
pub const UI_QUIT_BODY: &str =
    "Some instances are still running. Choose what should happen when you quit.";
/// Quit dialog: keep engines running (default).
pub const UI_QUIT_KEEP_RUNNING: &str = "Leave them running";
/// Quit dialog: stop every running instance before quitting.
pub const UI_QUIT_STOP_ALL: &str = "Stop all, then quit";
/// Quit dialog: a stop failed but the user can force quit anyway.
pub const UI_QUIT_STOP_FAILED: &str = "Some instances could not be stopped.";
/// Quit dialog: confirm quitting while stops are still pending.
pub const UI_QUIT_FORCE_CONFIRM: &str = "Some instances are still stopping. Quit now?";
/// Quit dialog: progress line while stopping all instances.
pub const UI_QUIT_STOPPING: &str = "Stopping instances...";
/// Neutral status-bar note while a background service call is in flight.
pub const UI_PENDING: &str = "Loading...";
/// Startup failure window title.
pub const UI_STARTUP_FAILED_TITLE: &str = "DBLocal could not start";
/// Startup failure window fallback body.
pub const UI_STARTUP_FAILED_BODY: &str = "DBLocal could not read or prepare its local data.";

mod admin;
pub use admin::*;

mod config;
pub use config::*;

mod update;
pub use update::*;
