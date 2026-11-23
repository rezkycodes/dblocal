//! User-facing Indonesian strings.
//!
//! Every string the user can see lives here, exactly as the PRD writes it.
//! Workers append to this module; nothing else in the crate hardcodes a
//! user-facing sentence.

/// Environment variable that relocates the whole `~/.dblocal` tree.
pub const ENV_DBLOCAL_HOME: &str = "DBLOCAL_HOME";

// --- Error messages, one per `DblError` variant -------------------------------

/// `DblError::InvalidInput`.
pub const ERR_INVALID_INPUT: &str = "Nilainya tidak valid. Periksa kembali isian yang ditandai.";
/// `DblError::NotFound`.
pub const ERR_NOT_FOUND: &str = "Data yang dicari tidak ditemukan.";
/// `DblError::AlreadyExists`.
pub const ERR_ALREADY_EXISTS: &str = "Data dengan nama itu sudah ada.";
/// `DblError::InvalidTransition`.
pub const ERR_INVALID_TRANSITION: &str =
    "Tindakan itu tidak bisa dilakukan pada status instance saat ini.";
/// `DblError::PortInUse`.
pub const ERR_PORT_IN_USE: &str = "Port itu sedang dipakai proses lain.";
/// `DblError::ChecksumMismatch`.
pub const ERR_CHECKSUM_MISMATCH: &str =
    "Berkas unduhan rusak atau tidak cocok dengan checksum. Coba unduh ulang.";
/// `DblError::UnsafeArchive`.
pub const ERR_UNSAFE_ARCHIVE: &str = "Arsip ditolak karena berisi path yang tidak aman.";
/// `DblError::Io`.
pub const ERR_IO: &str = "Gagal membaca atau menulis berkas.";
/// `DblError::Network`.
pub const ERR_NETWORK: &str = "Koneksi jaringan gagal. Periksa koneksi lalu coba lagi.";
/// `DblError::Storage`.
pub const ERR_STORAGE: &str = "Gagal mengakses basis data lokal DBLocal.";
/// `DblError::Engine`.
pub const ERR_ENGINE: &str = "Engine database gagal dijalankan. Lihat log untuk detailnya.";
/// `DblError::Secret`.
pub const ERR_SECRET: &str = "Gagal menyimpan atau membaca kredensial.";
/// `DblError::Unsupported`.
pub const ERR_UNSUPPORTED: &str = "Fitur ini belum didukung di platform ini.";
/// `DblError::Cancelled`.
pub const ERR_CANCELLED: &str = "Tindakan dibatalkan.";
/// `DblError::NotImplemented`.
pub const ERR_NOT_IMPLEMENTED: &str = "Fitur ini belum tersedia di versi ini.";

// --- Catalog status banner ---------------------------------------------------

/// Shown when the cached catalog is older than its TTL.
pub const CATALOG_STALE: &str = "Katalog mungkin tidak terbaru";
/// Shown when the catalog could not be refreshed at all.
pub const CATALOG_OFFLINE: &str = "Katalog offline, memakai data terakhir";

/// Shown when an engine has no installable artifacts for the current platform.
pub const CATALOG_ENGINE_UNAVAILABLE_PLATFORM: &str = "Tidak tersedia di platform ini.";

// --- Instance and credential strings ----------------------------------------

/// Badge for an instance created without a password.
pub const BADGE_NO_AUTH: &str = "No auth";
/// Note shown when credentials live in a file instead of the OS keychain.
pub const CREDENTIALS_IN_FILE: &str = "Disimpan di file, bukan keychain";
/// Delete action that removes the instance and its data.
pub const DELETE_PURGE: &str = "Hapus instance & data";
/// Delete action that keeps an archive copy of the data.
pub const DELETE_ARCHIVE: &str = "Hapus instance, simpan data";
/// Diagnostics hint when the data dir was written by another engine version.
pub const HINT_VERSION_MISMATCH: &str = "Versi tidak cocok, butuh instance baru";
/// Diagnostics hint when the runtime binary is missing or corrupt.
pub const HINT_REINSTALL_RUNTIME: &str = "Pasang ulang runtime";

// --- UI chrome (Indonesian) -------------------------------------------------

