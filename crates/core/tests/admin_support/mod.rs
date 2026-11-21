//! Support code for the admin end-to-end tests.
//!
//! Deliberately self-contained: the task forbids editing `tests/support/**`
//! because another worker owns those files, so this module re-implements the
//! small pieces the admin e2e needs (temp home, port allocation, `/proc`
//! scanning, `initdb`/`pg_ctl`) over the same production engine layout. The
//! ideas are the same; the code is independent.
//!
//! Nothing here starts a server unless a test asks. Every engine a test starts
//! is killed by [`AdminCluster::drop`], which scans `/proc` for a `postgres`
//! binary living inside the temp home's `runtimes/` tree — the positive
//! identification AGENTS.md invariant 7 requires — even on panic.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod cluster;
pub mod port;
pub mod temp_home;

pub use temp_home::TempHome;
