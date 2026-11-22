//! Shared helpers for the real-engine end-to-end tests.
//!
//! Everything here is test scaffolding: temp home isolation, port allocation,
//! `/proc` inspection, service construction, event collection and the security
//! invariant checks. The tests themselves live in `support/cases/`, wired to the
//! integration test binary by `tests/service_e2e.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod cache;
pub mod cases;
pub mod clients;
pub mod env;
pub mod events;
pub mod harness;
pub mod mariadb_shim;
pub mod ports;
pub mod proc;
pub mod security;

pub use harness::{grep_tree, part_files};
