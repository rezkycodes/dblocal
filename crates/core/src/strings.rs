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
