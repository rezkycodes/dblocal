//! Type-check probe for `crates/core/src/admin/mysql.rs` on a Windows target.
//!
//! `cargo check -p dblocal-core --target x86_64-pc-windows-gnu` stops in
//! `aws-lc-sys`/`libz-sys`/`libsqlite3-sys`/`liblzma-sys` (no mingw C compiler on
//! this host), so the touched module is compiled here against the **real**
//! `mysql_async` for the Windows target, with only its neighbours stubbed. The
//! modules the transport code depends on for its API (`admin/sql`,
//! `admin/error`, `admin/reset`, `admin/executor`, `admin/messages`) are pulled
//! in with `#[path = ...]` from the product source, never copied, so they cannot
//! drift. Nothing here is shipped. See ../README.md.

pub mod error;
pub mod secret;
pub mod secrets;
pub mod service;

pub mod admin;

/// The probe only means anything for a non-unix target: that is the cfg that
/// takes the `Transport::Loopback` arm. A host build would type-check the unix
/// arm and prove nothing, so refuse it.
#[cfg(unix)]
compile_error!(
    "this probe must be built for a Windows target: cargo check --target x86_64-pc-windows-gnu"
);