/// App name shown in the title bar.
pub const APP_NAME: &str = "DBLocal";
/// Title bar button to create a new instance.
pub const UI_NEW_INSTANCE: &str = "Instance baru";
/// Welcome heading when no tab is open.
pub const UI_WELCOME_TITLE: &str = "Selamat datang di DBLocal";
/// Welcome subtitle.
pub const UI_WELCOME_SUBTITLE: &str = "Buat instance database lokal pertama Anda untuk mulai.";
/// Call to action on the welcome screen.
pub const UI_CREATE_FIRST: &str = "Buat instance pertama Anda";
/// Shown when instances exist but no tab is open.
pub const UI_SELECT_INSTANCE_TITLE: &str = "Pilih instance";
/// Subtitle for the select-instance empty state.
pub const UI_SELECT_INSTANCE_SUBTITLE: &str =
    "Pilih instance dari panel kiri untuk melihat detail.";
/// Instances panel header.
pub const UI_INSTANCES: &str = "INSTANCE";
/// Instances panel footer button.
pub const UI_NEW_INSTANCE_FOOTER: &str = "Instance baru";
/// Overview tab label.
pub const UI_TAB_OVERVIEW: &str = "Ringkasan";
/// Logs tab label.
pub const UI_TAB_LOGS: &str = "Log";
/// Status bar: running count label fragment.
pub const UI_RUNNING: &str = "berjalan";
/// Status bar: total label fragment.
pub const UI_TOTAL: &str = "total";
/// Status bar: catalog label.
pub const UI_CATALOG: &str = "Katalog";
/// Status bar: downloads label.
pub const UI_DOWNLOADS: &str = "Unduhan";
/// Status bar: no active downloads.
pub const UI_NO_DOWNLOADS: &str = "Tidak ada unduhan aktif";
/// Status bar: tray unavailable note.
pub const UI_TRAY_UNAVAILABLE: &str = "Tray tidak tersedia";
/// Status bar: tray available note.
pub const UI_TRAY_ACTIVE: &str = "Tray aktif";
/// Version prefix.
pub const UI_VERSION_PREFIX: &str = "v";
/// Command palette placeholder.
pub const UI_PALETTE_PLACEHOLDER: &str = "Cari instance atau jalankan perintah...";
/// Generic close label.
pub const UI_CLOSE: &str = "Tutup";
/// Generic cancel label.
pub const UI_CANCEL: &str = "Batal";
/// Generic confirm label.
pub const UI_CONFIRM: &str = "Konfirmasi";

// --- Instance actions -------------------------------------------------------

/// Start action.
pub const UI_START: &str = "Mulai";
/// Stop action.
pub const UI_STOP: &str = "Hentikan";
/// Copy URI action.
pub const UI_COPY_URI: &str = "Salin URI";
/// Copy .env action.
pub const UI_COPY_DOTENV: &str = "Salin .env";
/// Delete action label.
pub const UI_DELETE: &str = "Hapus";
/// Retry action.
pub const UI_RETRY: &str = "Coba lagi";
/// Change port action.
pub const UI_CHANGE_PORT: &str = "Pindah port";
/// Reveal password toggle.
pub const UI_SHOW_PASSWORD: &str = "Tampilkan sandi";
/// Hide password toggle.
pub const UI_HIDE_PASSWORD: &str = "Sembunyikan sandi";

// --- Toasts -----------------------------------------------------------------

/// Toast shown after a URI is copied.
pub const UI_TOAST_URI_COPIED: &str = "URI disalin";
/// Toast shown after a .env block is copied.
pub const UI_TOAST_DOTENV_COPIED: &str = ".env disalin";
/// Toast shown after log lines are copied.
pub const UI_TOAST_LOGS_COPIED: &str = "Log disalin";

// --- Instance status labels (shown with the status dot) ---------------------

/// Status label for a running instance.
pub const STATUS_RUNNING: &str = "berjalan";
/// Status label for a stopped instance.
pub const STATUS_STOPPED: &str = "berhenti";
/// Status label for a failed instance.
pub const STATUS_FAILED: &str = "gagal";
/// Status label while downloading/provisioning.
pub const STATUS_PROVISIONING: &str = "mengunduh";
/// Status label while initializing.
pub const STATUS_INITIALIZING: &str = "inisialisasi";
/// Status label while starting.
pub const STATUS_STARTING: &str = "memulai";
/// Status label while stopping.
pub const STATUS_STOPPING: &str = "menghentikan";
/// Status label while being deleted.
pub const STATUS_DELETING: &str = "menghapus";
/// Status label for a draft instance.
pub const STATUS_DRAFT: &str = "draf";

