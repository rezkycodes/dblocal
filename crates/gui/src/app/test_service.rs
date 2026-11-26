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
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use dblocal_core::error::Result;
use dblocal_core::model::{
    ArchivedData, AuthMode, DeleteMode, EngineKey, InstanceId, InstanceName, InstanceSpec,
    InstanceStatus, Port, Runtime,
};
use dblocal_core::secret::SecretString;
use dblocal_core::service::{
    CatalogView, DblService, EngineEntry, InstanceView, OperationId, ServiceEvent, UriFormat,
    VersionEntry,
};
use tokio::sync::broadcast;

/// The password every URI this service returns carries.
pub(crate) const SECRET: &str = "p@ss:w/rd%";

/// A deterministic platform-filtered catalog with one engine (PostgreSQL) and
/// two versions, the newer installed. Enough for the create dialog to draw
/// engine cards and version rows without a network.
pub(crate) fn test_catalog() -> Vec<EngineEntry> {
    vec![EngineEntry {
        engine: EngineKey::Postgresql,
        available: true,
        unavailable_reason: None,
        versions: vec![
            VersionEntry {
                version: "17.4".to_owned(),
                line: "17".to_owned(),
                eol: false,
                status: None,
                installed: true,
                size_bytes: 0,
            },
            VersionEntry {
                version: "16.6".to_owned(),
                line: "16".to_owned(),
                eol: false,
                status: None,
                installed: false,
                size_bytes: 1024,
            },
        ],
    }]
}

