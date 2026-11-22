//! Headless tests proving DblService calls do not block the GPUI thread.

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

#[derive(Clone)]
struct SlowService {
    delay: Duration,
    calls: Arc<AtomicUsize>,
    instances: Arc<std::sync::Mutex<Vec<InstanceView>>>,
    tx: broadcast::Sender<ServiceEvent>,
}

impl SlowService {
    fn new(delay: Duration) -> Self {
        let (tx, _) = broadcast::channel(64);
        let spec = InstanceSpec::new(
            InstanceName::new("pg-slow").expect("name"),
            EngineKey::Postgresql,
            "17.4",
            Some(Port::new(5432).expect("port")),
            AuthMode::Password,
        )
        .expect("spec");
        let inst = dblocal_core::model::Instance::new(
            &spec,
            InstanceId::new(),
            InstanceStatus::Stopped,
            std::time::SystemTime::UNIX_EPOCH,
        );
        Self {
            delay,
            calls: Arc::new(AtomicUsize::new(0)),
            instances: Arc::new(std::sync::Mutex::new(vec![InstanceView::from_instance(
                &inst,
            )])),
            tx,
        }
    }
    fn bump(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
    }
}

impl DblService for SlowService {
    fn list_instances(&self) -> Result<Vec<InstanceView>> {
        self.bump();
        Ok(self.instances.lock().unwrap().clone())
    }
    fn catalog(&self) -> Result<CatalogView> {
        self.bump();
        Ok(CatalogView {
            status: dblocal_core::model::CatalogStatus::Fresh,
            banner: None,
            engines: Vec::new(),
        })
    }
    fn refresh_catalog(&self, _force: bool) -> Result<OperationId> {
        self.bump();
        Ok(OperationId::new("op-refresh"))
    }
    fn validate_spec(&self, _spec: &InstanceSpec) -> Result<()> {
        self.bump();
        Ok(())
    }
    fn suggest_port(&self, _engine: EngineKey) -> Result<Port> {
        self.bump();
        Port::new(5433)
    }
    fn create_instance(&self, _spec: InstanceSpec) -> Result<OperationId> {
        self.bump();
        Ok(OperationId::new("op-create"))
    }
    fn start_instance(&self, _id: &InstanceId) -> Result<OperationId> {
        self.bump();
        Ok(OperationId::new("op-start"))
    }
    fn stop_instance(&self, _id: &InstanceId) -> Result<OperationId> {
        self.bump();
        Ok(OperationId::new("op-stop"))
    }
    fn delete_instance(&self, _id: &InstanceId, _mode: DeleteMode) -> Result<OperationId> {
        self.bump();
        Ok(OperationId::new("op-del"))
    }
    fn change_port(&self, _id: &InstanceId, _port: Port) -> Result<()> {
        self.bump();
        Ok(())
    }
    fn data_size(&self, _id: &InstanceId) -> Result<u64> {
        self.bump();
        Ok(0)
    }
    fn set_autostart(&self, _id: &InstanceId, _enabled: bool) -> Result<()> {
        self.bump();
        Ok(())
    }
    fn cancel_operation(&self, _id: &OperationId) -> Result<()> {
        self.bump();
        Ok(())
    }
    fn connection_uri(&self, _id: &InstanceId, _format: UriFormat) -> Result<SecretString> {
        self.bump();
        Ok(SecretString::new(
            "postgresql://postgres:secret@127.0.0.1:5432/postgres",
        ))
    }
    fn tail_logs(&self, _id: &InstanceId, _lines: usize) -> Result<Vec<String>> {
        self.bump();
        Ok(vec!["line".to_owned()])
    }
    fn watch_logs(&self, _id: &InstanceId, _enabled: bool) -> Result<()> {
        self.bump();
        Ok(())
    }
    fn list_runtimes(&self) -> Result<Vec<Runtime>> {
        self.bump();
        Ok(Vec::new())
    }
    fn remove_runtime(&self, _runtime: &str) -> Result<()> {
        self.bump();
        Ok(())
    }
    fn list_archives(&self) -> Result<Vec<ArchivedData>> {
        self.bump();
        Ok(Vec::new())
    }
    fn purge_archive(&self, _archive: &str) -> Result<()> {
        self.bump();
        Ok(())
    }
    fn subscribe(&self) -> broadcast::Receiver<ServiceEvent> {
        self.tx.subscribe()
    }
}

#[gpui_kit::test]
fn slow_service_calls_do_not_block_gpui(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(SlowService::new(Duration::from_secs(2)));
    let calls = service.calls.clone();
    let svc: Arc<dyn DblService> = service.clone();

    let (view, _cx_handle) = cx.add_window_view(|window, cx| Workspace::new_preview(svc, window, cx));
    cx.run_until_parked();

    let start = Instant::now();
    view.update(cx, |this, cx| {
        this.refresh_instances_bg(cx);
    });
    view.update(cx, |this, _cx| {
        this.toggle_palette();
    });
    let after_toggle = Instant::now();
    assert!(
        after_toggle.duration_since(start) < Duration::from_millis(500),
        "UI blocked by service call"
    );

    std::thread::sleep(Duration::from_millis(2200));
    cx.run_until_parked();
    cx.run_until_parked();

    assert!(
        calls.load(Ordering::SeqCst) >= 1,
        "service was never called"
    );
    let palette_open = view.read_with(cx, |this, _| this.palette_is_open());
    assert!(palette_open);
}