// --- Create instance dialog -------------------------------------------------

/// Dialog title for creating an instance.
pub const UI_CREATE_TITLE: &str = "Instance baru";
/// Label for the engine field.
pub const UI_FIELD_ENGINE: &str = "Engine";
/// Label for the version field.
pub const UI_FIELD_VERSION: &str = "Versi";
/// Label for the name field.
pub const UI_FIELD_NAME: &str = "Nama";
/// Label for the port field.
pub const UI_FIELD_PORT: &str = "Port";
/// Label for the auth mode field.
pub const UI_FIELD_AUTH: &str = "Autentikasi";
/// Auth mode: password (default).
pub const UI_AUTH_PASSWORD: &str = "Sandi (disarankan)";
/// Auth mode: no password.
pub const UI_AUTH_NONE: &str = "Tanpa sandi";
/// Warning shown when "no password" auth is selected.
pub const UI_AUTH_NONE_WARNING: &str =
    "Proses dan pengguna lain di mesin ini bisa terhubung tanpa kredensial.";
/// Create button label.
pub const UI_CREATE_SUBMIT: &str = "Buat instance";
/// Version badge: already installed locally.
pub const UI_VERSION_INSTALLED: &str = "Terpasang";
/// Version badge: end of life.
pub const UI_VERSION_EOL: &str = "EOL";

// --- Delete dialog (D-18) ---------------------------------------------------

/// Delete dialog title.
pub const UI_DELETE_TITLE: &str = "Hapus instance";
/// Delete dialog prompt showing the instance name.
pub const UI_DELETE_PROMPT: &str = "Pilih apa yang terjadi pada data instance ini.";
/// Data size label in the delete dialog.
pub const UI_DELETE_SIZE: &str = "Ukuran data";
/// Instruction to type the name to confirm purge.
pub const UI_DELETE_TYPE_NAME: &str = "Ketik nama instance untuk mengonfirmasi penghapusan data.";
/// Placeholder for the name confirmation field.
pub const UI_DELETE_NAME_PLACEHOLDER: &str = "Nama instance";

// --- Failure / diagnostics (F-LOG-02) ---------------------------------------

/// Heading shown in a failure state.
pub const UI_FAILURE_HEADING: &str = "Instance gagal dimulai";
/// Label for the stable error code.
pub const UI_FAILURE_CODE: &str = "Kode";
/// Label for the diagnosis section.
pub const UI_FAILURE_DIAGNOSIS: &str = "Diagnosa";
/// Label for the recent log tail.
pub const UI_FAILURE_LOG_TAIL: &str = "Log terakhir";

// --- Logs -------------------------------------------------------------------

/// Follow toggle label when live.
pub const UI_LOGS_FOLLOWING: &str = "Mengikuti";
/// Follow toggle label when paused.
pub const UI_LOGS_PAUSED: &str = "Dijeda";
/// Copy logs action.
pub const UI_LOGS_COPY: &str = "Salin log";
/// Empty logs placeholder.
pub const UI_LOGS_EMPTY: &str = "Belum ada log.";

// --- Overview ---------------------------------------------------------------

/// Host label.
pub const UI_OVERVIEW_HOST: &str = "Host";
/// Port label.
pub const UI_OVERVIEW_PORT: &str = "Port";
/// Version label.
pub const UI_OVERVIEW_VERSION: &str = "Versi";
/// URI label.
pub const UI_OVERVIEW_URI: &str = "URI";
/// Status label.
pub const UI_OVERVIEW_STATUS: &str = "Status";

// --- Palette actions --------------------------------------------------------

/// Palette: create a new instance.
pub const UI_PALETTE_NEW: &str = "Instance baru";
/// Palette: start an instance (prefix).
pub const UI_PALETTE_START: &str = "Mulai";
/// Palette: stop an instance (prefix).
pub const UI_PALETTE_STOP: &str = "Hentikan";
/// Palette: copy URI for an instance (prefix).
pub const UI_PALETTE_COPY_URI: &str = "Salin URI";
/// Palette: open an instance tab (prefix).
pub const UI_PALETTE_OPEN: &str = "Buka";

