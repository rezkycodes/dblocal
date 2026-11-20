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
