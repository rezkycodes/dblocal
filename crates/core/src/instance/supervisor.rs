//! Background supervision of running instances.
//!
//! Watches processes started or adopted by this service and marks an unexpected
//! death as `Failed` with the log tail diagnosed, never as a silent `Running`.

use std::sync::Arc;

use crate::model::{Instance, InstanceEvent, InstanceId, InstanceStatus};
use crate::process::identity::ProcessState;

use super::helpers::{
    blocking, db_async, diagnose, emit_changed, failure_info, failure_tail, identity_of,
    log_file_for,
};
use super::service::Inner;

/// Runs forever on the service runtime, polling at `STATUS_POLL_SECS`.
pub(crate) async fn run(inner: Arc<Inner>) {
    loop {
        // Sleep first: reconciliation has just run, so an immediate poll would
        // only repeat it. The interval keeps idle CPU near zero.
        let interval = inner.status_poll;
        tokio::time::sleep(interval).await;
        if inner.is_stopping() {
            return;
        }
        tick(&inner).await;
    }
}

async fn tick(inner: &Arc<Inner>) {
    let Ok(instances) = db_async(inner, |store| store.list_instances()).await else {
        return;
    };
    for instance in instances {
        if !matches!(
            instance.status(),
            InstanceStatus::Running | InstanceStatus::Starting
        ) {
            continue;
        }
        let Some(identity) = identity_of(&instance) else {
            continue;
        };
        let probe = Arc::clone(&inner.probe);
        let state = match blocking(move || Ok(probe.validate(&identity))).await {
            Ok(state) => state,
            Err(_) => continue,
        };
        match state {
            ProcessState::Match => {}
            ProcessState::Absent | ProcessState::Reused => {
                mark_failed(inner, &instance).await;
            }
            ProcessState::Unknown => {}
        }
    }
}

async fn mark_failed(inner: &Arc<Inner>, instance: &Instance) {
    let id = instance.id().clone();
    let log_path = log_file_for(&inner.paths, instance);
    let tail = failure_tail(&log_path);
    let diagnosis = diagnose(inner, instance.engine(), &tail.join("\n"));

    if let Ok(mut failures) = inner.failures.lock() {
        failures.insert(
            id.clone(),
            failure_info(
                crate::error::CODE_ENGINE,
                crate::strings::ERR_ENGINE.to_owned(),
                diagnosis,
                tail,
            ),
        );
    }

    let id2 = id.clone();
    let _ = db_async(inner, move |store| {
        store.update_status(&id2, InstanceStatus::Failed)?;
        store.update_process_identity(&id2, None, None, None)
    })
    .await;
    emit_changed(inner, &id);
}

#[allow(dead_code)]
fn _unused(_: InstanceId, _: InstanceEvent) {}
