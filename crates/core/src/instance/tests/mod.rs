//! W2-A tests: lifecycle, events, failure injection, safety.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

mod archive_recovery;
mod catalog;
mod coverage;
mod download;
mod gates;
mod harness;
mod harness_fakes;
mod lifecycle;
mod mutations;
mod races;
mod recovery;
mod safety;

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
            "created instance {name} not listed after successful create; events={:?}",
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
