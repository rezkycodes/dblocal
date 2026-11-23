//! One module per e2e case, so a failure names the behaviour it was proving.
//!
//! Shared entry guard and operation assertions live here.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod admin_mysql;
pub mod autostart_instances;
pub mod cancel_and_checksum;
pub mod concurrency;
pub mod config_editor;
pub mod engines_outlive_app;
pub mod mariadb_download;
pub mod mysql_download;
pub mod mysql_mariadb;
pub mod mysql_mariadb_start_failure;
pub mod numbers;
pub mod port_in_use;
pub mod postgres_happy_path;
pub mod repro_change_port;
pub mod repro_data_dir_mode;
pub mod repro_hardlink;
pub mod security_invariants;
pub mod valkey_happy_path;

use crate::support::events::OpRun;

/// Short engine prefix used in instance names (`pg`, `vk`).
pub fn short(engine: dblocal_core::model::EngineKey) -> &'static str {
    match engine {
        dblocal_core::model::EngineKey::Postgresql => "pg",
        dblocal_core::model::EngineKey::Valkey => "vk",
        _ => "db",
    }
}

/// Refuses to run without `DBLOCAL_E2E=1`.
pub fn require_e2e() {
    if std::env::var("DBLOCAL_E2E").ok().as_deref() != Some("1") {
        panic!(
            "service_e2e needs `DBLOCAL_E2E=1`; run `DBLOCAL_E2E=1 cargo xtask e2e`, \
             or add `-- --ignored` to a direct `cargo test` invocation"
        );
    }
}

/// Panics with the operation's code, message and observed stages on failure.
pub fn expect_ok(run: &OpRun, what: &str) {
    if let Err(err) = &run.result {
        panic!(
            "{what} failed: [{}] {}; stages observed: {:?}",
            err.code,
            err.message,
            run.stage_names()
        );
    }
}

/// Panics with the observed stages when the operation unexpectedly succeeded.
pub fn expect_err(run: &OpRun, what: &str) -> dblocal_core::service::UserFacingError {
    match &run.result {
        Ok(()) => panic!(
            "{what} unexpectedly succeeded; stages observed: {:?}",
            run.stage_names()
        ),
        Err(err) => err.clone(),
    }
}
