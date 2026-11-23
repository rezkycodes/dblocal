//! Config tab strings (`F-CFG-01`, `F-CFG-02`), live over `DblService`.
//!
//! Re-exported from `strings` so call sites stay `strings::UI_CONFIG_...`. The
//! service's own messages live in `crate::config::messages`; these are only the
//! chrome the view draws. Names avoid the placeholder strings still in
//! `strings/mod.rs` (`UI_CONFIG_HEADING`, `UI_CONFIG_COLUMNS`, `UI_CONFIG_SAVE`).

// --- Card and columns -------------------------------------------------------

/// Config card heading (PRD F-CFG-01 AC1: one card "Pengaturan").
pub const UI_CONFIG_CARD_TITLE: &str = "Pengaturan";
/// The settings are being read.
pub const UI_CONFIG_LOADING: &str = "Membaca konfigurasi…";
/// No editable or read-only settings were returned.
pub const UI_CONFIG_EMPTY: &str = "Belum ada setelan untuk instance ini.";
/// A read of the config failed; the card offers a retry.
pub const UI_CONFIG_RETRY: &str = "Muat ulang";

// --- One row ----------------------------------------------------------------

/// Default value shown next to the field (`Bawaan: 100`).
pub const UI_CONFIG_DEFAULT_PREFIX: &str = "Bawaan: ";
/// Word shown for a key the file does not set, so the default is in effect.
pub const UI_CONFIG_UNSET_NOTE: &str = "bawaan";
/// Marker that an applied value needs a restart (text, never colour alone).
pub const UI_CONFIG_RESTART_MARKER: &str = "perlu restart";
/// Unit hint shown after the field when the view carries one.
pub const UI_CONFIG_UNIT_PREFIX: &str = "satuan: ";
/// Why `port` is read-only here (it goes through the Ubah port action).
pub const UI_CONFIG_READONLY_PORT: &str = "Ubah port lewat tindakan Ubah port";
/// Why the PostgreSQL authentication method is read-only here.
pub const UI_CONFIG_READONLY_MANAGED: &str = "Dikelola DBLocal, tidak dapat diubah di sini";

// --- Actions and results ----------------------------------------------------

/// Apply the edited settings ("Terapkan").
pub const UI_CONFIG_APPLY: &str = "Terapkan";
/// Discard the local edits and go back to the file's values.
pub const UI_CONFIG_CANCEL_CHANGES: &str = "Batalkan perubahan";
/// Shown after a successful apply (F-CFG-01 AC2).
pub const UI_CONFIG_APPLIED: &str = "Tersimpan. Berlaku setelah restart.";
/// The banner that a config file changed on disk while the tab is open.
pub const UI_CONFIG_CHANGED_OUTSIDE: &str = "File berubah di luar aplikasi";
/// Copy the raw path to the clipboard.
pub const UI_CONFIG_COPY_PATH: &str = "Salin";
/// Toast after copying the path.
pub const UI_CONFIG_PATH_COPIED: &str = "Path konfigurasi disalin ke clipboard";
/// A restore done in the background.
pub const UI_CONFIG_RESTORED: &str = "Konfigurasi dikembalikan dari salinan terbaru.";

// --- Restore confirmation ---------------------------------------------------

/// Restore dialog title (F-CFG-01 AC3).
pub const UI_CONFIG_RESTORE_TITLE: &str = "Pulihkan konfigurasi sebelumnya";
/// Restore action on the card.
pub const UI_CONFIG_RESTORE: &str = "Pulihkan konfigurasi sebelumnya";
/// Restore dialog body; `{time}` is the newest backup's time.
pub const UI_CONFIG_RESTORE_PROMPT: &str = "Berkas saat ini disimpan sebagai salinan terbaru, lalu konfigurasi dari {time} dikembalikan. Restart diperlukan agar berlaku.";
/// Restore dialog confirm action.
pub const UI_CONFIG_RESTORE_CONFIRM: &str = "Pulihkan";
/// The one-click failure-banner action (F-CFG-01 AC3).
pub const UI_CONFIG_RESTORE_AND_RESTART: &str = "Pulihkan konfigurasi dan mulai ulang";
/// The failure banner's short body: why a restore is offered.
pub const UI_CONFIG_RESTORE_BANNER_BODY: &str = "Berkas konfigurasi berubah sejak start terakhir yang berhasil. Kembalikan salinan sebelum perubahan lalu jalankan ulang.";
/// The backup time, as an age: `{n} detik lalu`.
pub const UI_CONFIG_AGE_SECONDS: &str = "{n} detik lalu";
/// The backup time, as an age: `{n} menit lalu`.
pub const UI_CONFIG_AGE_MINUTES: &str = "{n} menit lalu";
/// The backup time, as an age: `{n} jam lalu`.
pub const UI_CONFIG_AGE_HOURS: &str = "{n} jam lalu";
/// The backup time, as an age: `{n} hari lalu`.
pub const UI_CONFIG_AGE_DAYS: &str = "{n} hari lalu";
/// The backup time when the clock is behind the backup's stamp.
pub const UI_CONFIG_AGE_NOW: &str = "baru saja";

/// Formats the default-value note next to a field.
pub fn config_default(default: &str) -> String {
    if default.is_empty() {
        UI_CONFIG_DEFAULT_PREFIX.trim_end().to_owned()
    } else {
        format!("{UI_CONFIG_DEFAULT_PREFIX}{default}")
    }
}

/// Formats the unit note when the view carries a unit.
pub fn config_unit(unit: &str) -> Option<String> {
    if unit.is_empty() {
        None
    } else {
        Some(format!("{UI_CONFIG_UNIT_PREFIX}{unit}"))
    }
}

/// Formats the restore prompt with the backup time.
pub fn config_restore_prompt(time: &str) -> String {
    UI_CONFIG_RESTORE_PROMPT.replace("{time}", time)
}

/// Formats a backup's age from its Unix stamp, given the current Unix time.
///
/// A backup is shown as "12 menit lalu", never a raw epoch number; a stamp in
/// the future (a clock skew) reads "baru saja" rather than a negative age.
pub fn config_backup_age(taken_at_secs: u64, now_secs: u64) -> String {
    let age = now_secs.saturating_sub(taken_at_secs);
    if age < 60 {
        if age == 0 {
            return UI_CONFIG_AGE_NOW.to_owned();
        }
        return UI_CONFIG_AGE_SECONDS.replace("{n}", &age.to_string());
    }
    let minutes = age / 60;
    if minutes < 60 {
        return UI_CONFIG_AGE_MINUTES.replace("{n}", &minutes.to_string());
    }
    let hours = minutes / 60;
    if hours < 24 {
        return UI_CONFIG_AGE_HOURS.replace("{n}", &hours.to_string());
    }
    let days = hours / 24;
    UI_CONFIG_AGE_DAYS.replace("{n}", &days.to_string())
}
