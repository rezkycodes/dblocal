//! Indonesian user-facing messages for autostart (F-AUT-01, F-AUT-02).
//!
//! Lives inside the module so no shared file (`strings.rs`) has to change.
//! No message here contains a path that could hold a secret; the executable
//! path is shown only in the diagnostics the user asked for.

/// The platform has no user-level login mechanism DBLocal can use.
pub const ERR_UNSUPPORTED: &str = "Menjalankan otomatis saat login belum didukung di platform ini.";
/// Writing or removing the login item file failed.
pub const ERR_WRITE_FAILED: &str = "Gagal menulis berkas autostart untuk pengguna ini.";
/// A helper command (`systemctl --user`, `launchctl`, `reg`) failed.
pub const ERR_COMMAND_FAILED: &str = "Perintah sistem untuk autostart gagal dijalankan.";
/// The running executable path is unusable as a login command.
pub const ERR_INVALID_EXE: &str = "Lokasi aplikasi tidak bisa dipakai untuk autostart.";
/// The login item state could not be read.
pub const ERR_STATUS_FAILED: &str = "Gagal membaca status autostart saat login.";
/// Starting flagged instances could not be scheduled.
pub const ERR_PLAN_FAILED: &str = "Sebagian instance tidak bisa dijadwalkan untuk autostart.";

/// `--autostart` ran headless with nothing left to do: said before exiting.
pub const MODE_EXIT_NO_INSTANCES: &str =
    "Autostart: tidak ada instance yang berjalan, DBLocal keluar tanpa jendela.";
/// `--autostart` ran headless with at least one instance started.
pub const MODE_HEADLESS_KEEP: &str =
    "Autostart: tray tidak tersedia; instance tetap berjalan tanpa jendela.";
