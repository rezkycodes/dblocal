//! W4-B live-screen strings: Databases (`F-DBU`), Open in… (`F-OPN`) and
//! Settings (`F-PKG-05`, `F-AUT-01`). Re-exported from `strings` so call sites
//! stay `strings::UI_...`.

// --- Databases tab, live over DblService (F-DBU-01..03) ----------------------

/// Databases card: heading with the row count (`Database · 3`).
pub const UI_DATABASES_HEADING_COUNT: &str = "Database · {count}";
/// Databases table column: owner.
pub const UI_DATABASES_COLUMN_OWNER: &str = "Pemilik";
/// Databases tab: the list is being read.
pub const UI_DATABASES_LOADING: &str = "Membaca database…";
/// Databases tab: the list is empty.
pub const UI_DATABASES_EMPTY: &str = "Belum ada database.";
/// Databases tab: the users card is being read.
pub const UI_DATABASES_USERS_LOADING: &str = "Membaca user…";
/// Databases tab: no non-superuser login roles yet.
pub const UI_DATABASES_USERS_EMPTY: &str = "Belum ada user tambahan.";
/// Databases tab: one-click start on a stopped instance.
pub const UI_DATABASES_START: &str = "Nyalakan instance";
/// Databases tab: retry a failed read.
pub const UI_DATABASES_RETRY: &str = "Muat ulang";
/// Databases tab: delete dialog title.
pub const UI_DATABASES_DROP_TITLE: &str = "Hapus database";
/// Databases tab: delete dialog explanation.
pub const UI_DATABASES_DROP_PROMPT: &str =
    "Database dan seluruh isinya dihapus permanen. Ketik nama database untuk mengonfirmasi.";
/// Databases tab: delete dialog confirm action.
pub const UI_DATABASES_DROP_CONFIRM: &str = "Hapus database";
/// Databases tab: delete dialog input placeholder.
pub const UI_DATABASES_DROP_PLACEHOLDER: &str = "Nama database";
/// Databases tab: create-user card heading.
pub const UI_DATABASES_CREATE_USER: &str = "Buat user";
/// Databases tab: create-user name field placeholder.
pub const UI_DATABASES_USER_NAME: &str = "nama_user";
/// Databases tab: create-user database selector label.
pub const UI_DATABASES_USER_DATABASE: &str = "Database";
/// Databases tab: create-user privilege label.
pub const UI_DATABASES_USER_LEVEL: &str = "Hak akses";
/// Privilege level: read-only.
pub const UI_DATABASES_LEVEL_READ_ONLY: &str = "Read-only";
/// Privilege level: read-write.
pub const UI_DATABASES_LEVEL_READ_WRITE: &str = "Read-write";
/// Privilege label shown on a role that is a superuser.
pub const UI_DATABASES_LEVEL_SUPERUSER: &str = "superuser";
/// Privilege label shown on a role the service lists without a level.
pub const UI_DATABASES_LEVEL_LOGIN: &str = "login";
/// Databases tab: submit the new user.
pub const UI_DATABASES_USER_SUBMIT: &str = "Buat user";
/// One-time password dialog title.
pub const UI_DATABASES_SECRET_TITLE: &str = "Password baru";
/// One-time password dialog: the warning that it is not shown again (F-DBU-02 AC2).
pub const UI_DATABASES_SECRET_ONCE: &str =
    "Password ini hanya ditampilkan sekali dan tidak akan ditampilkan lagi.";
/// One-time password dialog: label above the value.
pub const UI_DATABASES_SECRET_VALUE: &str = "Password";
/// One-time password dialog: label above a URI.
pub const UI_DATABASES_SECRET_URI: &str = "URI baru";
/// One-time dialog: copy action.
pub const UI_DATABASES_SECRET_COPY: &str = "Salin";
/// One-time dialog: close action.
pub const UI_DATABASES_SECRET_CLOSE: &str = "Sudah saya simpan";
/// Databases tab: toast after the one-time value is copied.
pub const UI_DATABASES_SECRET_COPIED: &str = "Disalin ke clipboard";
/// Databases tab: reset-password confirmation title.
pub const UI_DATABASES_RESET_TITLE: &str = "Reset password superuser";
/// Databases tab: reset-password confirmation body.
pub const UI_DATABASES_RESET_PROMPT: &str =
    "Password baru dibuat, diterapkan ke engine, dan disimpan ke keychain. URI akan diperbarui.";