// --- Validation -------------------------------------------------------------

/// Validation: the chosen port is below the allowed minimum.
pub const UI_VALIDATE_PORT_LOW: &str =
    "Port di bawah 1024 butuh privilese; DBLocal tidak pernah meminta sudo.";
/// Validation: instance name is invalid.
pub const UI_VALIDATE_NAME: &str =
    "Nama harus 2-31 karakter, huruf kecil/angka/tanda hubung, diawali huruf atau angka.";

/// Validation: the typed port already belongs to another instance.
///
/// `{port}` and `{name}` are placeholders the GUI fills in.
pub const UI_VALIDATE_PORT_TAKEN: &str = "Port {port} sudah dipakai instance {name}";

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
    "DBLocal tidak bisa dijalankan karena Vulkan tidak tersedia di sistem ini.";
/// Vulkan pre-flight detected-reason prefix.
pub const VULKAN_DETECTED: &str = "Terdeteksi";
/// Vulkan pre-flight explanation.
pub const VULKAN_WHY: &str =
    "Sebab: antarmuka DBLocal memakai GPUI, yang membutuhkan driver/ICD Vulkan.";
/// Vulkan pre-flight fix heading.
pub const VULKAN_FIX_HEADING: &str = "Cara memperbaiki (pasang driver Vulkan untuk GPU Anda):";
/// Vulkan pre-flight verification heading.
pub const VULKAN_VERIFY: &str = "Verifikasi setelah memasang";
/// Vulkan pre-flight help link prefix.
pub const VULKAN_MORE_HELP: &str = "Bantuan lebih lanjut";
/// Vulkan pre-flight closing hint.
pub const VULKAN_HINT: &str = "Jika driver sudah terpasang tetapi DBLocal tetap gagal, pastikan ada berkas ICD JSON di \
     /usr/share/vulkan/icd.d dan VK_ICD_FILENAMES/VK_DRIVER_FILES tidak menunjuk path yang salah.";
/// Vulkan pre-flight exit-code line prefix.
pub const VULKAN_EXIT_CODE: &str = "Kode keluar";

// --- Human descriptions for machine slugs shown in the UI ---------------------

/// Catalog freshness: fresh.
pub const CATALOG_FRESH_LABEL: &str = "Terbaru";
/// Catalog freshness: stale.
pub const CATALOG_STALE_LABEL: &str = "Kedaluwarsa";
/// Catalog freshness: offline.
pub const CATALOG_OFFLINE_LABEL: &str = "Offline";

/// Start diagnosis: the port was already bound.
pub const DIAGNOSIS_PORT_IN_USE: &str = "Port sedang dipakai proses lain";
/// Start diagnosis: the data directory belongs to another engine version.
pub const DIAGNOSIS_VERSION_MISMATCH: &str = "Versi tidak cocok, butuh instance baru";
/// Start diagnosis: a path could not be read or written.
pub const DIAGNOSIS_PERMISSION_DENIED: &str = "Izin ditolak pada path data";
/// Start diagnosis: the engine binary is missing or corrupt.
pub const DIAGNOSIS_BINARY_MISSING: &str = "Biner engine hilang atau rusak";
/// Start diagnosis: nothing the app recognises (see the log).
pub const DIAGNOSIS_UNKNOWN: &str = "Penyebab tidak dikenali, lihat log";

/// Toast shown when a stop had to escalate to a forced kill (F-INS-03 AC3):
/// the engine ignored the graceful shutdown within the grace period.
pub const UI_TOAST_STOP_FORCED: &str = "Instance mengabaikan stop normal, jadi dihentikan paksa";

// --- Workspace chrome (D-25 prototype port) ---------------------------------

