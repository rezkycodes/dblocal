//! A scriptable `DblService` for the W4-B handler and render tests.
//!
//! [`QuietService`](super::test_service::QuietService) is deliberately minimal
//! (three instances, no admin). The W4-B screens need per-call answers and
//! counts, so this service stores what the test set and records how often each
//! W4-B call ran. It answers inline and never emits, the same rule that keeps
//! the gpui test scheduler deterministic.

#![cfg(test)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use dblocal_core::clients::{ClientKind, DetectedClient, LaunchPlan};
use dblocal_core::error::Result;
use dblocal_core::model::{
    ArchivedData, AuthMode, DeleteMode, EngineKey, InstanceId, InstanceName, InstanceSpec,
    InstanceStatus, Port, Runtime,
};
use dblocal_core::secret::SecretString;
use dblocal_core::service::{
    CatalogView, DatabaseInfo, DblService, InstanceView, OperationId, PrivilegeLevel, ServiceEvent,
    ServiceResult, UpdateStatus, UriFormat, UserFacingError, UserInfo,
};
use tokio::sync::broadcast;

/// How many times each W4-B call ran.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CallCounts {
    /// `list_databases`.
    pub list_databases: usize,
    /// `list_users`.
    pub list_users: usize,
    /// `create_database`.
    pub create_database: usize,
    /// `drop_database`.
    pub drop_database: usize,
    /// `create_user`.
    pub create_user: usize,
    /// `reset_superuser_password`.
    pub reset_password: usize,
    /// `detect_clients`.
    pub detect_clients: usize,
    /// `open_in_client`.
    pub open_in_client: usize,
    /// `check_update`.
    pub check_update: usize,
    /// `apply_update`.
    pub apply_update: usize,
}

/// What each W4-B call answers. Every field is a `Result` so a test can script a
/// failure exactly where it wants one.
#[derive(Clone)]
pub(crate) struct Script {
    /// `list_databases` and `list_users` answer.
    pub databases: ServiceResult<Vec<DatabaseInfo>>,
    /// `list_users` answer, used only when `databases` is `Ok`.
    pub users: ServiceResult<Vec<UserInfo>>,
    /// `create_database` answer.
    pub create_database: ServiceResult<()>,
    /// `drop_database` answer.
    pub drop_database: ServiceResult<()>,
    /// `create_user` answer (the one-time password).
    pub create_user: ServiceResult<SecretString>,
    /// `reset_superuser_password` answer.
    pub reset_password: ServiceResult<SecretString>,
    /// `connection_uri` answer for the reset flow.
    pub uri: ServiceResult<SecretString>,
    /// `detect_clients` answer.
    pub clients: ServiceResult<Vec<DetectedClient>>,
    /// `open_in_client` answer.
    pub launch: ServiceResult<LaunchPlan>,
    /// `check_update` answer.
    pub update: ServiceResult<UpdateStatus>,
    /// `apply_update` answer.
    pub apply_update: ServiceResult<()>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            databases: Ok(sample_databases()),
            users: Ok(sample_users()),
            create_database: Ok(()),
            drop_database: Ok(()),
            create_user: Ok(SecretString::new("pwd-user-0001")),
            reset_password: Ok(SecretString::new("pwd-reset-0001")),
            uri: Ok(SecretString::new(
                "postgresql://postgres:pwd-reset-0001@127.0.0.1:5432/postgres",
            )),
            clients: Ok(sample_clients()),
            launch: Ok(LaunchPlan::new(
                "/usr/bin/psql",
                vec!["postgresql://127.0.0.1:5432/postgres".to_owned()],
                Vec::new(),
                None,
                None,
                None,
            )),
            update: Ok(update_available()),
            apply_update: Ok(()),
        }
    }
}

fn sample_databases() -> Vec<DatabaseInfo> {
    vec![
        DatabaseInfo::new("postgres", "postgres", 8_388_608),
        DatabaseInfo::new("app", "postgres", 1_048_576),
    ]
}

fn sample_users() -> Vec<UserInfo> {
    vec![
        UserInfo::new("postgres", true, true),
        UserInfo::new("app_rw", true, false),
    ]
}

