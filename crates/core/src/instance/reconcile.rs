//! Startup reconciliation (`F-INS-04`, D-02).
//!
//! Engines outlive the app, so a pidfile from the previous session either names
//! a live process that is still ours (adopt as `Running`) or a dead/reused PID
//! (mark `Stopped` and clear the identity). Nothing is signalled here.

use std::sync::Arc;

use crate::model::{Instance, InstanceEvent, InstanceId, InstanceStatus};
use crate::process::reconcile::{ReconcileRecord, reconcile};

use super::helpers::{db_async, emit_changed, identity_of};
use super::service::Inner;

pub(crate) async fn reconcile_on_startup(inner: Arc<Inner>) {
    let instances = match db_async(&inner, |store| store.list_instances()).await {
        Ok(instances) => instances,
        Err(err) => {
            tracing::warn!(%err, "reconciliation could not list instances");
            return;
        }
    };

    // An instance with an operation running in this process is not interrupted;
    // it is still being created. Skipping it keeps reconciliation from racing a
    // create that started at the same instant as the app.
    let instances: Vec<Instance> = instances
        .into_iter()
        .filter(|instance| !inner.ops.is_busy(instance.id()))
        .collect();

    // A `Deleting` row is terminal, so finishing it is the only way out: the
    // delete it belongs to already archived or removed the data before the app
    // died, and leaving the row would make the instance unreachable forever.
    for instance in &instances {
        if instance.status() == InstanceStatus::Deleting {
            finish_delete(&inner, instance).await;
        }
    }

    // Finish interrupted operations first: a `Provisioning` or `Initializing`
    // row means the app died mid-create.
    for instance in &instances {
        if matches!(
            instance.status(),
            InstanceStatus::Provisioning | InstanceStatus::Initializing
        ) {
            mark(&inner, instance, InstanceEvent::Interrupted).await;
        }
    }

    let pending: Vec<Instance> = instances
        .iter()
        .filter(|instance| instance.status().holds_process())
        .cloned()
        .collect();
    if pending.is_empty() {
        return;
    }

    let records: Vec<ReconcileRecord> = pending
        .iter()
        .map(|instance| ReconcileRecord::new(instance.id().clone(), identity_of(instance)))
        .collect();

    let probe = Arc::clone(&inner.probe);
    let Ok(outcomes) = db_async(&inner, move |_store| {
        Ok(reconcile(&records, probe.as_ref()))
    })
    .await
    else {
        return;
    };

    for outcome in outcomes {
        let Some(instance) = pending.iter().find(|i| i.id() == outcome.id()) else {
            continue;
        };
        if outcome.is_adopted() {
            adopt(&inner, instance).await;
        } else {
            mark(&inner, instance, InstanceEvent::ReconciledAbsent).await;
        }
    }
}

/// Removes the row a crashed delete left behind, with the same guards the live
/// delete uses (nothing outside the home is touched).
async fn finish_delete(inner: &Arc<Inner>, instance: &Instance) {
    let id = instance.id().clone();
    let instance_dir = inner.paths.instance_dir(instance.name());
    let data_dir = inner.paths.data_dir(instance.name());
    if super::helpers::safe_within(&inner.paths, &instance_dir).is_ok() {
        let _ = super::helpers::blocking(move || {
            super::helpers::remove_tree(&data_dir)?;
            super::helpers::remove_tree(&instance_dir)
        })
        .await;
    }
    if let Ok(Some(reference)) = inner.db(|store| store.get_credential(&id)) {
        let _ = inner.secrets.delete(&reference);
    }
    let _ = db_async(inner, move |store| {
        store.delete_instance(&id).or_else(|err| match err {
            crate::DblError::NotFound { .. } => Ok(()),
            other => Err(other),
        })
    })
    .await;
    emit_changed(inner, &instance.id().clone());
}

async fn adopt(inner: &Arc<Inner>, instance: &Instance) {
    // A `Stopping` record that is still alive is finished rather than adopted,
    // so it never dangles; a live `Starting`/`Running` engine is adopted.
    let next = match instance.status() {
        InstanceStatus::Starting | InstanceStatus::Running => InstanceStatus::Running,
        InstanceStatus::Stopping => InstanceStatus::Stopped,
        other => other,
    };
    if next != instance.status() {
        persist(inner, instance.id(), next).await;
    }
    if next == InstanceStatus::Stopped {
        clear_identity(inner, instance.id()).await;
    }
    emit_changed(inner, instance.id());
}

async fn mark(inner: &Arc<Inner>, instance: &Instance, event: InstanceEvent) {
    if let Ok(next) = instance.status().apply(event) {
        persist(inner, instance.id(), next).await;
    }
    if matches!(
        event,
        InstanceEvent::ReconciledAbsent | InstanceEvent::Interrupted
    ) {
        clear_identity(inner, instance.id()).await;
    }
    emit_changed(inner, instance.id());
}

async fn persist(inner: &Arc<Inner>, id: &InstanceId, next: InstanceStatus) {
    let id = id.clone();
    let _ = db_async(inner, move |store| store.update_status(&id, next)).await;
}

async fn clear_identity(inner: &Arc<Inner>, id: &InstanceId) {
    let id = id.clone();
    let _ = db_async(inner, move |store| {
        store.update_process_identity(&id, None, None, None)
    })
    .await;
}