/// Databases tab: reset confirmation action.
pub const UI_DATABASES_RESET_CONFIRM: &str = "Reset password";
/// Databases tab: shown next to a failed reset (F-DBU-03 AC2).
pub const UI_DATABASES_RESET_FAILED_NOTE: &str = "Password lama tetap berlaku.";
/// Databases tab: the reset succeeded, so the keychain holds the new value.
pub const UI_DATABASES_RESET_DONE: &str = "Password superuser diperbarui.";
/// Databases tab: the create-user password succeeded.
pub const UI_DATABASES_USER_DONE: &str = "User {name} dibuat.";

/// Formats the databases heading with its row count.
pub fn databases_heading(count: usize) -> String {
    UI_DATABASES_HEADING_COUNT.replace("{count}", &count.to_string())
}

/// Formats the user-created confirmation.
pub fn databases_user_created(name: &str) -> String {
    UI_DATABASES_USER_DONE.replace("{name}", name)
}

// --- Open in… menu, live over DblService (F-OPN-01..02) ---------------------

/// Menu group heading for GUI clients.
pub const UI_OPEN_IN_GUI_GROUP: &str = "Aplikasi GUI";
/// Menu group heading for CLI clients.
pub const UI_OPEN_IN_CLI_GROUP: &str = "Terminal (CLI)";
/// Menu item that re-runs detection.
pub const UI_OPEN_IN_REFRESH: &str = "Perbarui daftar client";
/// Menu: detection failed, with the typed error underneath.
pub const UI_OPEN_IN_FAILED: &str = "Deteksi client gagal.";
/// Launcher result: the client was started.
pub const UI_OPEN_IN_LAUNCHED: &str = "{client} dibuka.";

/// Formats the launch confirmation.
pub fn open_in_launched(client: &str) -> String {
    UI_OPEN_IN_LAUNCHED.replace("{client}", client)
}

// --- Settings dialog (F-PKG-05, F-AUT-01) -----------------------------------

/// Title bar: open the settings dialog.
pub const UI_SETTINGS: &str = "Pengaturan";
/// Settings dialog title.
pub const UI_SETTINGS_TITLE: &str = "Pengaturan";
/// Settings section heading: updates.
pub const UI_SETTINGS_UPDATES: &str = "Pembaruan";
/// Settings updates: the running version label.
pub const UI_SETTINGS_CURRENT_VERSION: &str = "Versi saat ini";
/// Settings updates: how the app was installed.
pub const UI_SETTINGS_INSTALL_KIND: &str = "Cara pemasangan";
/// Settings updates: the manual check action.
pub const UI_SETTINGS_CHECK_NOW: &str = "Periksa sekarang";
/// Settings updates: the check is in flight.
pub const UI_SETTINGS_CHECKING: &str = "Memeriksa…";
/// Settings updates: no newer release.
pub const UI_SETTINGS_UP_TO_DATE: &str = "DBLocal sudah versi terbaru.";
/// Settings updates: a newer release, with its version.
pub const UI_SETTINGS_AVAILABLE: &str = "Versi {version} tersedia.";
/// Settings updates: install action.
pub const UI_SETTINGS_INSTALL: &str = "Pasang";
/// Settings updates: install is in flight.
pub const UI_SETTINGS_INSTALLING: &str = "Memasang…";
/// Settings updates: the update was applied.
pub const UI_SETTINGS_INSTALLED: &str = "Pembaruan terpasang. Mulai ulang DBLocal.";
/// Settings section heading: run at login.
pub const UI_SETTINGS_AUTOSTART: &str = "Jalankan saat login";
/// Settings autostart: explanation of what turning it on does (F-AUT-01 AC1).
pub const UI_SETTINGS_AUTOSTART_BODY: &str =
    "DBLocal terdaftar sebagai login item milik pengguna ini: tanpa sudo, tanpa layanan sistem.";
/// Settings autostart: switch label.
pub const UI_SETTINGS_AUTOSTART_SWITCH: &str = "Jalankan DBLocal saat login";
/// Settings autostart: the item is registered.
pub const UI_SETTINGS_AUTOSTART_ON: &str = "Terdaftar untuk login.";
/// Settings autostart: the item is not registered.
pub const UI_SETTINGS_AUTOSTART_OFF: &str = "Tidak terdaftar untuk login.";
/// Settings autostart: the status could not be read.
pub const UI_SETTINGS_AUTOSTART_UNKNOWN: &str = "Status login item tidak diketahui.";
/// Settings autostart: where the login item lives.
pub const UI_SETTINGS_AUTOSTART_LOCATION: &str = "Lokasi";
/// Settings autostart: close action.
pub const UI_SETTINGS_CLOSE: &str = "Tutup";

/// Formats the "newer version available" line.
pub fn settings_available(version: &str) -> String {
    UI_SETTINGS_AVAILABLE.replace("{version}", version)
}
