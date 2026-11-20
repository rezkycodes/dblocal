//! SQLite persistence for instances, runtimes, archives and settings.
//!
//! The layer is synchronous on purpose: the service layer (W2-A) calls it from
//! `spawn_blocking`, so nothing here touches async. See
//! `docs/architecture/store.md` for the schema and invariants.

mod api;
mod error;
mod migrations;
mod rows;
mod schema;
mod time;

pub use api::Store;

#[cfg(test)]
mod tests;