/// Breadcrumb separator between the engine and the instance name.
pub const UI_BREADCRUMB_SEPARATOR: &str = "›";
/// Dock section header above the engine groups.
pub const UI_DOCK_INSTANCES: &str = "INSTANCES";
/// Dock section header above the installed runtimes.
pub const UI_DOCK_RUNTIMES: &str = "RUNTIMES";
/// Filter input placeholder in the dock.
pub const UI_DOCK_FILTER: &str = "Filter…";
/// Placeholder when the dock filter matches nothing.
pub const UI_DOCK_NO_MATCHES: &str = "Tidak ada instance yang cocok.";
/// Runtimes list: this runtime is referenced by no instance (F-RTM-04).
pub const UI_RUNTIME_UNUSED: &str = "tak terpakai";
/// Runtimes list: how many instances share a runtime.
pub const UI_RUNTIME_INSTANCE_COUNT: &str = "inst.";
/// Section heading on the Overview tab for the connection card.
pub const UI_OVERVIEW_CONNECTION: &str = "Koneksi";
/// Section heading on the Overview tab for the detail card.
pub const UI_OVERVIEW_DETAIL: &str = "Detail";
/// Section heading on the Overview tab for the process card.
pub const UI_OVERVIEW_PROCESS: &str = "Proses";
/// Section heading on the Overview tab for the recent-log card.
pub const UI_OVERVIEW_RECENT_LOGS: &str = "Log terbaru";
/// Acknowledge the local-only bind (F-INS-02 AC4).
pub const UI_OVERVIEW_LOOPBACK_ONLY: &str = "Hanya mendengarkan 127.0.0.1 dan ::1";
/// Note that the connection URI is available although the instance is stopped.
pub const UI_OVERVIEW_URI_STOPPED_NOTE: &str =
    "URI tetap bisa disalin, tetapi instance belum berjalan.";
/// Label for the `.env` block preview.
pub const UI_OVERVIEW_DOTENV: &str = "# .env";
/// Detail row label: engine.
pub const UI_OVERVIEW_ENGINE: &str = "Engine";
/// Detail row label: superuser.
pub const UI_OVERVIEW_SUPERUSER: &str = "Superuser";
/// Detail row label: data directory.
pub const UI_OVERVIEW_DATA_DIR: &str = "Data dir";
/// Detail row label: data size.
pub const UI_OVERVIEW_DATA_SIZE: &str = "Ukuran data";
/// Marker shown next to an instance on the engine's canonical port.
pub const UI_OVERVIEW_CANONICAL_PORT: &str = "(kanonik)";
/// Process row label: PID.
pub const UI_OVERVIEW_PID: &str = "PID";
/// Process row label: health check.
pub const UI_OVERVIEW_HEALTH: &str = "Health check";
/// Health value when the protocol probe passed.
pub const UI_OVERVIEW_HEALTH_OK: &str = "OK · protokol handshake";
/// Health value when the last start failed.
pub const UI_OVERVIEW_HEALTH_FAILED: &str = "Gagal — lihat Logs";
/// Health value when the instance is not running.
pub const UI_OVERVIEW_HEALTH_OFFLINE: &str = "Tidak berjalan";
/// Process row label: process mode.
pub const UI_OVERVIEW_PROCESS_MODE: &str = "Mode proses";
/// Process mode value: spawned detached, survives an app quit (D-10).
pub const UI_OVERVIEW_PROCESS_DETACHED: &str = "Detached · tetap hidup saat app ditutup";
/// Process row label: credential reference.
pub const UI_OVERVIEW_CREDENTIAL: &str = "Kredensial";
/// Autostart checkbox label.
pub const UI_OVERVIEW_AUTOSTART: &str = "Jalankan otomatis saat login";
/// Link from the Overview card to the Logs tab.
pub const UI_OVERVIEW_GOTO_LOGS: &str = "Buka tab Logs";
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
pub const UI_ACTION_CLOSE_TAB: &str = "Tutup tab";

// --- Status bar (D-25) -------------------------------------------------------

/// Status bar: stopped count fragment.
pub const UI_STOPPED: &str = "berhenti";
/// Status bar: failed count fragment.
pub const UI_FAILED: &str = "gagal";
/// Status bar: the data directory label.
pub const UI_DATA_DIR: &str = "Direktori data";
/// Status bar: the credential backend label.
pub const UI_CREDENTIAL_BACKEND: &str = "Kredensial";
/// Status bar: credential backend name when the OS keychain is in use.
pub const UI_CREDENTIAL_KEYCHAIN: &str = "Keychain";
/// Status bar: credential backend name when the `0600` file fallback is in use.
pub const UI_CREDENTIAL_FILE: &str = "File (0600)";

// --- Downloads (F-RTM-01) ----------------------------------------------------