fn sample_clients() -> Vec<DetectedClient> {
    vec![
        DetectedClient::new(
            ClientKind::TablePlus,
            "/usr/bin/tableplus".into(),
            ClientKind::TablePlus.supported_engines(),
        ),
        DetectedClient::new(
            ClientKind::Psql,
            "/usr/bin/psql".into(),
            ClientKind::Psql.supported_engines(),
        ),
    ]
}

fn update_available() -> UpdateStatus {
    UpdateStatus {
        current: "0.8.0".to_owned(),
        available: Some(dblocal_core::service::AvailableUpdateView {
            version: "0.9.0".to_owned(),
            tag: "v0.9.0".to_owned(),
            release_notes: "Catatan".to_owned(),
            asset_name: "dblocal-linux-x86_64.tar.gz".to_owned(),
            size_bytes: 1024,
            can_self_update: true,
        }),
        install_kind: dblocal_core::update::InstallKind::PortableTarball {
            exe_path: "/opt/dblocal/dblocal".into(),
        },
        package_manager_hint: None,
    }
}

/// A service whose W4-B answers the test scripts and whose calls it counts.
#[derive(Clone)]
pub(crate) struct ScriptedService {
    instances: Arc<Vec<InstanceView>>,
    script: Arc<Mutex<Script>>,
    counts: Arc<Mutex<CallCounts>>,
    tx: broadcast::Sender<ServiceEvent>,
}

