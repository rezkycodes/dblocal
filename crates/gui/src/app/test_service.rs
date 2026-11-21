//! A deterministic `DblService` for GPUI tests.
//!
//! `MockService` completes operations on tokio threads and emits from there.
//! gpui's test scheduler treats a wake from a thread it does not own as
//! non-determinism and fails the test, so any handler test that used the mock
//! demo service was flaky under parallel test threads. This service answers
//! every call inline and never emits, which is all the handler tests need.

#![cfg(test)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use dblocal_core::error::Result;
use dblocal_core::model::{
    ArchivedData, AuthMode, DeleteMode, EngineKey, InstanceId, InstanceName, InstanceSpec,
    InstanceStatus, Port, Runtime,
};
use dblocal_core::secret::SecretString;
use dblocal_core::service::{
    CatalogView, DblService, InstanceView, OperationId, ServiceEvent, UriFormat,
};
use tokio::sync::broadcast;

/// The password every URI this service returns carries.
pub(crate) const SECRET: &str = "p@ss:w/rd%";

/// An emit-free service with a fixed set of instances.
#[derive(Clone)]
pub(crate) struct QuietService {
    instances: Arc<Vec<InstanceView>>,
    runtimes: Arc<Vec<Runtime>>,
    calls: Arc<AtomicUsize>,
    tx: broadcast::Sender<ServiceEvent>,
}

impl QuietService {
    /// One running, one stopped and one failed instance.
    pub(crate) fn three() -> Self {
        let statuses = [
            ("pg-run", EngineKey::Postgresql, InstanceStatus::Running),
            ("pg-stop", EngineKey::Postgresql, InstanceStatus::Stopped),
            ("pg-fail", EngineKey::Postgresql, InstanceStatus::Failed),
        ];
        let mut instances = Vec::with_capacity(statuses.len());
        for (name, engine, status) in statuses {
            let spec = InstanceSpec::new(
                InstanceName::new(name).expect("name"),
                engine,
                "17.4",
                Some(Port::new(5432).expect("port")),
                AuthMode::Password,
            )
            .expect("spec");
            let instance = dblocal_core::model::Instance::new(
                &spec,
                InstanceId::new(),
                status,
                std::time::SystemTime::UNIX_EPOCH,
            );
            instances.push(InstanceView::from_instance(&instance));
        }
        let (tx, _) = broadcast::channel(8);
        Self {
            instances: Arc::new(instances),
            runtimes: Arc::new(Vec::new()),
            calls: Arc::new(AtomicUsize::new(0)),
            tx,
        }
    }

    /// Number of service calls served so far.
    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The id of the first instance in the given status.
    pub(crate) fn id_with(&self, status: InstanceStatus) -> InstanceId {
        self.instances
            .iter()
            .find(|view| view.status == status)
            .map(|view| view.id.clone())
            .expect("an instance in that status")
    }

    fn bump(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

impl DblService for QuietService {
    fn list_instances(&self) -> Result<Vec<InstanceView>> {
        self.bump();
        Ok(self.instances.as_ref().clone())
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
        Ok(4096)
    }
    fn set_autostart(&self, _id: &InstanceId, _enabled: bool) -> Result<()> {
        self.bump();
        Ok(())
    }
    fn cancel_operation(&self, _id: &OperationId) -> Result<()> {
        self.bump();
        Ok(())
    }
    fn connection_uri(&self, id: &InstanceId, format: UriFormat) -> Result<SecretString> {
        self.bump();
        let engine = self
            .instances
            .iter()
            .find(|view| &view.id == id)
            .map(|view| view.engine)
            .unwrap_or(EngineKey::Postgresql);
        let credential = dblocal_core::engine::Credential::new("postgres", SecretString::new(SECRET));
        Ok(match format {
            UriFormat::DotEnv => {
                dblocal_core::engine::uri::build_dotenv(engine, engine.canonical_port(), &credential)
            }
            _ => dblocal_core::engine::uri::build_uri(engine, engine.canonical_port(), &credential),
        })
    }
    fn tail_logs(&self, _id: &InstanceId, _lines: usize) -> Result<Vec<String>> {
        self.bump();
        Ok(vec![
            "starting PostgreSQL".to_owned(),
            "database system is ready to accept connections".to_owned(),
        ])
    }
    fn watch_logs(&self, _id: &InstanceId, _enabled: bool) -> Result<()> {
        self.bump();
        Ok(())
    }
    fn list_runtimes(&self) -> Result<Vec<Runtime>> {
        self.bump();
        Ok(self.runtimes.as_ref().clone())
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
