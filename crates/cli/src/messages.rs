//! User-facing CLI strings (Indonesian, per AGENTS.md). Flags stay English.
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
pub const ERR_TIMEOUT: &str = "waktu tunggu habis";
/// Code for a failure to build the CLI's own async runtime.
pub const CODE_RUNTIME: &str = "DBL-8402";
/// Message for [`CODE_RUNTIME`].
pub const ERR_RUNTIME: &str = "gagal menyiapkan runtime CLI";

/// Label in front of a start failure's diagnosis.
pub const LABEL_DIAGNOSIS: &str = "diagnosis:";
/// Prefix of a download-progress line.
pub const PROGRESS_DOWNLOAD: &str = "... unduhan";
/// Prefix of an operation-stage line.
pub const PROGRESS_STAGE: &str = "...";

pub const ERR_SERVICE_OPEN: &str = "Gagal membuka layanan DBLocal.";
pub const ERR_INSTANCE_NOT_FOUND: &str = "Instance tidak ditemukan";
pub const ERR_ALREADY_RUNNING: &str = "sudah berjalan";
pub const ERR_STOPPING_ELSEWHERE: &str = "sedang dihentikan oleh proses lain";
pub const ERR_NAME_CONFIRMATION: &str = "Penghapusan dibatalkan: nama instance tidak cocok.";
pub const ERR_DELETE_NEEDS_YES: &str = "Penghapusan memerlukan --yes pada sesi non-interaktif.";
pub const ERR_UNKNOWN_ENGINE: &str = "Engine tidak dikenal.";
pub const ERR_CONFIRM_READ: &str = "gagal membaca konfirmasi";
pub const ERR_CREATE_MISSING: &str = "instance hilang setelah create";
pub const MSG_ARCHIVED_TO: &str = "Data diarsipkan ke";
pub const MSG_DELETED: &str = "Instance dihapus.";
pub const MSG_CREATING: &str = "Membuat instance";
pub const MSG_STARTING: &str = "Menjalankan instance";
pub const MSG_STOPPING: &str = "Menghentikan instance";
pub const MSG_ALREADY_STOPPED: &str = "sudah berhenti";
pub const MSG_READY: &str = "Siap.";
pub const PROMPT_DELETE: &str = "Ketik nama instance untuk konfirmasi penghapusan: ";

/// Reply to `autostart status`/`enable` when the login item exists.
pub const AUTOSTART_ENABLED: &str = "autostart: aktif";
/// Reply to `autostart status`/`disable` when the login item is absent.
pub const AUTOSTART_DISABLED: &str = "autostart: nonaktif";
/// Label in front of the login-item location.
pub const AUTOSTART_LOCATION: &str = "lokasi";
/// Unknown `autostart` action (English noun kept for the parser contract).
pub const ERR_AUTOSTART_ACTION: &str = "Aksi autostart tidak dikenal";
/// The running executable path could not be resolved for `autostart enable`.
pub const ERR_AUTOSTART_EXE: &str = "lokasi aplikasi tidak bisa ditentukan";

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
