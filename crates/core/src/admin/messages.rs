//! User-facing messages for every [`super::error::AdminError`].
//!
//! Every string here is static: it can never embed a password, a connection
//! string or the text of a failing statement. The message is what the user
//! reads; the stable `DBL-81xx` code is what support searches for.

/// `DBL-8100`: the engine could not be reached.
pub const ERR_CONNECT: &str =
    "Could not connect to the database server. Make sure the server is running.";
/// `DBL-8116`: the stored password was refused by the engine.
pub const ERR_AUTH_FAILED: &str = "The stored password does not match. Check the server credentials.";
/// `DBL-8101`: a name or value failed validation.
pub const ERR_INVALID_IDENTIFIER: &str =
    "Invalid name. Use only letters, digits and underscores.";
/// `DBL-8102`: the target host is not loopback.
pub const ERR_NON_LOOPBACK: &str = "Only connections to 127.0.0.1 or ::1 are allowed.";
/// `DBL-8103`: the target description is incomplete.
pub const ERR_INVALID_TARGET: &str = "The connection details are incomplete.";
/// `DBL-8104`: the database already exists.
pub const ERR_DATABASE_EXISTS: &str = "A database with that name already exists.";
/// `DBL-8105`: the database does not exist.
pub const ERR_DATABASE_NOT_FOUND: &str = "Database not found.";
/// `DBL-8106`: the database still has connections.
pub const ERR_DATABASE_IN_USE: &str =
    "Other connections still use the database. Close them and try again.";
/// `DBL-8107`: the user already exists.
pub const ERR_USER_EXISTS: &str = "A user with that name already exists.";
/// `DBL-8108`: the user does not exist.
pub const ERR_USER_NOT_FOUND: &str = "User not found.";
/// `DBL-8109`: the server refused the operation for lack of privilege.
pub const ERR_INSUFFICIENT_PRIVILEGE: &str =
    "The server refused the command because the privileges are insufficient.";
/// `DBL-8110`: the server returned an error this module cannot classify.
pub const ERR_QUERY_FAILED: &str = "The command to the database server failed.";
/// `DBL-8111`: the operation is not supported for this engine.
pub const ERR_UNSUPPORTED: &str = "This operation is not supported for that engine yet.";
/// `DBL-8112`: the new password did not authenticate.
pub const ERR_PASSWORD_VERIFY: &str =
    "The new password cannot be used to log in; the old password was restored.";
/// `DBL-8113`: the credential store refused the new password.
pub const ERR_CREDENTIAL_STORE: &str =
    "The new password could not be saved; the old password was restored.";
/// `DBL-8114`: the rollback to the old password failed.
pub const ERR_ROLLBACK: &str =
    "Restoring the old password failed. Check the server credentials manually.";
/// `DBL-8115`: the operating system RNG failed.
pub const ERR_PASSWORD_GENERATE: &str = "A new password could not be generated.";
/// `DBL-8117`: the instance has no password, so admin cannot authenticate.
pub const ERR_NO_PASSWORD: &str = "This instance was created without a password, so database management requires an instance created with a password.";
/// `DBL-8118`: the database is one the MySQL/MariaDB engine owns.
pub const ERR_SYSTEM_SCHEMA: &str = "Engine-owned system databases (mysql, sys, information_schema, performance_schema) cannot be managed from here.";
