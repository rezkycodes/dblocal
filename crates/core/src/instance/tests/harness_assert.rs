//! Event and end-state assertions over the harness fakes.
//!
//! Split out of `harness.rs` so each test-support file stays under the size limit;
//! `harness.rs` re-exports every item here, so call sites are unchanged.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::time::Duration;

use crate::model::InstanceId;
use crate::paths::DblPaths;
use crate::service::ServiceEvent;
use crate::store::Store;

use super::harness::Harness;

/// Reserves a test port in 20000-29999.
pub fn test_port() -> u16 {
    crate::ports::test_support::reserve_test_port().get()
}

/// Reads the last `n` events of a finished operation of a kind.
pub fn stages(events: &[ServiceEvent]) -> Vec<&'static str> {
    events
        .iter()
        .filter_map(|event| match event {
            ServiceEvent::OperationProgress { stage, .. } => Some(stage.as_str()),
            _ => None,
        })
        .collect()
}

pub fn finished_result(
    events: &[ServiceEvent],
    op: &crate::service::OperationId,
) -> Option<std::result::Result<(), crate::service::UserFacingError>> {
    events.iter().find_map(|event| match event {
        ServiceEvent::OperationFinished { id, result } if id == op => Some(result.clone()),
        _ => None,
    })
}

/// Whether a `InstanceRemoved` for `id` appears in `events`.
pub fn removed_for(events: &[ServiceEvent], id: &InstanceId) -> bool {
    events
        .iter()
        .any(|event| matches!(event, ServiceEvent::InstanceRemoved { id: got } if got == id))
}

/// Whether a `StopForced` for `id` appears in `events`.
pub fn stop_forced_for(events: &[ServiceEvent], id: &InstanceId) -> bool {
    events
        .iter()
        .any(|event| matches!(event, ServiceEvent::StopForced { id: got } if got == id))
}

/// Counts `OperationFinished` events in `events`.
pub fn finished_count(events: &[ServiceEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, ServiceEvent::OperationFinished { .. }))
        .count()
}

/// Compact rendering used in an assertion message when an event order is wrong.
pub fn describe(events: &[ServiceEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| match event {
            ServiceEvent::OperationProgress { stage, .. } => {
                format!("Progress({})", stage.as_str())
            }
            ServiceEvent::InstanceChanged { view } => format!("Changed({})", view.status.as_str()),
            ServiceEvent::OperationFinished { result, .. } => match result {
                Ok(()) => "Finished(ok)".to_owned(),
                Err(err) => format!("Finished({})", err.code),
            },
            ServiceEvent::DownloadProgress { .. } => "Download".to_owned(),
            ServiceEvent::InstanceRemoved { .. } => "Removed".to_owned(),
            ServiceEvent::StopForced { .. } => "StopForced".to_owned(),
            _ => "Other".to_owned(),
        })
        .collect()
}

