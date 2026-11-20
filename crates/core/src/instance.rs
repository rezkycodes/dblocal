//! Real `DblService` implementation: instance lifecycle orchestration.
//!
//! Owned by W2-A. Wires the store, catalog, downloader, process layer, secrets
//! and engine adapters into one `DblService`. Design notes live in
//! `docs/architecture/service.md`.

mod catalog_ops;
mod create;
mod delete;
mod helpers;
mod log_watch;
mod read_ops;
mod reconcile;
mod registry;
mod runtime_mgmt;
mod service;
mod start;
mod stop;
mod supervisor;

pub use service::{
    LocalService, ProcessTerminator, RuntimeSource, SecretAccess, ServiceOptions, Terminator,
};

#[cfg(test)]
mod tests;
