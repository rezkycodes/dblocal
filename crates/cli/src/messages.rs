//! User-facing CLI strings. Flags stay English.
//!
//! Every sentence, label and code the CLI prints lives here; a handler that
//! builds its own text is a defect, because this file is also the translation
//! and documentation surface.

/// Code for a CLI-side wait that ran out of time.
///
/// `DBL-84xx` is the CLI range from `docs/architecture/error-codes.md`; a
/// timeout on the CLI side is not an engine failure and must not wear an engine
/// code.
pub const CODE_TIMEOUT: &str = "DBL-8401";
/// Message for [`CODE_TIMEOUT`].
pub const ERR_TIMEOUT: &str = "timed out";
/// Code for a failure to build the CLI's own async runtime.
pub const CODE_RUNTIME: &str = "DBL-8402";
/// Message for [`CODE_RUNTIME`].
pub const ERR_RUNTIME: &str = "could not set up the CLI runtime";

/// Label in front of a start failure's diagnosis.
pub const LABEL_DIAGNOSIS: &str = "diagnosis:";
/// Prefix of a download-progress line.
pub const PROGRESS_DOWNLOAD: &str = "... downloading";
/// Prefix of an operation-stage line.
pub const PROGRESS_STAGE: &str = "...";

pub const ERR_SERVICE_OPEN: &str = "Could not open the DBLocal service.";
pub const ERR_INSTANCE_NOT_FOUND: &str = "Instance not found";
pub const ERR_ALREADY_RUNNING: &str = "already running";
pub const ERR_STOPPING_ELSEWHERE: &str = "another process is stopping it";
pub const ERR_NAME_CONFIRMATION: &str = "Delete cancelled: the instance name does not match.";
pub const ERR_DELETE_NEEDS_YES: &str = "Deleting requires --yes in a non-interactive session.";
pub const ERR_UNKNOWN_ENGINE: &str = "Unknown engine.";
pub const ERR_CONFIRM_READ: &str = "could not read the confirmation";
pub const ERR_CREATE_MISSING: &str = "the instance is missing after create";
pub const MSG_ARCHIVED_TO: &str = "Data archived to";
pub const MSG_DELETED: &str = "Instance deleted.";
pub const MSG_CREATING: &str = "Creating instance";
pub const MSG_STARTING: &str = "Starting instance";
pub const MSG_STOPPING: &str = "Stopping instance";
pub const MSG_ALREADY_STOPPED: &str = "already stopped";
pub const MSG_READY: &str = "Ready.";
pub const PROMPT_DELETE: &str = "Type the instance name to confirm deletion: ";

/// Reply to `autostart status`/`enable` when the login item exists.
pub const AUTOSTART_ENABLED: &str = "autostart: enabled";
/// Reply to `autostart status`/`disable` when the login item is absent.
pub const AUTOSTART_DISABLED: &str = "autostart: disabled";
/// Label in front of the login-item location.
pub const AUTOSTART_LOCATION: &str = "location";
/// Unknown `autostart` action.
pub const ERR_AUTOSTART_ACTION: &str = "Unknown autostart action";
/// The running executable path could not be resolved for `autostart enable`.
pub const ERR_AUTOSTART_EXE: &str = "could not determine the application location";

/// Formats the service-open failure with a stable-looking diagnostic.
pub fn service_open_error(detail: &str) -> String {
    format!("{ERR_SERVICE_OPEN} {detail}")
}

/// The "wait ran out" line, with its code.
pub fn timeout_line() -> String {
    format!("[{CODE_TIMEOUT}] {ERR_TIMEOUT}")
}

/// The "CLI runtime could not be built" line, with its code.
pub fn runtime_line(detail: &str) -> String {
    format!("[{CODE_RUNTIME}] {}", runtime_message(detail))
}

/// The runtime failure's message, before the code is prepended.
pub fn runtime_message(detail: &str) -> String {
    format!("{ERR_RUNTIME}: {detail}")
}

/// The "another process is stopping it" line, with its code.
pub fn stopping_elsewhere_line() -> String {
    format!(
        "[{}] {ERR_STOPPING_ELSEWHERE}",
        dblocal_core::error::CODE_INVALID_TRANSITION
    )
}

/// The diagnosis label followed by the engine's diagnosis.
pub fn diagnosis_line(diagnosis: &str) -> String {
    format!("{LABEL_DIAGNOSIS} {diagnosis}")
}

