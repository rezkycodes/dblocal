//! The admin modules the transport code needs, included by path from the product
//! source so they cannot drift.

#[path = "../../../../../../../crates/core/src/admin/error.rs"]
pub mod error;
#[path = "../../../../../../../crates/core/src/admin/executor.rs"]
pub mod executor;
#[path = "../../../../../../../crates/core/src/admin/messages.rs"]
pub mod messages;
#[path = "../../../../../../../crates/core/src/admin/reset.rs"]
pub mod reset;
#[path = "../../../../../../../crates/core/src/admin/sql.rs"]
pub mod sql;

/// Stub of the two record types (see the file's own header).
pub mod postgres;

#[path = "../../../../../../../crates/core/src/admin/mysql.rs"]
pub mod mysql;
