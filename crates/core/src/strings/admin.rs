//! W4-B live-screen strings: Databases (`F-DBU`), Open in… (`F-OPN`) and
//! Settings (`F-PKG-05`, `F-AUT-01`). Re-exported from `strings` so call sites
//! stay `strings::UI_...`.

// --- Databases tab, live over DblService (F-DBU-01..03) ----------------------

/// Databases card: heading with the row count (`Database · 3`).
pub const UI_DATABASES_HEADING_COUNT: &str = "Database · {count}";
/// Databases table column: owner.
pub const UI_DATABASES_COLUMN_OWNER: &str = "Owner";
/// Databases tab: the list is being read.
pub const UI_DATABASES_LOADING: &str = "Reading databases…";
/// Databases tab: the list is empty.
pub const UI_DATABASES_EMPTY: &str = "No databases yet.";
/// Databases tab: the users card is being read.
pub const UI_DATABASES_USERS_LOADING: &str = "Reading users…";
/// Databases tab: no non-superuser login roles yet.
pub const UI_DATABASES_USERS_EMPTY: &str = "No additional users yet.";
/// Databases tab: one-click start on a stopped instance.
pub const UI_DATABASES_START: &str = "Start the instance";
/// Databases tab: retry a failed read.
pub const UI_DATABASES_RETRY: &str = "Reload";
/// Databases tab: delete dialog title.
pub const UI_DATABASES_DROP_TITLE: &str = "Delete database";
/// Databases tab: delete dialog explanation.
pub const UI_DATABASES_DROP_PROMPT: &str = "The database and all of its contents are permanently deleted. Type the database name to confirm.";
/// Databases tab: delete dialog short danger line under the explanation.
pub const UI_DATABASES_DROP_WARNING: &str = "This cannot be undone.";
/// Databases tab: delete dialog confirm action.
pub const UI_DATABASES_DROP_CONFIRM: &str = "Delete database";
/// Databases tab: delete dialog input placeholder.
pub const UI_DATABASES_DROP_PLACEHOLDER: &str = "Database name";
/// Databases tab: create-user card heading.
pub const UI_DATABASES_CREATE_USER: &str = "Create user";
/// Databases tab: create-user name field placeholder.
pub const UI_DATABASES_USER_NAME: &str = "user_name";
/// Databases tab: create-user database selector label.
pub const UI_DATABASES_USER_DATABASE: &str = "Database";
/// Databases tab: create-user privilege label.
pub const UI_DATABASES_USER_LEVEL: &str = "Privileges";
/// Privilege level: read-only.
pub const UI_DATABASES_LEVEL_READ_ONLY: &str = "Read-only";
/// Privilege level: read-write.
pub const UI_DATABASES_LEVEL_READ_WRITE: &str = "Read-write";
/// Privilege label shown on a role that is a superuser.
pub const UI_DATABASES_LEVEL_SUPERUSER: &str = "superuser";
/// Privilege label shown on a role the service lists without a level.
pub const UI_DATABASES_LEVEL_LOGIN: &str = "login";
/// Databases tab: submit the new user.
pub const UI_DATABASES_USER_SUBMIT: &str = "Create user";
/// One-time password dialog title.
pub const UI_DATABASES_SECRET_TITLE: &str = "New password";
/// One-time password dialog: the warning that it is not shown again (F-DBU-02 AC2).
pub const UI_DATABASES_SECRET_ONCE: &str =
    "This password is shown only once and will not be shown again.";
/// One-time password dialog: label above the value.
pub const UI_DATABASES_SECRET_VALUE: &str = "Password";
/// One-time password dialog: label above a URI.
pub const UI_DATABASES_SECRET_URI: &str = "New URI";
/// One-time dialog: copy action.
pub const UI_DATABASES_SECRET_COPY: &str = "Copy";
/// One-time dialog: close action.
pub const UI_DATABASES_SECRET_CLOSE: &str = "I have saved it";
/// Databases tab: toast after the one-time value is copied.
pub const UI_DATABASES_SECRET_COPIED: &str = "Copied to the clipboard";
/// Databases tab: reset-password confirmation title.
pub const UI_DATABASES_RESET_TITLE: &str = "Reset the superuser password";
/// Databases tab: reset-password confirmation body.
pub const UI_DATABASES_RESET_PROMPT: &str = "A new password is generated, applied to the engine and saved in the keychain. The URI is updated.";
/// Databases tab: reset confirmation action.
pub const UI_DATABASES_RESET_CONFIRM: &str = "Reset password";
/// Databases tab: shown next to a failed reset (F-DBU-03 AC2).
pub const UI_DATABASES_RESET_FAILED_NOTE: &str = "The old password is still in effect.";
/// Databases tab: the reset succeeded, so the keychain holds the new value.
pub const UI_DATABASES_RESET_DONE: &str = "The superuser password was updated.";
/// Databases tab: the create-user password succeeded.
pub const UI_DATABASES_USER_DONE: &str = "User {name} created.";
/// Databases tab: toast after a database is dropped.
pub const UI_DATABASES_DROPPED: &str = "Database {name} deleted";

