//! Bridging `DblService` events into the UI without polling.
//!
//! The service emits on a `tokio::sync::broadcast` receiver. A dedicated tokio
//! runtime (its own thread, never the GPUI executor) forwards events into a
//! `flume` channel; the GPUI side parks on `recv_async()` and wakes only when an
//! event actually arrives. Nothing here runs a timer while idle.

use std::sync::Arc;
use std::thread;

use dblocal_core::model::{DownloadProgress, InstanceId};
use dblocal_core::service::{
    DblService, InstanceView, OperationId, OperationStage, ServiceEvent, UserFacingError,
};
use flume::{Receiver, Sender};
use tokio::sync::broadcast::error::RecvError;

/// An event the UI thread must react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    /// One instance changed; re-render its row, tab and detail.
    InstanceChanged(InstanceView),
    /// A long-running operation progressed.
    OperationProgress {
        /// The operation.
        id: OperationId,
        /// New stage.
        stage: OperationStage,
    },
    /// A download sample, keyed by operation.
    DownloadProgress {
        /// The operation downloading.
        id: OperationId,
        /// Latest sample.
        progress: DownloadProgress,
    },
    /// An operation finished; `result` carries the user-facing failure.
    OperationFinished {
        /// The operation.
        id: OperationId,
        /// Outcome.
        result: Result<(), UserFacingError>,
    },
    /// Catalog freshness changed.
    CatalogUpdated,
    /// New lines for a watched instance.
    LogLines {
        /// The instance.
        instance: String,
        /// Appended lines, oldest first.
        lines: Vec<String>,
    },
    /// An instance no longer exists; drop its row, tab and log buffer.
    InstanceRemoved(InstanceId),
    /// The databases or users of one instance changed; re-read them (`F-DBU`).
    AdminChanged(InstanceId),
    /// An update check finished; the Settings dialog renders the result.
    UpdateChecked {
        /// Newer version, or `None` when the running version is current.
        version: Option<String>,
        /// Whether this install may self-update.
        can_self_update: bool,
    },
    /// A stop escalated to a forced kill; tell the user it was forced.
    StopForced(InstanceId),
    /// The service stream lagged and the UI may have missed changes.
    ///
    /// `broadcast` drops the oldest events of a slow receiver. After a lag the
    /// visible state can be stale forever (a row stuck "berjalan"), so the
    /// workspace re-reads `list_instances()` and the catalog when it sees this.
    Resync,
    /// The service event stream ended.
    Closed,
}

/// Forwards service events into a flume channel from a background runtime.
///
/// The returned [`Receiver`] is safe to await on the GPUI side; the background
/// thread stops when the service channel closes or the receiver is dropped.
pub fn spawn_bridge(service: Arc<dyn DblService>) -> Receiver<UiEvent> {
    let (tx, rx) = flume::unbounded::<UiEvent>();
    // Subscribe before starting the thread so no early event is missed.
    let events = service.subscribe();
    let builder = thread::Builder::new().name("dblocal-service-bridge".to_owned());
    let handle = builder.spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::error!("could not build the service bridge runtime: {err}");
                let _ = tx.send(UiEvent::Closed);
                return;
            }
        };
        runtime.block_on(forward(events, tx));
    });
    if let Err(err) = handle {
        tracing::error!("could not start the service bridge thread: {err}");
    }
    rx
}

async fn forward(mut events: tokio::sync::broadcast::Receiver<ServiceEvent>, tx: Sender<UiEvent>) {
    loop {
        match events.recv().await {
            Ok(event) => {
                if let Some(ui) = translate(event)
                    && tx.send(ui).is_err()
                {
                    // The UI dropped the receiver; stop forwarding.
                    return;
                }
            }
            Err(RecvError::Lagged(skipped)) => {
                tracing::warn!(
                    "service event stream lagged; dropped {skipped} events, requesting a resync"
                );
                if tx.send(UiEvent::Resync).is_err() {
                    return;
                }
            }
            Err(RecvError::Closed) => {
                let _ = tx.send(UiEvent::Closed);
                return;
            }
        }
    }
}

/// Maps one service event to the UI's event shape.
pub fn translate(event: ServiceEvent) -> Option<UiEvent> {
    match event {
        ServiceEvent::InstanceChanged { view } => Some(UiEvent::InstanceChanged(view)),
        ServiceEvent::OperationProgress { id, stage } => {
            Some(UiEvent::OperationProgress { id, stage })
        }
        ServiceEvent::DownloadProgress { id, progress } => {
            Some(UiEvent::DownloadProgress { id, progress })
        }
        ServiceEvent::OperationFinished { id, result } => {
            Some(UiEvent::OperationFinished { id, result })
        }
        ServiceEvent::InstanceRemoved { id } => Some(UiEvent::InstanceRemoved(id)),
        ServiceEvent::StopForced { id } => Some(UiEvent::StopForced(id)),
        ServiceEvent::CatalogUpdated { .. } => Some(UiEvent::CatalogUpdated),
        ServiceEvent::LogLines { id, lines } => Some(UiEvent::LogLines {
            instance: id.to_string(),
            lines,
        }),
        ServiceEvent::AdminChanged { id } => Some(UiEvent::AdminChanged(id)),
        ServiceEvent::UpdateChecked {
            version,
            can_self_update,
        } => Some(UiEvent::UpdateChecked {
            version,
            can_self_update,
        }),
        _ => None,
    }
}

