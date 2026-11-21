//! Event collection over the real `broadcast` stream.
//!
//! The stream drops old events for a slow reader (`Lagged`) instead of blocking
//! the service, so the collector treats a lag as "keep going" and only ever
//! asserts on what it actually saw. A create emits one progress sample every
//! 200 ms during the download, so lag is expected on a long download and never
//! affects the stage order assertions, which are about the stages that survived.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::{Duration, Instant};

use dblocal_core::model::InstanceId;
use dblocal_core::service::{OperationId, OperationStage, ServiceEvent, UserFacingError};
use tokio::sync::broadcast;

/// One operation's observed events plus its outcome.
#[derive(Debug, Clone)]
pub struct OpRun {
    /// Operation id the service returned.
    pub op: OperationId,
    /// Every event observed while the operation ran.
    pub events: Vec<ServiceEvent>,
    /// Final outcome, as carried by `OperationFinished`.
    pub result: Result<(), UserFacingError>,
    /// When each `OperationProgress` stage was observed, for the timing report.
    pub stage_at: Vec<(OperationStage, Instant)>,
}

impl OpRun {
    /// When the operation reported `stage`, if it did.
    pub fn at_stage(&self, stage: OperationStage) -> Option<Instant> {
        self.stage_at
            .iter()
            .find(|(seen, _)| *seen == stage)
            .map(|(_, at)| *at)
    }

    /// Stages the service reported for this operation, in arrival order.
    pub fn stages(&self) -> Vec<OperationStage> {
        self.events
            .iter()
            .filter_map(|event| match event {
                ServiceEvent::OperationProgress { id, stage } if *id == self.op => Some(*stage),
                _ => None,
            })
            .collect()
    }

    /// The stages as their stable strings.
    pub fn stage_names(&self) -> Vec<&'static str> {
        self.stages()
            .into_iter()
            .map(OperationStage::as_str)
            .collect()
    }

    /// Download progress samples observed for this operation.
    pub fn downloads(&self) -> Vec<dblocal_core::model::DownloadProgress> {
        self.events
            .iter()
            .filter_map(|event| match event {
                ServiceEvent::DownloadProgress { id, progress } if *id == self.op => {
                    Some(*progress)
                }
                _ => None,
            })
            .collect()
    }

    /// How many `OperationFinished` events this operation produced.
    pub fn finished_count(&self) -> usize {
        self.events
            .iter()
            .filter(|event| {
                matches!(event, ServiceEvent::OperationFinished { id, .. } if *id == self.op)
            })
            .count()
    }

    /// Whether `InstanceChanged` for `id` carried `status`.
    pub fn saw_status(&self, id: &InstanceId, status: dblocal_core::model::InstanceStatus) -> bool {
        self.events.iter().any(|event| {
            matches!(event, ServiceEvent::InstanceChanged { view } if view.id == *id && view.status == status)
        })
    }

    /// Whether an `InstanceRemoved` for `id` was observed.
    pub fn saw_removed(&self, id: &InstanceId) -> bool {
        self.events
            .iter()
            .any(|event| matches!(event, ServiceEvent::InstanceRemoved { id: got } if got == id))
    }
}

/// Reads the service event stream with a hard deadline.
pub struct Events {
    rx: broadcast::Receiver<ServiceEvent>,
}

impl Events {
    /// Wraps a receiver.
    pub fn new(rx: broadcast::Receiver<ServiceEvent>) -> Self {
        Self { rx }
    }

    /// Next event, or `None` when `timeout` elapses.
    pub fn next(&mut self, timeout: Duration) -> Option<ServiceEvent> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.rx.try_recv() {
                Ok(event) => return Some(event),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Empty) => {}
                Err(broadcast::error::TryRecvError::Closed) => return None,
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Collects until `op` reports `OperationFinished`, or panics at the deadline.
    pub fn until_finished(&mut self, op: &OperationId, timeout: Duration) -> OpRun {
        let deadline = Instant::now() + timeout;
        let mut events = Vec::new();
        let mut stage_at: Vec<(OperationStage, Instant)> = Vec::new();
        let mut result = None;
        while Instant::now() < deadline {
            let Some(event) = self.next(deadline.saturating_duration_since(Instant::now())) else {
                break;
            };
            let now = Instant::now();
            if let ServiceEvent::OperationProgress { id, stage } = &event
                && id == op
            {
                stage_at.push((*stage, now));
            }
            let finished = match &event {
                ServiceEvent::OperationFinished {
                    id,
                    result: outcome,
                } if id == op => {
                    result = Some(outcome.clone());
                    true
                }
                _ => false,
            };
            events.push(event);
            if finished {
                return OpRun {
                    op: op.clone(),
                    events,
                    result: result.expect("finished event carries a result"),
                    stage_at,
                };
            }
        }
        panic!(
            "operation {op} did not finish within {timeout:?}; observed: {:?}",
            describe(&events)
        );
    }

    /// Collects every event that arrives within `window`, then returns.
    ///
    /// Used to prove nothing *else* was emitted: "exactly one
    /// `OperationFinished`" is only meaningful if the stream is drained after
    /// the first one.
    pub fn drain_for(&mut self, window: Duration) -> Vec<ServiceEvent> {
        let deadline = Instant::now() + window;
        let mut events = Vec::new();
        while let Some(event) = self.next(deadline.saturating_duration_since(Instant::now())) {
            events.push(event);
        }
        events
    }

    /// Collects events until `pred` holds, or panics at the deadline.
    pub fn until(
        &mut self,
        timeout: Duration,
        what: &str,
        mut pred: impl FnMut(&ServiceEvent) -> bool,
    ) -> Vec<ServiceEvent> {
        let deadline = Instant::now() + timeout;
        let mut events = Vec::new();
        while Instant::now() < deadline {
            let Some(event) = self.next(deadline.saturating_duration_since(Instant::now())) else {
                break;
            };
            let done = pred(&event);
            events.push(event);
            if done {
                return events;
            }
        }
        panic!(
            "never observed {what} within {timeout:?}; saw: {:?}",
            describe(&events)
        );
    }
}

/// Compact rendering of events for assertion messages.
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
            ServiceEvent::DownloadProgress { progress, .. } => {
                format!("Download({}/{})", progress.bytes_done, progress.bytes_total)
            }
            ServiceEvent::InstanceRemoved { .. } => "Removed".to_owned(),
            ServiceEvent::StopForced { .. } => "StopForced".to_owned(),
            ServiceEvent::CatalogUpdated { .. } => "CatalogUpdated".to_owned(),
            ServiceEvent::LogLines { .. } => "LogLines".to_owned(),
            other => format!("{other:?}"),
        })
        .collect()
}
