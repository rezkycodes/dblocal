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
    /// `config_view`.
    pub config_view: usize,
    /// `config_apply`.
    pub config_apply: usize,
    /// `config_restore`.
    pub config_restore: usize,
    /// `config_open_raw`.
    pub config_open_raw: usize,
    /// `config_raw_path`.
    pub config_raw_path: usize,
    /// `start_instance`.
    pub start_instance: usize,
    /// `stop_instance`.
    pub stop_instance: usize,
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
    /// `config_view` answer (the read model the Config tab renders).
    pub config_view: ServiceResult<dblocal_core::config::ConfigView>,
    /// `config_apply` answer.
    pub config_apply: ServiceResult<dblocal_core::config::ConfigApplied>,
    /// `config_restore` answer.
    pub config_restore: ServiceResult<()>,
    /// `config_raw_path` answer.
    pub config_raw_path: ServiceResult<std::path::PathBuf>,
    /// `config_open_raw` answer.
    pub config_open_raw: ServiceResult<dblocal_core::config::OpenCommand>,
    /// `start_instance` answer: when `Err`, the code is returned as an engine
    /// failure (the service's real `start_instance` error shape).
    pub start_instance: ServiceResult<()>,
}

/// The arguments one mutation was called with.
///
/// The GUI's own guarantee (`F-DBU-01` AC3 "never kill a live session") is a
/// constant argument, so a test has to see the value that crossed the seam, not
/// only that the call happened.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Mutations {
    /// `(instance, name, force)` of every `drop_database` call, in order.
    pub drops: Vec<(InstanceId, String, bool)>,
    /// `(instance, name, database, level)` of every `create_user` call.
    pub users: Vec<(InstanceId, String, String, PrivilegeLevel)>,
    /// `(instance, name)` of every `create_database` call.
    pub creates: Vec<(InstanceId, String)>,
    /// `(instance, changes)` of every `config_apply` call, in order.
    pub config_applies: Vec<(InstanceId, Vec<(String, String)>)>,
    /// Instance of every `config_restore` call, in order.
    pub config_restores: Vec<InstanceId>,
    /// Instance of every `config_open_raw` call, in order.
    pub config_opens: Vec<InstanceId>,
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
            config_view: Ok(sample_config_view()),
            config_apply: Ok(sample_config_applied()),
            config_restore: Ok(()),
            config_raw_path: Ok(std::path::PathBuf::from("/mock/pg-run/postgresql.conf")),
            config_open_raw: Ok(dblocal_core::config::OpenCommand::for_os(
                dblocal_core::platform::Os::Linux,
                &std::path::PathBuf::from("/mock/pg-run/postgresql.conf"),
            )),
            start_instance: Ok(()),
        }
    }
}

/// The Config tab's sample read model: a PostgreSQL instance with editable and
/// read-only rows and no backup yet.
fn sample_config_view() -> dblocal_core::config::ConfigView {
    use dblocal_core::config::{ConfigSetting, ConfigView};
    let mut view = ConfigView::new(
        InstanceId::new(),
        EngineKey::Postgresql,
        std::path::PathBuf::from("/mock/pg-run/postgresql.conf"),
    );
    view.settings.push(ConfigSetting {
        key: "max_connections".to_owned(),
        current: Some("100".to_owned()),
        default: "100".to_owned(),
        unit: String::new(),
        restart_required: true,
        description: dblocal_core::config::messages::DESC_MAX_CONNECTIONS_PG.to_owned(),
        readonly: false,
    });
    view.settings.push(ConfigSetting {
        key: "shared_buffers".to_owned(),
        current: None,
        default: "128MB".to_owned(),
        unit: "bytes".to_owned(),
        restart_required: true,
        description: dblocal_core::config::messages::DESC_SHARED_BUFFERS.to_owned(),
        readonly: false,
    });
    view.settings.push(ConfigSetting {
        key: "port".to_owned(),
        current: Some("5432".to_owned()),
        default: "5432".to_owned(),
        unit: String::new(),
        restart_required: false,
        description: dblocal_core::config::messages::READONLY_LABEL.to_owned(),
        readonly: true,
    });
    view
}