/// One download-progress line.
pub fn download_progress(percent: u64, done: u64, total: u64) -> String {
    format!("{PROGRESS_DOWNLOAD} {percent}% ({done}/{total})")
}

/// One operation-stage line.
pub fn stage_line(stage: &str) -> String {
    format!("{PROGRESS_STAGE} {stage}")
}

/// A diagnostic line: `[DBL-NNNN] message`.
pub fn code_line(code: &str, message: &str) -> String {
    format!("[{code}] {message}")
}

// --- Database, user, password, update, open (W4-A) ---------------------------

/// A database was created.
pub const MSG_DB_CREATED: &str = "Database created.";
/// A database was dropped.
pub const MSG_DB_DROPPED: &str = "Database deleted.";
/// A login role was created.
pub const MSG_USER_CREATED: &str = "User created. Save this password; it is shown only once:";
/// The superuser password was reset; the new URI is printed once.
pub const MSG_PASSWORD_RESET: &str = "Superuser password changed. New URI (shown only once):";
/// No newer release is available.
pub const MSG_UPDATE_UP_TO_DATE: &str = "DBLocal is up to date.";
/// An update is available; `{}` is the version.
pub const MSG_UPDATE_AVAILABLE: &str = "A new version is available:";
/// The update was applied.
pub const MSG_UPDATE_APPLIED: &str = "Update installed.";
/// Formats the post-install message with the installed version.
pub fn update_applied_version(version: &str) -> String {
    format!("{MSG_UPDATE_APPLIED} Restart DBLocal to use v{version}.")
}
/// The install is package-manager owned, so self-update is refused.
pub const MSG_UPDATE_PACKAGE_MANAGER: &str =
    "Automatic updates are not available; update through your package manager.";
/// The client was launched.
pub const MSG_CLIENT_OPENED: &str = "Client opened.";
/// The dry run printed the plan instead of launching.
pub const MSG_CLIENT_DRY_RUN: &str = "Launch plan (not executed):";
/// `update apply` requires `--yes` on a non-interactive session.
pub const ERR_UPDATE_NEEDS_YES: &str =
    "Installing an update requires --yes in a non-interactive session.";
/// The `--confirm` value did not match the database name.
pub const ERR_DB_CONFIRM_MISMATCH: &str = "Delete cancelled: the database name does not match.";
/// `db drop` needs `--yes` or a matching `--confirm` on a non-interactive session.
pub const ERR_DB_NEEDS_CONFIRM: &str = "Dropping a database requires --yes or --confirm <name>.";
/// The `--level` value was neither `ro` nor `rw`.
pub const ERR_BAD_LEVEL: &str = "The privilege level must be `ro` or `rw`.";
/// The `--client` value named no known client.
pub const ERR_UNKNOWN_CLIENT: &str = "Unknown client.";
/// Prompt for the typed database-name confirmation.
pub const PROMPT_DB_CONFIRM: &str = "Type the database name to confirm deletion: ";

// --- runtime (F-RTM-04) ------------------------------------------------------

/// `USERS` cell for a runtime no instance uses.
pub const RUNTIME_UNUSED: &str = "unused";
/// No installed runtime matched the engine and version.
pub const ERR_RUNTIME_NOT_FOUND: &str = "Runtime not found";
/// `runtime remove` requires `--yes` on a non-interactive session.
pub const ERR_RUNTIME_REMOVE_NEEDS_YES: &str =
    "Removing a runtime requires --yes in a non-interactive session.";
/// The typed version did not match.
pub const ERR_RUNTIME_CONFIRMATION: &str = "Remove cancelled: the version does not match.";
/// The runtime was removed.
pub const MSG_RUNTIME_REMOVED: &str = "Runtime removed.";

/// Prompt for the typed version confirmation of `runtime remove`.
pub fn runtime_remove_prompt(engine: &str, version: &str) -> String {
    format!("Type the version ({version}) to remove {engine} {version}: ")
}

/// The line naming the instances that keep a runtime from being removed.
pub fn runtime_used_by(users: &[String]) -> String {
    format!("Used by: {}", users.join(", "))
}

// --- toolchain (v0.3.0) ------------------------------------------------------