/// Collects events until `op` reports `OperationFinished`, with a hard deadline.
///
/// Uses `try_recv` plus a short sleep instead of `blocking_recv`: an operation
/// can finish before the receiver subscribes (the receiver is created after the
/// call in the two-service races), and `blocking_recv` would then wait forever
/// because the missed event is never re-delivered. Polling keeps the deadline
/// honest.
pub(super) fn wait_for_finished(
    receiver: &mut tokio::sync::broadcast::Receiver<ServiceEvent>,
    op: &crate::service::OperationId,
    is_finished: impl Fn() -> bool,
) -> Vec<ServiceEvent> {
    // Generous: the whole instance suite runs many services (each with a
    // two-worker runtime) in parallel, and a busy machine can delay a task's
    // first poll by seconds. The deadline only has to catch a genuine hang.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let mut collected = Vec::new();
    loop {
        loop {
            match receiver.try_recv() {
                Ok(event) => {
                    let finished =
                        matches!(&event, ServiceEvent::OperationFinished { id, .. } if id == op);
                    collected.push(event);
                    if finished {
                        return collected;
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                    panic!("event channel closed before {op} finished")
                }
            }
        }
        // The operation may have finished before this receiver subscribed; its
        // `OperationFinished` is then lost to the broadcast. Completion is still
        // observable through the registry, so stop waiting once it reports done.
        //
        // `finish_operation` broadcasts before recording the registry entry, so a
        // receiver that is subscribed (the normal case here) finds the event in its
        // channel by the time the registry says finished. Drain once more before
        // returning so that ordering actually pays off instead of racing the
        // registry read against the final `try_recv`.
        if is_finished() {
            drain(receiver, &mut collected);
            return collected;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "operation {op} did not finish in time; saw: {:?}",
            describe(&collected)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Drains every currently-available event into `collected`.
pub(super) fn drain(
    receiver: &mut tokio::sync::broadcast::Receiver<ServiceEvent>,
    collected: &mut Vec<ServiceEvent>,
) {
    loop {
        match receiver.try_recv() {
            Ok(event) => collected.push(event),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => return,
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return,
        }
    }
}

/// Receives events for up to `timeout`, returning every event seen.
///
/// The non-panicking counterpart of [`recv_until`]: a caller that wants to retry a
/// stimulus (for example append to a log again) uses this and checks the result
/// itself, and still never blocks unboundedly.
pub fn recv_for(
    receiver: &mut tokio::sync::broadcast::Receiver<ServiceEvent>,
    timeout: Duration,
) -> Vec<ServiceEvent> {
    let deadline = std::time::Instant::now() + timeout;
    let mut seen = Vec::new();
    loop {
        match receiver.try_recv() {
            Ok(event) => seen.push(event),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return seen,
        }
        if std::time::Instant::now() >= deadline {
            return seen;
        }
        if !seen.is_empty() {
            // Drain anything already queued before returning.
            drain(receiver, &mut seen);
            return seen;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Receives events until `accept` returns true, with a hard deadline.
///
/// Uses `try_recv` plus a short sleep, never `blocking_recv`: a `blocking_recv`
/// loop whose deadline is only checked after an event arrives hangs forever when
/// nothing is ever delivered (a missing event must fail the test, not stall it).
/// Every event seen is pushed to `seen`. Panics with `missing` at the deadline.
pub fn recv_until(
    receiver: &mut tokio::sync::broadcast::Receiver<ServiceEvent>,
    timeout: Duration,
    missing: &str,
    mut accept: impl FnMut(&ServiceEvent) -> bool,
) -> Vec<ServiceEvent> {
    let deadline = std::time::Instant::now() + timeout;
    let mut seen = Vec::new();
    loop {
        match receiver.try_recv() {
            Ok(event) => {
                let accepted = accept(&event);
                seen.push(event);
                if accepted {
                    return seen;
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                panic!("{missing}: the event channel closed before it arrived")
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{missing}; saw: {:?}",
            describe(&seen)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn row_gone_diagnostic(paths: &DblPaths, id: &InstanceId) -> String {
    let store = match Store::open(paths) {
        Ok(store) => store,
        Err(err) => return format!("store open failed: {err}"),
    };
    let row = match store.get_instance(id) {
        Ok(row) => row,
        Err(err) => return format!("store read failed: {err}"),
    };
    let Some(instance) = row else {
        return "row is gone".to_owned();
    };
    let instance_dir = paths.instance_dir(instance.name());
    let marker_path = super::super::delete::marker_path(paths, &instance.id().clone());
    let marker = std::fs::read_to_string(&marker_path)
        .map(|contents| contents.replace('\n', " | "))
        .unwrap_or_else(|err| format!("<unreadable: {err}>"));
    let dir_entries: Vec<String> = std::fs::read_dir(&instance_dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    format!(
        "status={:?} pid={:?} started_at={:?} exe={:?} instance={} instance_dir={} \
         marker_path={} marker=[{marker}] dir={dir_entries:?} db={}",
        instance.status(),
        instance.pid(),
        instance.process_started_at(),
        instance.process_exe(),
        instance.name().as_str(),
        instance_dir.display(),
        marker_path.display(),
        paths.state_db().display(),
    )
}

/// Waits until `id` is gone, with a deadline. Shared by recovery tests.
pub fn wait_until_row_gone(h: &Harness, id: &InstanceId, round: usize) {
    // Generous: the whole instance suite runs many services (each with a
    // two-worker runtime) in parallel, and a busy machine can delay a task's
    // first poll by seconds. The deadline only has to catch a genuine hang.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        if h.instance(id).is_none() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "round {round}: the row was never removed; {}",
            row_gone_diagnostic(&h.paths, id)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Waits for an `InstanceRemoved` event for `id`, returning false on timeout.
pub fn wait_for_removed(
    receiver: &mut tokio::sync::broadcast::Receiver<ServiceEvent>,
    id: &InstanceId,
    timeout: Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match receiver.try_recv() {
            Ok(event) if removed_for(std::slice::from_ref(&event), id) => return true,
            Ok(_) => continue,
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return false,
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