/// A successful apply result with a backup taken an hour ago.
fn sample_config_applied() -> dblocal_core::config::ConfigApplied {
    dblocal_core::config::ConfigApplied {
        restart_required: true,
        backup: dblocal_core::config::BackupInfo {
            taken_at: 1_700_000_000,
            taken_at_secs: 1_700_000_000,
            path: std::path::PathBuf::from("/mock/backups/postgresql.conf.1700000000.bak"),
        },
        changed: 2,
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
    mutations: Arc<Mutex<Mutations>>,
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
            mutations: Arc::new(Mutex::new(Mutations::default())),
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

    /// The arguments of every mutation so far, a snapshot.
    pub(crate) fn mutations(&self) -> Mutations {
        self.mutations.lock().expect("mutations lock").clone()
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
        self.bump(|counts| counts.start_instance += 1);
        let script = self.script.lock().expect("script lock");
        match &script.start_instance {
            Ok(()) => Ok(OperationId::new("op-start")),
            Err(err) => Err(dblocal_core::DblError::Engine {
                reason: format!("{} — {}", err.code, err.message),
            }),
        }
    }
    fn stop_instance(&self, _id: &InstanceId) -> Result<OperationId> {
        self.bump(|counts| counts.stop_instance += 1);
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
    fn create_database(&self, id: &InstanceId, name: &str) -> ServiceResult<()> {
        self.bump(|counts| counts.create_database += 1);
        self.mutations
            .lock()
            .expect("mutations lock")
            .creates
            .push((id.clone(), name.to_owned()));
        let script = self.script.lock().expect("script lock");
        script
            .create_database
            .as_ref()
            .map(|_| ())
            .map_err(clone_error)
    }
    fn drop_database(&self, id: &InstanceId, name: &str, force: bool) -> ServiceResult<()> {
        self.bump(|counts| counts.drop_database += 1);
        self.mutations.lock().expect("mutations lock").drops.push((
            id.clone(),
            name.to_owned(),
            force,
        ));
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
        id: &InstanceId,
        name: &str,
        db: &str,
        level: PrivilegeLevel,
    ) -> ServiceResult<SecretString> {
        self.bump(|counts| counts.create_user += 1);
        self.mutations.lock().expect("mutations lock").users.push((
            id.clone(),
            name.to_owned(),
            db.to_owned(),
            level,
        ));
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

    // --- Config editor (F-CFG) ----------------------------------------------
    fn config_view(&self, _id: &InstanceId) -> ServiceResult<dblocal_core::config::ConfigView> {
        self.bump(|counts| counts.config_view += 1);
        let script = self.script.lock().expect("script lock");
        match &script.config_view {
            Ok(view) => Ok(view.clone()),
            Err(err) => Err(clone_error(err)),
        }
    }
    fn config_apply(
        &self,
        id: &InstanceId,
        changes: Vec<(String, String)>,
    ) -> ServiceResult<dblocal_core::config::ConfigApplied> {
        self.bump(|counts| counts.config_apply += 1);
        self.mutations
            .lock()
            .expect("mutations lock")
            .config_applies
            .push((id.clone(), changes));
        let script = self.script.lock().expect("script lock");
        match &script.config_apply {
            Ok(applied) => Ok(applied.clone()),
            Err(err) => Err(clone_error(err)),
        }
    }
    fn config_restore(&self, id: &InstanceId) -> ServiceResult<()> {
        self.bump(|counts| counts.config_restore += 1);
        self.mutations
            .lock()
            .expect("mutations lock")
            .config_restores
            .push(id.clone());
        let script = self.script.lock().expect("script lock");
        script
            .config_restore
            .as_ref()
            .map(|_| ())
            .map_err(clone_error)
    }
    fn config_raw_path(&self, _id: &InstanceId) -> ServiceResult<std::path::PathBuf> {
        self.bump(|counts| counts.config_raw_path += 1);
        let script = self.script.lock().expect("script lock");
        match &script.config_raw_path {
            Ok(path) => Ok(path.clone()),
            Err(err) => Err(clone_error(err)),
        }
    }
    fn config_open_raw(&self, id: &InstanceId) -> ServiceResult<dblocal_core::config::OpenCommand> {
        self.bump(|counts| counts.config_open_raw += 1);
        self.mutations
            .lock()
            .expect("mutations lock")
            .config_opens
            .push(id.clone());
        let script = self.script.lock().expect("script lock");
        match &script.config_open_raw {
            Ok(command) => Ok(command.clone()),
            Err(err) => Err(clone_error(err)),
        }
    }
}

/// `UserFacingError` is `Clone`, so a scripted failure is returned unchanged.
fn clone_error(err: &UserFacingError) -> UserFacingError {
    err.clone()
}