/// A toolchain action was cancelled at the prompt.
pub const ERR_TOOLCHAIN_CANCELLED: &str = "Cancelled.";
/// A confirmation-requiring action was run non-interactively without `--yes`.
pub const ERR_TOOLCHAIN_NEEDS_YES: &str = "This action needs --yes in a non-interactive session.";
/// More than one of `--print`/`--apply`/`--remove` was given to `toolchain path`.
pub const ERR_TOOLCHAIN_PATH_FLAGS: &str = "Pass at most one of --print, --apply or --remove.";
/// `php-settings set` was called without a key and value.
pub const ERR_TOOLCHAIN_SET_USAGE: &str = "Usage: php-settings set <key> <value>";
/// The PATH block was already in the requested state.
pub const MSG_TOOLCHAIN_PATH_UNCHANGED: &str = "PATH already in the requested state.";
/// The doctor found no problems.
pub const MSG_TOOLCHAIN_DOCTOR_OK: &str = "No problems found.";

/// An unknown tool name was supplied.
pub fn toolchain_unknown_tool(tool: &str) -> String {
    format!(
        "[{}] Unknown tool: {tool} (expected php, node or composer)",
        dblocal_core::error::CODE_INVALID_INPUT
    )
}

/// Prompt before installing.
pub fn toolchain_install_prompt(tool: &str, spec: &str) -> String {
    format!("Install {tool} {spec}? [y/N] ")
}

/// Progress line before an install starts.
pub fn toolchain_installing(tool: &str, spec: &str) -> String {
    format!("Installing {tool} {spec}...")
}

/// Success line after an install.
pub fn toolchain_installed(
    tool: &str,
    version: &str,
    profile: Option<&str>,
    already: bool,
    activated: bool,
) -> String {
    let label = match profile {
        Some(profile) => format!("{tool} {version} ({profile})"),
        None => format!("{tool} {version}"),
    };
    let mut line = if already {
        format!("{label} is already installed.")
    } else {
        format!("Installed {label}.")
    };
    if activated {
        line.push_str(" It is now active.");
    }
    line
}

/// Success line after `use`.
pub fn toolchain_activated(tool: &str, version: &str, profile: Option<&str>) -> String {
    match profile {
        Some(profile) => format!("Now using {tool} {version} ({profile})."),
        None => format!("Now using {tool} {version}."),
    }
}

/// Prompt before removing.
pub fn toolchain_remove_prompt(tool: &str, spec: &str) -> String {
    format!("Remove {tool} {spec}? [y/N] ")
}

/// Success line after a remove.
pub fn toolchain_removed(tool: &str, version: &str) -> String {
    format!("Removed {tool} {version}.")
}

/// The active entry was cleared as part of a remove.
pub fn toolchain_cleared_active(tool: &str) -> String {
    format!("{tool} no longer has an active version.")
}

/// Other versions of a tool remain installed after its active one was removed.
pub fn toolchain_others_remain(tool: &str) -> String {
    format!(
        "Other {tool} versions are still installed; pick one with `dblocal toolchain use {tool} <version>`."
    )
}

/// A tool has no active version.
pub fn toolchain_no_active(tool: &str) -> String {
    format!(
        "[{}] No active {tool}; run `dblocal toolchain use {tool} <version>` first.",
        dblocal_core::toolchain::error::CODE_NO_ACTIVE_VERSION
    )
}

/// Prompt for `toolchain path --apply`.
pub fn toolchain_path_apply_prompt() -> String {
    "This adds the lines above to your shell startup files (a backup is written first). Continue? [y/N] "
        .to_owned()
}

/// One file changed by a PATH edit.
pub fn toolchain_path_changed(path: &str) -> String {
    format!("Updated {path}")
}

/// A PATH edit could not touch a file.
pub fn toolchain_path_failed(path: &str, reason: &str) -> String {
    format!("Could not update {path}: {reason}")
}

/// Summary of the backups written by a PATH edit.
pub fn toolchain_backups_written(count: usize) -> String {
    format!("Wrote {count} backup file(s).")
}

/// The current PATH state, with the shims dir.
pub fn toolchain_path_state(applied: bool, dir: &str) -> String {
    if applied {
        format!("PATH block applied ({dir} is prepended).")
    } else {
        format!("PATH block not applied. Shims live in {dir}.")
    }
}

/// The managed settings file was written.
pub fn toolchain_settings_written(path: &str) -> String {
    format!("Wrote {path}")
}

/// An unknown `php-settings` action was supplied.
pub fn toolchain_unknown_action(action: &str) -> String {
    format!(
        "[{}] Unknown action: {action} (expected get, set, reset or open)",
        dblocal_core::error::CODE_INVALID_INPUT
    )
}