/// Sends an event to the UI thread, ignoring a closed channel.
pub fn send(tx: &Sender<UiEvent>, event: UiEvent) {
    if tx.send(event).is_err() {
        tracing::debug!("UI event receiver is gone; dropping event");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use dblocal_core::model::{
        AuthMode, CatalogStatus, EngineKey, InstanceId, InstanceName, InstanceSpec, InstanceStatus,
        Port,
    };
    use dblocal_core::service::UserFacingError;

    use super::*;

    fn view() -> InstanceView {
        let spec = InstanceSpec::new(
            InstanceName::new("bridge").expect("name"),
            EngineKey::Postgresql,
            "17.4",
            Some(Port::new(5432).expect("port")),
            AuthMode::Password,
        )
        .expect("spec");
        let instance = dblocal_core::model::Instance::new(
            &spec,
            InstanceId::new(),
            InstanceStatus::Running,
            std::time::SystemTime::UNIX_EPOCH,
        );
        InstanceView::from_instance(&instance)
    }

    #[test]
    fn translate_maps_each_event_kind() {
        let changed = translate(ServiceEvent::InstanceChanged { view: view() });
        assert!(matches!(changed, Some(UiEvent::InstanceChanged(_))));

        let progress = translate(ServiceEvent::OperationProgress {
            id: OperationId::new("op"),
            stage: OperationStage::Downloading,
        });
        assert!(matches!(progress, Some(UiEvent::OperationProgress { .. })));

        let catalog = translate(ServiceEvent::CatalogUpdated {
            status: CatalogStatus::Stale,
        });
        assert!(matches!(catalog, Some(UiEvent::CatalogUpdated)));

        let logs = translate(ServiceEvent::LogLines {
            id: InstanceId::new(),
            lines: vec!["line".to_owned()],
        });
        match logs {
            Some(UiEvent::LogLines { lines, .. }) => assert_eq!(lines.len(), 1),
            other => panic!("unexpected {other:?}"),
        }

        let finished = translate(ServiceEvent::OperationFinished {
            id: OperationId::new("op"),
            result: Err(UserFacingError::new("DBL-1005", "busy")),
        });
        assert!(matches!(finished, Some(UiEvent::OperationFinished { .. })));
    }

    #[test]
    fn translate_maps_removal_and_forced_stop() {
        let id = InstanceId::new();
        assert!(matches!(
            translate(ServiceEvent::InstanceRemoved { id: id.clone() }),
            Some(UiEvent::InstanceRemoved(removed)) if removed == id
        ));
        assert!(matches!(
            translate(ServiceEvent::StopForced { id: id.clone() }),
            Some(UiEvent::StopForced(forced)) if forced == id
        ));
    }

    #[test]
    fn translate_maps_download_progress() {
        let event = translate(ServiceEvent::DownloadProgress {
            id: OperationId::new("op"),
            progress: dblocal_core::model::DownloadProgress::new(1, 2, 3, None),
        });
        match event {
            Some(UiEvent::DownloadProgress { progress, .. }) => {
                assert_eq!(progress.bytes_done, 1);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn bridge_forwards_a_real_service_event() {
        use dblocal_core::service::mock::MockService;

        let service = MockService::deterministic();
        let id = service.seed_instance(
            InstanceSpec::new(
                InstanceName::new("real").expect("name"),
                EngineKey::Postgresql,
                "17.4",
                Some(Port::new(5432).expect("port")),
                AuthMode::Password,
            )
            .expect("spec"),
            InstanceStatus::Stopped,
        );
        let shared: Arc<dyn DblService> = Arc::new(service.clone());
        let rx = spawn_bridge(shared);

        // Starting the instance must reach the receiver as an InstanceChanged.
        service.start_instance(&id).expect("start");
        let received = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match rx.recv_async().await {
                    Ok(UiEvent::InstanceChanged(view)) => return Some(view),
                    Ok(_) => continue,
                    Err(_) => return None,
                }
            }
        })
        .await
        .expect("bridge must deliver an event");
        assert!(received.is_some());
    }

    #[tokio::test]
    async fn lagging_the_stream_emits_a_resync() {
        // A 1-slot channel: send a burst without draining the receiver so the
        // next `recv` reports `Lagged`, which must surface as `Resync`.
        let (event_tx, event_rx) = tokio::sync::broadcast::channel::<ServiceEvent>(1);
        let (ui_tx, ui_rx) = flume::unbounded::<UiEvent>();
        for _ in 0..5 {
            let _ = event_tx.send(ServiceEvent::CatalogUpdated {
                status: CatalogStatus::Fresh,
            });
        }
        drop(event_tx);

        // `forward` returns once the broadcast channel is closed and drained.
        let bridge = tokio::spawn(forward(event_rx, ui_tx));
        bridge.await.expect("forward completes");

        let events: Vec<UiEvent> = ui_rx.try_iter().collect();
        let resyncs = events
            .iter()
            .filter(|event| matches!(event, UiEvent::Resync))
            .count();
        eprintln!("bridge lag test: {} events, {resyncs} Resync", events.len());
        assert_eq!(resyncs, 1, "a lagged stream must emit exactly one Resync");
        assert!(
            events
                .iter()
                .any(|event| matches!(event, UiEvent::CatalogUpdated)),
            "the drain must still deliver the newest event"
        );
        assert!(matches!(events.last(), Some(UiEvent::Closed)));
    }

    #[test]
    fn send_into_a_closed_channel_is_a_no_op() {
        let (tx, rx) = flume::unbounded::<UiEvent>();
        drop(rx);
        send(&tx, UiEvent::CatalogUpdated);
    }
}