/// Active-download banner heading.
pub const UI_DOWNLOAD_HEADING: &str = "Menyiapkan runtime";
/// Cancel an active download.
pub const UI_DOWNLOAD_CANCEL: &str = "Batal";
/// Copy logs from the log viewer.
pub const UI_LOGS_SHOW_FILE: &str = "engine.log";
/// Log footer: rotation policy (F-LOG-01 AC2).
pub const UI_LOGS_ROTATION: &str = "rotasi maks 3 × 10 MB";
/// Log line count suffix.
pub const UI_LOGS_LINE_COUNT: &str = "baris";
/// Logs tab: follow-mode toggle hint.
pub const UI_LOGS_FOLLOW_HINT: &str = "Ikuti baris baru";

// --- Diagnose banner (F-LOG-02) ---------------------------------------------

/// One-click fix heading for a port conflict (F-PRT-02 AC1).
pub const UI_DIAGNOSIS_PORT_HEADING: &str = "Port {port} sedang dipakai";
/// Explanation under the port-conflict heading.
pub const UI_DIAGNOSIS_PORT_BODY: &str = "Uji bind ke 127.0.0.1:{port} gagal sebelum engine dijalankan. Port ditempati proses di luar DBLocal. Port bebas terdekat: {suggested}.";
/// One-click fix action: move to the next port and start (F-PRT-02 AC2).
pub const UI_DIAGNOSIS_MOVE_AND_START: &str = "Pindah ke {port} & start";
/// Report-issue action for an unrecognised failure (F-LOG-02 AC2).
pub const UI_DIAGNOSIS_REPORT: &str = "Laporkan issue";
/// Toast after the issue template was prepared.
pub const UI_DIAGNOSIS_REPORT_READY: &str =
    "Template issue disiapkan tanpa password dan tanpa path home";

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

// --- Placeholder tabs (D-25: honest, never sample rows in the product) ------

/// Databases tab, product build: the backend lands in Phase 2 (F-DBU).
pub const UI_PLACEHOLDER_DATABASES: &str = "Kelola database dan user belum tersedia di versi ini.";
/// Config tab, product build: the backend lands in Phase 3 (F-CFG).
pub const UI_PLACEHOLDER_CONFIG: &str = "Editor konfigurasi belum tersedia di versi ini.";
/// "Open in…" menu, product build: the backend lands in Phase 2 (F-OPN).
pub const UI_PLACEHOLDER_OPEN_IN: &str = "Membuka di klien eksternal belum tersedia di versi ini.";
/// Phase label appended to a placeholder.
pub const UI_PLACEHOLDER_PHASE: &str = "Fase {phase}";
/// The line that explains what a placeholder tab will contain.
pub const UI_PLACEHOLDER_PREVIEW: &str = "Pratinjau tata letak memakai data contoh (mode demo).";

/// Formats a phase label.
pub fn placeholder_phase(phase: &str) -> String {
    UI_PLACEHOLDER_PHASE.replace("{phase}", phase)
}

// --- Databases tab (demo content, mock build only) --------------------------

/// Databases table heading.
pub const UI_DATABASES_HEADING: &str = "Database";
/// Databases table column: name.
pub const UI_DATABASES_COLUMN_NAME: &str = "Nama";
/// Databases table column: encoding.
pub const UI_DATABASES_COLUMN_ENCODING: &str = "Encoding";
/// Databases table column: size.
pub const UI_DATABASES_COLUMN_SIZE: &str = "Ukuran";
/// Databases table: default encoding values.
pub const UI_DATABASES_ENCODING: &str = "UTF8 · C.UTF-8";
/// Databases tab: create action (F-DBU-01).
pub const UI_DATABASES_CREATE: &str = "Buat database";
/// Databases tab: the name field placeholder.
pub const UI_DATABASES_NAME_PLACEHOLDER: &str = "nama_database";
/// Databases tab: delete action.
pub const UI_DATABASES_DELETE: &str = "Hapus database (ketik nama untuk konfirmasi)";
/// Databases tab: validation failure for a bad name (F-DBU-01 AC4).
pub const UI_DATABASES_INVALID_NAME: &str = "Nama hanya boleh huruf, angka, dan garis bawah.";
/// Databases tab: heading above the user list.
pub const UI_DATABASES_USERS: &str = "User & hak akses";
/// Databases tab: new user action.
pub const UI_DATABASES_NEW_USER: &str = "User baru";
/// Databases tab: the boundary note that DBLocal manages servers, not data.
pub const UI_DATABASES_BOUNDARY: &str = "DBLocal mengelola server, bukan isi data. Untuk menjelajah tabel, buka instance di client Anda.";
/// Databases tab: superuser password card heading.
pub const UI_DATABASES_PASSWORD_HEADING: &str = "Password superuser";
/// Databases tab: superuser password card body.
pub const UI_DATABASES_PASSWORD_BODY: &str =
    "Membuat password baru dan memperbarui URI. Bila gagal, password lama tetap berlaku.";
