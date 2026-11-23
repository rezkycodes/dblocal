//! Indonesian user-facing messages for the config editor (`DBL-86xx`).
//!
//! Every string is static, so a message can never scream a value the user typed
//! (which could be a password for `requirepass`, though that key is not editable
//! here). Descriptions name the setting's purpose, never its current value.

/// `DBL-8600`: the key is not editable for this engine.
pub const ERR_UNKNOWN_KEY: &str =
    "Setelan itu tidak ada di daftar yang bisa diubah untuk engine ini.";
/// `DBL-8602`: the key is managed by DBLocal.
pub const ERR_MANAGED_KEY: &str =
    "Setelan itu dikelola DBLocal sendiri. Gunakan tindakan yang sesuai (misalnya ubah port).";
/// `DBL-8603`: the file could not be read or written.
pub const ERR_IO: &str = "Berkas konfigurasi tidak dapat dibaca atau ditulis.";
/// `DBL-8604`: a lifecycle operation owns the instance.
pub const ERR_BUSY: &str = "Instance sedang dipakai operasi lain. Coba lagi setelah selesai.";
/// `DBL-8605`: nothing to restore.
pub const ERR_NO_BACKUP: &str = "Belum ada salinan konfigurasi yang bisa dikembalikan.";
/// `DBL-8606`: the whole call was refused.
pub const ERR_REFUSED: &str =
    "Perubahan ditolak seluruhnya karena ada setelan yang tidak diizinkan.";

// --- One-line descriptions for the editable settings -------------------------

/// PostgreSQL `max_connections`.
pub const DESC_MAX_CONNECTIONS_PG: &str =
    "Jumlah maksimum koneksi klien yang diterima server sekaligus.";
/// MySQL `max_connections`.
pub const DESC_MAX_CONNECTIONS_MYSQL: &str =
    "Jumlah maksimum koneksi klien yang diterima server sekaligus.";
/// MariaDB `max_connections`.
pub const DESC_MAX_CONNECTIONS_MARIADB: &str =
    "Jumlah maksimum koneksi klien yang diterima server sekaligus.";
/// PostgreSQL `shared_buffers`.
pub const DESC_SHARED_BUFFERS: &str =
    "Memori bersama untuk cache halaman; sekitar 25% RAM adalah titik awal yang lazim.";
/// PostgreSQL `work_mem`.
pub const DESC_WORK_MEM: &str =
    "Batas memori per operasi pengurutan atau hash sebelum memakai berkas sementara.";
/// PostgreSQL `timezone`.
pub const DESC_TIMEZONE_PG: &str = "Zona waktu yang dipakai server saat menampilkan waktu.";
/// MySQL `default-time-zone`.
pub const DESC_TIME_ZONE_MYSQL: &str =
    "Zona waktu bawaan server; kosong berarti zona waktu sistem.";
/// MariaDB `default-time-zone`.
pub const DESC_TIME_ZONE_MARIADB: &str =
    "Zona waktu bawaan server; kosong berarti zona waktu sistem.";
/// MySQL/MariaDB `innodb_buffer_pool_size`.
pub const DESC_INNODB_BUFFER_POOL: &str =
    "Cache data dan indeks InnoDB; bagian terbesar dari memori database.";
/// MySQL `sort_buffer_size`.
pub const DESC_SORT_BUFFER_MYSQL: &str =
    "Memori per operasi pengurutan pada MySQL; nilainya berlaku per koneksi.";
/// MariaDB `sort_buffer_size`.
pub const DESC_SORT_BUFFER_MARIADB: &str =
    "Memori per operasi pengurutan pada MariaDB; nilainya berlaku per koneksi.";
/// MySQL/MariaDB `character-set-server`.
pub const DESC_CHARACTER_SET: &str = "Set karakter bawaan untuk data dan koneksi baru.";
/// Valkey `maxmemory`.
pub const DESC_MAXMEMORY: &str = "Batas memori yang boleh dipakai Valkey; 0 berarti tanpa batas.";
/// Valkey `maxmemory-policy`.
pub const DESC_MAXMEMORY_POLICY: &str = "Cara Valkey membuang kunci saat memori mencapai batas.";
/// Valkey `appendonly`.
pub const DESC_APPENDONLY: &str =
    "Menyimpan setiap perubahan ke berkas append-only agar data tidak hilang saat restart.";
/// Valkey `save`.
pub const DESC_SAVE: &str =
    "Jadwal snapshot RDB (pasangan detik dan jumlah perubahan); kosong berarti nonaktif.";
/// Valkey `timeout`.
pub const DESC_TIMEOUT: &str =
    "Menutup koneksi klien yang menganggur lebih lama dari detik ini; 0 berarti tidak pernah.";

// --- CLI strings -------------------------------------------------------------

/// Shown after a successful `config set`.
pub const MSG_SAVED: &str = "Konfigurasi tersimpan. Perubahan berlaku setelah restart.";
/// Shown when a backup path is printed.
pub const MSG_BACKUP_AT: &str = "Salinan sebelum perubahan:";
/// Shown when a restore succeeded.
pub const MSG_RESTORED: &str = "Konfigurasi dikembalikan dari salinan terbaru.";
/// Column header for the settings table.
pub const HEADER_SETTING: &str = "SETELAN";
/// Column header for the current value.
pub const HEADER_CURRENT: &str = "SAAT INI";
/// Column header for the default.
pub const HEADER_DEFAULT: &str = "BAWAAN";
/// Column header for the unit.
pub const HEADER_UNIT: &str = "SATUAN";
/// Column header for restart-required.
pub const HEADER_RESTART: &str = "RESTART";
/// The word shown for "yes" in the restart column.
pub const YES: &str = "ya";
/// The word shown for "default" when a setting is unset in the file.
pub const DEFAULT_LABEL: &str = "bawaan";
/// The word shown for a read-only row.
pub const READONLY_LABEL: &str = "hanya-baca";
/// Prompt for a `key=value` pair the CLI cannot parse.
pub const ERR_SET_FORMAT: &str = "Format harus key=nilai.";