/// An emit-free service with a fixed set of instances.
#[derive(Clone)]
pub(crate) struct QuietService {
    instances: Arc<Vec<InstanceView>>,
    runtimes: Arc<Vec<Runtime>>,
    calls: Arc<AtomicUsize>,
    /// Every `create_instance` spec, in order (F3: assert the spec chosen by key).
    created: Arc<Mutex<Vec<InstanceSpec>>>,
    /// Every `start_instance` id, in order (F3: assert the started instance).
    started: Arc<Mutex<Vec<InstanceId>>>,
    /// Every `stop_instance` id, in order (G1: assert the stopped instance).
    stopped: Arc<Mutex<Vec<InstanceId>>>,
    /// Every `connection_uri` request `(id, format)`, in order (F3: copy URI).
    uris: Arc<Mutex<Vec<(InstanceId, UriFormat)>>>,
    /// Every `delete_instance` request `(id, mode)`, in order (W4-I D1: the
    /// dialog must pass the mode the user chose for the instance they opened).
    deleted: Arc<Mutex<Vec<(InstanceId, DeleteMode)>>>,
    /// Ids a delete removed, hidden from later `list_instances` reads so the
    /// sidebar refresh after a delete shows the row gone (as the real store does).
    removed: Arc<Mutex<Vec<InstanceId>>>,
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
            created: Arc::new(Mutex::new(Vec::new())),
            started: Arc::new(Mutex::new(Vec::new())),
            stopped: Arc::new(Mutex::new(Vec::new())),
            uris: Arc::new(Mutex::new(Vec::new())),
            deleted: Arc::new(Mutex::new(Vec::new())),
            removed: Arc::new(Mutex::new(Vec::new())),
            tx,
        }
    }

    /// Number of service calls served so far.
    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Every `create_instance` spec, in call order.
    pub(crate) fn created(&self) -> Vec<InstanceSpec> {
        self.created.lock().expect("created lock").clone()
    }

    /// Every `start_instance` id, in call order.
    pub(crate) fn started(&self) -> Vec<InstanceId> {
        self.started.lock().expect("started lock").clone()
    }

    /// Every `stop_instance` id, in call order.
    pub(crate) fn stopped(&self) -> Vec<InstanceId> {
        self.stopped.lock().expect("stopped lock").clone()
    }

    /// Every `connection_uri` request, in call order.
    pub(crate) fn uris(&self) -> Vec<(InstanceId, UriFormat)> {
        self.uris.lock().expect("uris lock").clone()
    }

    /// Every `delete_instance` request `(id, mode)`, in call order.
    pub(crate) fn deleted(&self) -> Vec<(InstanceId, DeleteMode)> {
        self.deleted.lock().expect("deleted lock").clone()
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
        let removed = self.removed.lock().expect("removed lock");
        Ok(self
            .instances
            .iter()
            .filter(|view| !removed.contains(&view.id))
            .cloned()
            .collect())
    }
    fn catalog(&self) -> Result<CatalogView> {
        self.bump();
        Ok(CatalogView {
            status: dblocal_core::model::CatalogStatus::Fresh,
            banner: None,
            engines: test_catalog(),
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
    fn create_instance(&self, spec: InstanceSpec) -> Result<OperationId> {
        self.bump();
        self.created.lock().expect("created lock").push(spec);
        Ok(OperationId::new("op-create"))
    }
    fn start_instance(&self, id: &InstanceId) -> Result<OperationId> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.lock().expect("started lock").push(id.clone());
        Ok(OperationId::new(format!("op-start-{n}")))
    }
    fn stop_instance(&self, id: &InstanceId) -> Result<OperationId> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        self.stopped.lock().expect("stopped lock").push(id.clone());
        Ok(OperationId::new(format!("op-stop-{n}")))
    }
    fn delete_instance(&self, id: &InstanceId, mode: DeleteMode) -> Result<OperationId> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        self.deleted
            .lock()
            .expect("deleted lock")
            .push((id.clone(), mode));
        // The real store drops the row when a delete completes. Hiding it here
        // makes the post-delete sidebar refresh show it gone, exactly as the
        // product does, without a second event.
        self.removed.lock().expect("removed lock").push(id.clone());
        Ok(OperationId::new(format!("op-del-{n}")))
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
        self.uris
            .lock()
            .expect("uris lock")
            .push((id.clone(), format));
        let engine = self
            .instances
            .iter()
            .find(|view| &view.id == id)
            .map(|view| view.engine)
            .unwrap_or(EngineKey::Postgresql);
        let credential =
            dblocal_core::engine::Credential::new("postgres", SecretString::new(SECRET));
        Ok(match format {
            UriFormat::DotEnv => dblocal_core::engine::uri::build_dotenv(
                engine,
                engine.canonical_port(),
                &credential,
            ),
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

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use dblocal_core::model::{DeleteMode, EngineKey, InstanceStatus, Port};
    use dblocal_core::service::{DblService, UriFormat};

    use super::{QuietService, SECRET};

    /// Every trait method answers inline, so a caller never waits on a thread —
    /// this is what makes the handler tests deterministic.
    #[test]
    fn every_trait_method_answers_immediately() {
        let service = QuietService::three();
        let running = service.id_with(InstanceStatus::Running);
        let spec = dblocal_core::model::InstanceSpec::new(
            dblocal_core::model::InstanceName::new("pg-new").expect("name"),
            EngineKey::Postgresql,
            "17.4",
            Some(Port::new(5500).expect("port")),
            dblocal_core::model::AuthMode::Password,
        )
        .expect("spec");

        assert_eq!(service.list_instances().expect("instances").len(), 3);
        assert!(!service.catalog().expect("catalog").engines.is_empty());
        assert_eq!(
            service.refresh_catalog(false).expect("refresh").as_str(),
            "op-refresh"
        );
        assert!(service.validate_spec(&spec).is_ok());
        assert_eq!(
            service.suggest_port(EngineKey::Mysql).expect("port").get(),
            5433
        );
        assert!(service.create_instance(spec).is_ok());
        assert!(service.start_instance(&running).is_ok());
        assert!(service.stop_instance(&running).is_ok());
        assert!(
            service
                .delete_instance(&running, DeleteMode::ArchiveData)
                .is_ok()
        );
        assert!(
            service
                .change_port(&running, Port::new(5501).expect("port"))
                .is_ok()
        );
        assert_eq!(service.data_size(&running).expect("size"), 4096);
        assert!(service.set_autostart(&running, true).is_ok());
        assert!(
            service
                .cancel_operation(&dblocal_core::service::OperationId::new("op"))
                .is_ok()
        );
        assert!(service.list_runtimes().expect("runtimes").is_empty());
        assert!(service.remove_runtime("none").is_ok());
        assert!(service.list_archives().expect("archives").is_empty());
        assert!(service.purge_archive("none").is_ok());
        assert!(service.watch_logs(&running, true).is_ok());
        assert_eq!(service.tail_logs(&running, 10).expect("logs").len(), 2);
        assert_eq!(service.subscribe().len(), 0);
    }

    /// Both URI formats round-trip the real builder, with the secret encoded.
    #[test]
    fn connection_uri_uses_the_real_builder_for_both_formats() {
        let service = QuietService::three();
        let id = service.id_with(InstanceStatus::Running);
        let encoded = dblocal_core::engine::uri::encode_userinfo(SECRET);

        let uri = service.connection_uri(&id, UriFormat::Uri).expect("uri");
        assert!(uri.expose().starts_with("postgresql://"));
        assert!(
            uri.expose().contains(&encoded),
            "the URI must carry the encoded secret: {}",
            uri.expose()
        );

        let dotenv = service
            .connection_uri(&id, UriFormat::DotEnv)
            .expect("dotenv");
        assert!(
            dotenv.expose().contains('='),
            "a .env block has KEY=value rows"
        );
        assert!(
            !format!("{dotenv:?}").contains(SECRET),
            "Debug must hide it"
        );
    }

    /// The counter is how the handler tests prove a call was (or was not) made.
    #[test]
    fn the_call_counter_counts() {
        let service = QuietService::three();
        assert_eq!(service.calls(), 0);
        let _ = service.list_instances().expect("instances");
        let _ = service.catalog().expect("catalog");
        assert_eq!(service.calls(), 2);
    }
}
