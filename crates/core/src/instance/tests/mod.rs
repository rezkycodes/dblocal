//! W2-A tests: lifecycle, events, failure injection, safety.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

mod archive_recovery;
mod catalog;
mod config;
mod coverage;
mod delete_marker_cleanup;
mod download;
mod gates;
mod harness;
mod harness_assert;
mod harness_fakes;
mod lifecycle;
mod mutations;
mod port_sync;
mod races;
mod recovery;
mod runtime_fixture;
mod runtime_reuse;
mod safety;
mod service_admin;

use crate::model::{EngineKey, InstanceStatus};
use crate::service::DblService;

use harness::{Harness, test_port};

/// A stopped PostgreSQL instance created through the service.
fn created(h: &Harness, name: &str) -> crate::model::Instance {
    let port = test_port();
    let spec = h.spec(name, EngineKey::Postgresql, port);
    let events = h.create(spec);
    let failed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            crate::service::ServiceEvent::OperationFinished {
                result: Err(err), ..
            } => Some(err.clone()),
            _ => None,
        })
        .collect();
    assert!(failed.is_empty(), "create failed: {failed:?}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Some(instance) = h
            .instances()
            .into_iter()
            .find(|i| i.name().as_str() == name)
        {
            return instance;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "created instance {name} not listed after successful create; \
             service_view={:?} fresh_view={:?} state_db={} exists={} len={} \
             events={:?}",
            h.service
                .list_instances()
                .map(|views| views.into_iter().map(|view| view.name).collect::<Vec<_>>()),
            h.instances()
                .into_iter()
                .map(|instance| instance.name().as_str().to_owned())
                .collect::<Vec<_>>(),
            h.paths.state_db().display(),
            h.paths.state_db().exists(),
            std::fs::metadata(h.paths.state_db())
                .map(|meta| meta.len())
                .unwrap_or(0),
            harness::describe(&events)
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn create_then_list_reports_a_stopped_instance() {
    let h = Harness::new(EngineKey::Postgresql);
    let instance = created(&h, "app-dev");
    assert_eq!(instance.status(), InstanceStatus::Stopped);
    let views = h.service.list_instances().expect("list");
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].name, "app-dev");
}