/// Formats the databases heading with its row count.
pub fn databases_heading(count: usize) -> String {
    UI_DATABASES_HEADING_COUNT.replace("{count}", &count.to_string())
}

/// Formats the user-created confirmation.
pub fn databases_user_created(name: &str) -> String {
    UI_DATABASES_USER_DONE.replace("{name}", name)
}

/// Formats the database-dropped confirmation.
pub fn databases_dropped(name: &str) -> String {
    UI_DATABASES_DROPPED.replace("{name}", name)
}

// --- Open in… menu, live over DblService (F-OPN-01..02) ---------------------

/// Menu group heading for GUI clients.
pub const UI_OPEN_IN_GUI_GROUP: &str = "GUI applications";
/// Menu group heading for CLI clients.
pub const UI_OPEN_IN_CLI_GROUP: &str = "Terminal (CLI)";
/// Menu item that re-runs detection.
pub const UI_OPEN_IN_REFRESH: &str = "Refresh the client list";
/// Menu: detection failed, with the typed error underneath.
pub const UI_OPEN_IN_FAILED: &str = "Client detection failed.";
/// Launcher result: the client was started.
pub const UI_OPEN_IN_LAUNCHED: &str = "{client} opened.";

/// Formats the launch confirmation.
pub fn open_in_launched(client: &str) -> String {
    UI_OPEN_IN_LAUNCHED.replace("{client}", client)
}

// --- Settings dialog (F-PKG-05, F-AUT-01) -----------------------------------

/// Title bar: open the settings dialog.
pub const UI_SETTINGS: &str = "Settings";
/// Settings dialog title.
pub const UI_SETTINGS_TITLE: &str = "Settings";
/// Settings section heading: updates.
pub const UI_SETTINGS_UPDATES: &str = "Updates";
/// Settings updates: the running version label.
pub const UI_SETTINGS_CURRENT_VERSION: &str = "Current version";
/// Settings updates: how the app was installed.
pub const UI_SETTINGS_INSTALL_KIND: &str = "How it was installed";
/// Settings updates: the manual check action.
pub const UI_SETTINGS_CHECK_NOW: &str = "Check now";
/// Settings updates: the check is in flight.
pub const UI_SETTINGS_CHECKING: &str = "Checking…";
/// Settings updates: no newer release.
pub const UI_SETTINGS_UP_TO_DATE: &str = "DBLocal is up to date.";
/// Settings updates: a newer release, with its version.
pub const UI_SETTINGS_AVAILABLE: &str = "Version {version} is available.";
/// Settings updates: install action.
pub const UI_SETTINGS_INSTALL: &str = "Install";
/// Settings updates: install is in flight.
pub const UI_SETTINGS_INSTALLING: &str = "Installing…";
/// Settings updates: the update was applied.
pub const UI_SETTINGS_INSTALLED: &str = "Update installed. Restart DBLocal.";
/// Settings section heading: run at login.
pub const UI_SETTINGS_AUTOSTART: &str = "Run at login";
/// Settings autostart: explanation of what turning it on does (F-AUT-01 AC1).
pub const UI_SETTINGS_AUTOSTART_BODY: &str =
    "DBLocal registers as a login item for this user: no sudo, no system service.";
/// Settings autostart: switch label.
pub const UI_SETTINGS_AUTOSTART_SWITCH: &str = "Run DBLocal at login";
/// Settings autostart: the item is registered.
pub const UI_SETTINGS_AUTOSTART_ON: &str = "Registered for login.";
/// Settings autostart: the item is not registered.
pub const UI_SETTINGS_AUTOSTART_OFF: &str = "Not registered for login.";
/// Settings autostart: the status could not be read.
pub const UI_SETTINGS_AUTOSTART_UNKNOWN: &str = "Login item status is unknown.";
/// Settings autostart: where the login item lives.
pub const UI_SETTINGS_AUTOSTART_LOCATION: &str = "Location";
/// Settings autostart: close action.
pub const UI_SETTINGS_CLOSE: &str = "Close";

/// Formats the "newer version available" line.
pub fn settings_available(version: &str) -> String {
    UI_SETTINGS_AVAILABLE.replace("{version}", version)
}