impl ScriptedService {
    /// One running and one stopped PostgreSQL instance.
    pub(crate) fn new() -> Self {
        let mut instances = Vec::new();
        for (name, status) in [
            ("pg-run", InstanceStatus::Running),
            ("pg-stop", InstanceStatus::Stopped),
        ] {
            let spec = InstanceSpec::new(
                InstanceName::new(name).expect("name"),
                EngineKey::Postgresql,
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
            script: Arc::new(Mutex::new(Script::default())),
            counts: Arc::new(Mutex::new(CallCounts::default())),
            tx,
        }
    }

    /// One field of the current script, changed in place.
    pub(crate) fn set<T>(&self, f: impl FnOnce(&mut Script) -> T) -> T {
        let mut script = self.script.lock().expect("script lock");
        f(&mut script)
    }

    /// The call counts of the current script, a snapshot.
    pub(crate) fn counts(&self) -> CallCounts {
        *self.counts.lock().expect("counts lock")
    }

    /// The id of the first instance in the given status.
    pub(crate) fn id_with(&self, status: InstanceStatus) -> InstanceId {
        self.instances
            .iter()
            .find(|view| view.status == status)
            .map(|view| view.id.clone())
            .expect("an instance in that status")
    }

    fn bump(&self, f: impl FnOnce(&mut CallCounts)) {
        f(&mut self.counts.lock().expect("counts lock"));
    }
}

impl DblService for ScriptedService {
    fn list_instances(&self) -> Result<Vec<InstanceView>> {
        Ok(self.instances.as_ref().clone())
    }
    fn catalog(&self) -> Result<CatalogView> {
        Ok(CatalogView {
            status: dblocal_core::model::CatalogStatus::Fresh,
            banner: None,
            engines: Vec::new(),
        })
    }
    fn refresh_catalog(&self, _force: bool) -> Result<OperationId> {
        Ok(OperationId::new("op-refresh"))
    }
    fn validate_spec(&self, _spec: &InstanceSpec) -> Result<()> {
        Ok(())
    }
    fn suggest_port(&self, _engine: EngineKey) -> Result<Port> {
        Port::new(5433)
    }
    fn create_instance(&self, _spec: InstanceSpec) -> Result<OperationId> {
        Ok(OperationId::new("op-create"))
    }
    fn start_instance(&self, _id: &InstanceId) -> Result<OperationId> {
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
        Ok(4096)
    }
    fn set_autostart(&self, _id: &InstanceId, _enabled: bool) -> Result<()> {
        Ok(())
    }
    fn cancel_operation(&self, _id: &OperationId) -> Result<()> {
        Ok(())
    }
    fn connection_uri(&self, _id: &InstanceId, _format: UriFormat) -> Result<SecretString> {
        let script = self.script.lock().expect("script lock");
        match &script.uri {
            Ok(uri) => Ok(uri.clone()),
            Err(err) => Err(dblocal_core::DblError::InvalidInput {
                field: "scripted",
                reason: format!("{} — {}", err.code, err.message),
            }),
        }
    }
    fn tail_logs(&self, _id: &InstanceId, _lines: usize) -> Result<Vec<String>> {
        Ok(vec!["log line".to_owned()])
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
    fn purge_archive(&self, _id: &str) -> Result<()> {
        Ok(())
    }
    fn subscribe(&self) -> broadcast::Receiver<ServiceEvent> {
        self.tx.subscribe()
    }

    // --- W4-B: admin ---------------------------------------------------------
    fn list_databases(&self, _id: &InstanceId) -> ServiceResult<Vec<DatabaseInfo>> {
        self.bump(|counts| counts.list_databases += 1);
        let script = self.script.lock().expect("script lock");
        match &script.databases {
            Ok(rows) => Ok(rows.clone()),
            Err(err) => Err(clone_error(err)),
        }
    }
    fn create_database(&self, _id: &InstanceId, _name: &str) -> ServiceResult<()> {
        self.bump(|counts| counts.create_database += 1);
        let script = self.script.lock().expect("script lock");
        script
            .create_database
            .as_ref()
            .map(|_| ())
            .map_err(clone_error)
    }
    fn drop_database(&self, _id: &InstanceId, _name: &str, _force: bool) -> ServiceResult<()> {
        self.bump(|counts| counts.drop_database += 1);
        let script = self.script.lock().expect("script lock");
        script
            .drop_database
            .as_ref()
            .map(|_| ())
            .map_err(clone_error)
    }
    fn list_users(&self, _id: &InstanceId) -> ServiceResult<Vec<UserInfo>> {
        self.bump(|counts| counts.list_users += 1);
        let script = self.script.lock().expect("script lock");
        match &script.users {
            Ok(rows) => Ok(rows.clone()),
            Err(err) => Err(clone_error(err)),
        }
    }
    fn create_user(
        &self,
        _id: &InstanceId,
        _name: &str,
        _db: &str,
        _level: PrivilegeLevel,
    ) -> ServiceResult<SecretString> {
        self.bump(|counts| counts.create_user += 1);
        let script = self.script.lock().expect("script lock");
        match &script.create_user {
            Ok(password) => Ok(password.clone()),
            Err(err) => Err(clone_error(err)),
        }
    }
    fn reset_superuser_password(&self, _id: &InstanceId) -> ServiceResult<SecretString> {
        self.bump(|counts| counts.reset_password += 1);
        let script = self.script.lock().expect("script lock");
        match &script.reset_password {
            Ok(password) => Ok(password.clone()),
            Err(err) => Err(clone_error(err)),
        }
    }

    // --- W4-B: updates -------------------------------------------------------
    fn check_update(&self) -> ServiceResult<UpdateStatus> {
        self.bump(|counts| counts.check_update += 1);
        let script = self.script.lock().expect("script lock");
        match &script.update {
            Ok(status) => Ok(status.clone()),
            Err(err) => Err(clone_error(err)),
        }
    }
    fn apply_update(&self) -> ServiceResult<()> {
        self.bump(|counts| counts.apply_update += 1);
        let script = self.script.lock().expect("script lock");
        script
            .apply_update
            .as_ref()
            .map(|_| ())
            .map_err(clone_error)
    }

    // --- W4-B: open in -------------------------------------------------------
    fn detect_clients(&self, _refresh: bool) -> ServiceResult<Vec<DetectedClient>> {
        self.bump(|counts| counts.detect_clients += 1);
        let script = self.script.lock().expect("script lock");
        match &script.clients {
            Ok(clients) => Ok(clients.clone()),
            Err(err) => Err(clone_error(err)),
        }
    }
    fn open_in_client(
        &self,
        _id: &InstanceId,
        _client: ClientKind,
        _dry_run: bool,
    ) -> ServiceResult<LaunchPlan> {
        self.bump(|counts| counts.open_in_client += 1);
        let script = self.script.lock().expect("script lock");
        match &script.launch {
            Ok(plan) => Ok(plan.clone()),
            Err(err) => Err(clone_error(err)),
        }
    }
}

/// `UserFacingError` is `Clone`, so a scripted failure is returned unchanged.
fn clone_error(err: &UserFacingError) -> UserFacingError {
    err.clone()
}