/// Databases tab: reset-password action (F-DBU-03).
pub const UI_DATABASES_RESET: &str = "Reset password";
/// Databases tab: toast after a database is created.
pub const UI_DATABASES_CREATED: &str = "Database {name} dibuat (UTF-8)";
/// Databases tab: toast after the superuser password is reset.
pub const UI_DATABASES_PASSWORD_RESET: &str = "Password baru tersimpan di Keychain";
/// Databases tab: why the list is empty while the instance is not running.
pub const UI_DATABASES_STOPPED: &str = "Jalankan instance untuk melihat dan mengelola database.";

/// Formats the database-created toast.
pub fn databases_created(name: &str) -> String {
    UI_DATABASES_CREATED.replace("{name}", name)
}

// --- Config tab (demo content, mock build only) -----------------------------

/// Config table heading.
pub const UI_CONFIG_HEADING: &str = "Setting penting";
/// Config table column note.
pub const UI_CONFIG_COLUMNS: &str = "Nilai saat ini · Default";
/// Config table: changed badge.
pub const UI_CONFIG_CHANGED: &str = "diubah";
/// Config banner after saving (F-CFG-01 AC2).
pub const UI_CONFIG_RESTART_REQUIRED: &str =
    "Tersimpan. Restart diperlukan agar perubahan berlaku.";
/// Config banner: restart later.
pub const UI_CONFIG_LATER: &str = "Nanti";
/// Config banner: restart now.
pub const UI_CONFIG_RESTART_NOW: &str = "Restart sekarang";
/// Config footer: open the config file (F-CFG-02).
pub const UI_CONFIG_OPEN_FILE: &str = "Buka file konfigurasi";
/// Config footer: revert to the previous file (F-CFG-01 AC3).
pub const UI_CONFIG_REVERT: &str = "Kembalikan konfigurasi sebelumnya";
/// Config footer: discard changes.
pub const UI_CONFIG_DISCARD: &str = "Buang perubahan";
/// Config footer: save changes.
pub const UI_CONFIG_SAVE: &str = "Simpan";

// --- New instance dialog (D-25 prototype layout) ----------------------------

/// Intro line at the top of the create dialog.
pub const UI_CREATE_INTRO: &str =
    "Runtime diunduh dari upstream dan diverifikasi SHA-256 sebelum dipasang.";
/// Section heading for the version list.
pub const UI_CREATE_VERSIONS: &str = "Versi · {engine}";
/// Badge on a catalog version that is not installed yet.
pub const UI_CREATE_DOWNLOAD_BADGE: &str = "Unduh, {size}";
/// Inline hint under the name field.
pub const UI_CREATE_NAME_HINT: &str = "huruf kecil, angka, tanda hubung";
/// Inline hint under the port field.
pub const UI_CREATE_PORT_HINT: &str = "port bebas berikutnya";
/// Inline hint under the superuser field.
pub const UI_CREATE_SUPERUSER_HINT: &str = "password → Keychain";
/// Footer note when the chosen runtime is already installed.
pub const UI_CREATE_READY_NOTE: &str = "Runtime sudah terpasang — siap dalam < 10 detik";
/// Footer note when a download is needed (F-INS-01 AC2).
pub const UI_CREATE_DOWNLOAD_NOTE: &str = "Akan mengunduh runtime lalu verifikasi SHA-256";
/// Submit button label in the create dialog (prototype wording).
pub const UI_CREATE_SUBMIT_PROTOTYPE: &str = "Create";
/// The no-password option label.
pub const UI_CREATE_NO_PASSWORD: &str = "Izinkan koneksi tanpa password";
/// The full warning under the no-password option (F-CRD-03 AC1).
pub const UI_CREATE_NO_PASSWORD_WARNING: &str = "Proses dan user lain di mesin ini bisa terhubung tanpa kredensial. Tidak bisa diubah setelah dibuat.";

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
pub const UI_DELETE_ARCHIVE_HINT: &str = "Data dipindah ke arsip, instance dihapus.";
/// Delete dialog: purge mode removes the data too.
pub const UI_DELETE_PURGE_HINT: &str = "Instance dan seluruh datanya dihapus permanen.";

