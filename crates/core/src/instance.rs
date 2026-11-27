//! Real `DblService` implementation: instance lifecycle orchestration.
//!
//! Owned by W2-A. Wires the store, catalog, downloader, process layer, secrets
//! and engine adapters into one `DblService`. Design notes live in
//! `docs/architecture/service.md`.

mod admin_ops;
mod archive_name;
mod catalog_ops;
mod clients_ops;
mod config_ops;
mod create;
mod delete;
mod helpers;
mod log_watch;
mod read_ops;
mod reconcile;
mod registry;
mod retry;
mod runtime_files;
mod runtime_lock;
mod runtime_mgmt;
mod service;
mod start;
mod stop;
mod supervisor;
mod update_ops;

pub use service::{
    LocalService, ProcessTerminator, RuntimeSource, SecretAccess, ServiceOptions, Terminator,
};

#[cfg(test)]
mod tests;
