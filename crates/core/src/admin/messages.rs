//! Indonesian user-facing messages for every [`super::error::AdminError`].
//!
//! Every string here is static: it can never embed a password, a connection
//! string or the text of a failing statement. The message is what the user
//! reads; the stable `DBL-81xx` code is what support searches for.

/// `DBL-8100`: the engine could not be reached.
pub const ERR_CONNECT: &str = "Tidak dapat terhubung ke server database. Pastikan server sedang berjalan.";
/// `DBL-8101`: a name or value failed validation.
pub const ERR_INVALID_IDENTIFIER: &str = "Nama tidak valid. Gunakan hanya huruf, angka, dan garis bawah.";
/// `DBL-8102`: the target host is not loopback.
pub const ERR_NON_LOOPBACK: &str = "Hanya koneksi ke 127.0.0.1 atau ::1 yang diizinkan.";
/// `DBL-8103`: the target description is incomplete.
pub const ERR_INVALID_TARGET: &str = "Konfigurasi koneksi tidak lengkap.";
/// `DBL-8104`: the database already exists.
pub const ERR_DATABASE_EXISTS: &str = "Database dengan nama itu sudah ada.";
/// `DBL-8105`: the database does not exist.
pub const ERR_DATABASE_NOT_FOUND: &str = "Database tidak ditemukan.";
/// `DBL-8106`: the database still has connections.
pub const ERR_DATABASE_IN_USE: &str = "Database masih dipakai koneksi lain. Tutup koneksi tersebut lalu coba lagi.";
/// `DBL-8107`: the user already exists.
pub const ERR_USER_EXISTS: &str = "User dengan nama itu sudah ada.";
/// `DBL-8108`: the user does not exist.
pub const ERR_USER_NOT_FOUND: &str = "User tidak ditemukan.";
/// `DBL-8109`: the server refused the operation for lack of privilege.
pub const ERR_INSUFFICIENT_PRIVILEGE: &str = "Server menolak perintah karena hak akses tidak mencukupi.";
/// `DBL-8110`: the server returned an error this module cannot classify.
pub const ERR_QUERY_FAILED: &str = "Perintah ke server database gagal.";
/// `DBL-8111`: the operation is not supported for this engine.
pub const ERR_UNSUPPORTED: &str = "Operasi ini belum didukung untuk engine tersebut.";
/// `DBL-8112`: the new password did not authenticate.
pub const ERR_PASSWORD_VERIFY: &str = "Password baru tidak dapat dipakai untuk login; password lama dipulihkan.";
/// `DBL-8113`: the credential store refused the new password.
pub const ERR_CREDENTIAL_STORE: &str = "Password baru tidak dapat disimpan; password lama dipulihkan.";
/// `DBL-8114`: the rollback to the old password failed.
pub const ERR_ROLLBACK: &str = "Terjadi kegagalan saat memulihkan password lama. Periksa kredensial server secara manual.";
/// `DBL-8115`: the operating system RNG failed.
pub const ERR_PASSWORD_GENERATE: &str = "Password baru tidak dapat dibuat.";
