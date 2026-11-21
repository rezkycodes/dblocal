//! End-to-end tray-to-window latency through the production path.
//!
//! This is the F-TRY-01 AC2 measurement, taken on the real plumbing rather than
//! a 50 ms pump: a tray action enters the same `flume` channel the ksni backend
//! uses, the workspace's tray pump wakes on it, the service call runs on the
//! background executor, and the `InstanceChanged` arrives through
//! `service_bridge` and is applied. There is no timer between those steps.
//!
//! Run with the report's command:
//! `cargo test -p dblocal-gui --lib tray_latency -- --nocapture`

#![cfg(test)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use dblocal_core::error::Result;
use dblocal_core::model::{
    ArchivedData, AuthMode, DeleteMode, EngineKey, InstanceId, InstanceName, InstanceSpec,
    InstanceStatus, Port, Runtime,
};
use dblocal_core::secret::SecretString;
use dblocal_core::service::{
    CatalogView, DblService, InstanceView, OperationId, ServiceEvent, UriFormat,
};
use gpui_kit::TestAppContext;
use tokio::sync::broadcast;

use super::Workspace;
use crate::tray::TrayAction;

/// A service that emits `InstanceChanged(Running)` shortly after `start_instance`,
/// like `LocalService` does, through the real event channel.
#[derive(Clone)]
struct EchoService {
    tx: broadcast::Sender<ServiceEvent>,
    view: Arc<std::sync::Mutex<InstanceView>>,
    starts: Arc<AtomicUsize>,
}

impl EchoService {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        let spec = InstanceSpec::new(
            InstanceName::new("pg-tray").expect("name"),
            EngineKey::Postgresql,
            "17.4",
            Some(Port::new(5432).expect("port")),
            AuthMode::Password,
        )
        .expect("spec");
        let instance = dblocal_core::model::Instance::new(
            &spec,
            InstanceId::new(),
            InstanceStatus::Stopped,
            std::time::SystemTime::UNIX_EPOCH,
        );
        Self {
            tx,
            view: Arc::new(std::sync::Mutex::new(InstanceView::from_instance(
                &instance,
            ))),
            starts: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl DblService for EchoService {
    fn list_instances(&self) -> Result<Vec<InstanceView>> {
        Ok(vec![self.view.lock().unwrap().clone()])
    }
    fn catalog(&self) -> Result<CatalogView> {
        Ok(CatalogView {
            status: dblocal_core::model::CatalogStatus::Fresh,
            banner: None,
            engines: Vec::new(),
        })
    }
    fn refresh_catalog(&self, _force: bool) -> Result<OperationId> {
        Ok(OperationId::new("op"))
    }
    fn validate_spec(&self, _spec: &InstanceSpec) -> Result<()> {
        Ok(())
    }
    fn suggest_port(&self, engine: EngineKey) -> Result<Port> {
        Port::new(engine.canonical_port())
    }
    fn create_instance(&self, _spec: InstanceSpec) -> Result<OperationId> {
        Ok(OperationId::new("op"))
    }
    fn start_instance(&self, _id: &InstanceId) -> Result<OperationId> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        let mut view = self.view.lock().unwrap();
        view.status = InstanceStatus::Running;
        let _ = self
            .tx
            .send(ServiceEvent::InstanceChanged { view: view.clone() });
        Ok(OperationId::new("op-start"))
    }
    fn stop_instance(&self, _id: &InstanceId) -> Result<OperationId> {
        Ok(OperationId::new("op-stop"))
    }
    fn delete_instance(&self, _id: &InstanceId, _mode: DeleteMode) -> Result<OperationId> {
        Ok(OperationId::new("op-del"))
    }
    fn change_port(&self, _id: &InstanceId, _port: Port) -> Result<()> {
        Ok(())
    }
    fn data_size(&self, _id: &InstanceId) -> Result<u64> {
        Ok(0)
    }
    fn set_autostart(&self, _id: &InstanceId, _enabled: bool) -> Result<()> {
        Ok(())
    }
    fn cancel_operation(&self, _id: &OperationId) -> Result<()> {
        Ok(())
    }
    fn connection_uri(&self, _id: &InstanceId, _format: UriFormat) -> Result<SecretString> {
        Ok(SecretString::new(
            "postgresql://postgres:p@127.0.0.1:5432/postgres",
        ))
    }
    fn tail_logs(&self, _id: &InstanceId, _lines: usize) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn watch_logs(&self, _id: &InstanceId, _enabled: bool) -> Result<()> {
        Ok(())
    }
    fn list_runtimes(&self) -> Result<Vec<Runtime>> {
        Ok(Vec::new())
    }
    fn remove_runtime(&self, _runtime: &str) -> Result<()> {
        Ok(())
    }
    fn list_archives(&self) -> Result<Vec<ArchivedData>> {
        Ok(Vec::new())
    }
    fn purge_archive(&self, _archive: &str) -> Result<()> {
        Ok(())
    }
    fn subscribe(&self) -> broadcast::Receiver<ServiceEvent> {
        self.tx.subscribe()
    }
}

#[gpui_kit::test]
fn tray_action_reaches_the_window_fast(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(EchoService::new());
    let shared: Arc<dyn DblService> = service.clone();
    let (view, _handle) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::new(shared, window, cx);
        workspace.seed_state(service.list_instances().unwrap());
        workspace
    });
    cx.run_until_parked();

    let id = {
        let state = view.read_with(cx, |this, _| this.instances().to_vec());
        state.first().expect("seeded instance").id.clone()
    };

    // One discarded warm-up: the first event wakes the bridge thread and pays
    // the first SQLite/OS touch. W0-C measured after warm-up for the same reason.
    view.update(cx, |this, _| this.reset_for_latency(&id));
    cx.run_until_parked();
    let sender = view.read_with(cx, |this, _| this.tray_sender());
    sender
        .send(TrayAction::Start(id.to_string()))
        .expect("tray channel");
    for _ in 0..3000 {
        cx.run_until_parked();
        if view.read_with(cx, |this, _| {
            this.instance(&id)
                .map(|v| v.status == InstanceStatus::Running)
                .unwrap_or(false)
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    let mut samples = Vec::new();
    for _ in 0..20 {
        // Reset to Stopped so each round really starts.
        view.update(cx, |this, _| {
            this.reset_for_latency(&id);
        });
        cx.run_until_parked();
        // Exactly what the ksni menu activate closure does.
        let sender = view.read_with(cx, |this, _| this.tray_sender());
        let start = Instant::now();
        sender
            .send(TrayAction::Start(id.to_string()))
            .expect("tray channel");
        // Drain the pumps until the window reflects Running.
        for _ in 0..3000 {
            cx.run_until_parked();
            let running = view.read_with(cx, |this, _| {
                this.instance(&id)
                    .map(|v| v.status == InstanceStatus::Running)
                    .unwrap_or(false)
            });
            if running {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let elapsed = start.elapsed();
        assert!(
            view.read_with(cx, |this, _| this
                .instance(&id)
                .map(|v| v.status == InstanceStatus::Running)
                .unwrap_or(false)),
            "round did not land"
        );
        samples.push(elapsed.as_secs_f64() * 1000.0);
    }

    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = samples[samples.len() / 2];
    let max = *samples.last().expect("samples");
    eprintln!(
        "TRAY_LATENCY n={} p50={p50:.1}ms max={max:.1}ms target=1000ms",
        samples.len()
    );
    assert!(max < 1000.0, "tray action exceeded 1 s: max={max:.1}ms");
}
