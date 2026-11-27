//! Toolchains: PHP, Composer and Node.js installed per user and reached through
//! shims.
//!
//! This module is the core of the v0.3.0 "Toolchains" release. It owns:
//!
//! * [`catalog`] — the tolerant parser for `catalog/toolchains-v1.json`, the
//!   bundled/cached/refreshed snapshot, and the selection helpers that map a user
//!   version spec to a catalog artifact;
//! * [`install`] — download, SHA-256 verification, safe extraction and the atomic
//!   rename into `<data>/toolchains/<tool>/<version>[-<profile>]/`;
//! * [`registry`] — the `active.json` choice, plus the install scan that is the
//!   source of truth for "installed";
//! * [`shim`] — the generated launchers in `<data>/bin`;
//! * [`php_ini`] — the typed PHP settings rendered to a DBLocal-managed `conf.d`;
//! * [`path`] — the consent-gated PATH block and the doctor checks.
//!
//! Nothing here starts a background thread or timer: every operation is driven by
//! a CLI call or a GUI action, so the app's idle CPU and RSS do not change.

pub mod catalog;
pub mod consts;
pub mod error;
pub mod fsops;
pub mod install;
pub mod lock;
pub mod path;
pub mod php_ini;
pub mod registry;
pub mod shim;

pub use catalog::{ToolId, ToolchainCatalog, VersionSpec};
pub use error::ToolchainError;
pub use fsops::ToolchainPaths;

#[cfg(test)]
pub(crate) mod catalog_test_support;

#[cfg(test)]
mod contract_tests;
