//! User-facing CLI strings (Indonesian, per AGENTS.md). Flags stay English.

pub const ERR_SERVICE_OPEN: &str = "Gagal membuka layanan DBLocal.";
pub const ERR_INSTANCE_NOT_FOUND: &str = "Instance tidak ditemukan.";
pub const ERR_ALREADY_RUNNING: &str = "sudah berjalan";
pub const ERR_NAME_CONFIRMATION: &str = "Penghapusan dibatalkan: nama instance tidak cocok.";
pub const ERR_DELETE_NEEDS_YES: &str = "Penghapusan memerlukan --yes pada sesi non-interaktif.";
pub const ERR_UNKNOWN_ENGINE: &str = "Engine tidak dikenal.";
pub const MSG_ARCHIVED_TO: &str = "Data diarsipkan ke";
pub const MSG_DELETED: &str = "Instance dihapus.";
pub const MSG_CREATING: &str = "Membuat instance";
pub const MSG_STARTING: &str = "Menjalankan instance";
pub const MSG_STOPPING: &str = "Menghentikan instance";
pub const MSG_READY: &str = "Siap.";
pub const PROMPT_DELETE: &str = "Ketik nama instance untuk konfirmasi penghapusan: ";

/// Formats the service-open failure with a stable-looking diagnostic.
pub fn service_open_error(detail: &str) -> String {
    format!("{ERR_SERVICE_OPEN} {detail}")
}