// --- Palette hints -----------------------------------------------------------

/// Palette row: focus an open instance tab.
pub const UI_PALETTE_FOCUS: &str = "Fokus";

// --- Toast after an operation that changed state -----------------------------

/// Toast after a start request was accepted.
pub const UI_TOAST_START_REQUESTED: &str = "Permintaan start dikirim";
/// Toast after a stop request was accepted.
pub const UI_TOAST_STOP_REQUESTED: &str = "Permintaan stop dikirim";
/// Toast after an instance was deleted.
pub const UI_TOAST_DELETED: &str = "Instance dihapus";
/// Toast after an autostart toggle.
pub const UI_TOAST_AUTOSTART_ON: &str = "Instance akan berjalan saat login";
/// Toast after an autostart toggle was cleared.
pub const UI_TOAST_AUTOSTART_OFF: &str = "Autostart dimatikan";

// --- Window controls (per platform, as the prototype draws them) -----------

/// Title bar: minimise the window.
pub const UI_WINDOW_MINIMIZE: &str = "Perkecil";
/// Title bar: maximise the window.
pub const UI_WINDOW_MAXIMIZE: &str = "Perbesar";
/// Title bar: close the window.
pub const UI_WINDOW_CLOSE: &str = "Tutup jendela";

// --- Tray and quit (F-TRY-01, F-TRY-02) ------------------------------------

/// Tray menu item that brings the main window to the front.
pub const UI_TRAY_OPEN: &str = "Buka DBLocal";
/// Tray menu item that quits the app.
pub const UI_TRAY_QUIT: &str = "Keluar";
/// Status-bar note shown once when no tray host is available.
pub const UI_TRAY_UNAVAILABLE_NOTE: &str = "Tray tidak tersedia di lingkungan desktop ini.";
/// Quit dialog heading.
pub const UI_QUIT_TITLE: &str = "Keluar dari DBLocal?";
/// Quit dialog body shown when instances are still running.
pub const UI_QUIT_BODY: &str =
    "Masih ada instance yang berjalan. Pilih apa yang harus terjadi saat keluar.";
/// Quit dialog: keep engines running (default).
pub const UI_QUIT_KEEP_RUNNING: &str = "Biarkan tetap berjalan";
/// Quit dialog: stop every running instance before quitting.
pub const UI_QUIT_STOP_ALL: &str = "Hentikan semua lalu keluar";
/// Quit dialog: a stop failed but the user can force quit anyway.
pub const UI_QUIT_STOP_FAILED: &str = "Sebagian instance gagal dihentikan.";
/// Quit dialog: confirm quitting while stops are still pending.
pub const UI_QUIT_FORCE_CONFIRM: &str =
    "Masih ada instance yang sedang dihentikan. Keluar sekarang?";
/// Quit dialog: progress line while stopping all instances.
pub const UI_QUIT_STOPPING: &str = "Menghentikan instance...";
/// Neutral status-bar note while a background service call is in flight.
pub const UI_PENDING: &str = "Memuat...";
/// Startup failure window title.
pub const UI_STARTUP_FAILED_TITLE: &str = "DBLocal tidak bisa dimulai";
/// Startup failure window fallback body.
pub const UI_STARTUP_FAILED_BODY: &str =
    "DBLocal tidak bisa membaca atau menyiapkan data lokalnya.";

/// Placeholders the UI renders while no backend exists (D-25). Each entry names
/// the phase that fills it, so `docs/architecture/gui.md` and the GUI agree.
///
/// `Databases` and `Open in…` are gone: `F-DBU` and `F-OPN` now have their
/// service calls. Only the Config editor is still a placeholder (`F-CFG`).
pub const PLACEHOLDERS: [(&str, &str); 1] = [("Config", "F-CFG · Phase 3")];

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
