//! Database and user administration (PRD `F-DBU-01..03`).
//!
//! Two layers:
//!
//! - [`sql`] is pure. It validates identifiers against an ASCII allowlist,
//!   quotes per dialect and builds every statement with PostgreSQL and
//!   MySQL/MariaDB variants. All of it is unit-tested, including a hostile
//!   corpus and a tiny tokenizer that proves one input cannot become two
//!   statements.
//! - [`postgres`] executes those statements over `tokio-postgres`, loopback
//!   only, with typed [`error::AdminError`] failures, and carries the
//!   F-DBU-03 AC2 password-reset rollback guarantee.
//!
//! The module never stores a password, never interpolates a raw identifier into
//! SQL, and never touches the GUI or the `DblService` wiring: that injection is
//! a later task, and `docs/architecture/admin.md` proposes the trait shape for
//! it.

pub mod error;
pub mod messages;
pub mod postgres;
pub mod sql;

pub use error::{AdminError, AdminResult, CODE_RANGE};
pub use postgres::{AdminTarget, CredentialSink, DatabaseInfo, PgAdmin, UserInfo};
pub use sql::{Dialect, Identifier, PrivilegeLevel};
