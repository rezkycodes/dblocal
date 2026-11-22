//! W4-A unit tests: the admin, update and open-in-client wiring on the real
//! `LocalService`, driven through the public `DblService` surface with injected
//! seams so no engine and no network are needed.
//!
//! The real PostgreSQL executor and a live keychain are proved by the ignored
//! `tests/admin_service_e2e.rs`; these tests prove the wiring above it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::sync::{Arc, Mutex};

use crate::admin::{
    AdminError, AdminResult, CredentialSink, DatabaseInfo, Identifier, PrivilegeLevel, UserInfo,
};
use crate::clients::{ClientKind, DetectedClient, HostProbe, LaunchPlan, Spawner};
use crate::model::{EngineKey, Instance, InstanceId};
use crate::secret::SecretString;
use crate::service::DblService;
use crate::service::SecretAccess;
use crate::update::{FetchResponse, InstallKind, PackageManagerHint, UpdateFetcher, UpdateResult};

use super::harness::Harness;
use super::{created, test_port};

// --- Fakes -------------------------------------------------------------------

/// A fake admin executor recording every call, with switchable failures.
#[derive(Default)]
struct FakeAdmin {
    calls: Mutex<Vec<String>>,
    fail_create: bool,
    fail_grant: bool,
    fail_reset: bool,
    password: Option<SecretString>,
}

impl FakeAdmin {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

impl super::super::admin_ops::AdminExecutor for FakeAdmin {
    fn list_databases(&self) -> AdminResult<Vec<DatabaseInfo>> {
        self.calls.lock().unwrap().push("list_databases".to_owned());
        Ok(vec![DatabaseInfo::new("postgres", "postgres", 1)])
    }
    fn list_users(&self) -> AdminResult<Vec<UserInfo>> {
        self.calls.lock().unwrap().push("list_users".to_owned());
        Ok(vec![UserInfo::new("postgres", true, true)])
    }
    fn create_database(&self, db: &Identifier) -> AdminResult<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("create_database:{}", db.as_str()));
        if self.fail_create {
            return Err(AdminError::DatabaseExists);
        }
        Ok(())
    }
    fn drop_database(&self, db: &Identifier, force: bool) -> AdminResult<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("drop_database:{}:{force}", db.as_str()));
        Ok(())
    }
    fn create_user(&self, user: &Identifier) -> AdminResult<SecretString> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("create_user:{}", user.as_str()));
        Ok(SecretString::new("user-once-pw"))
    }
    fn grant(&self, user: &Identifier, db: &Identifier, level: PrivilegeLevel) -> AdminResult<()> {
        self.calls.lock().unwrap().push(format!(
            "grant:{}:{}:{level:?}",
            user.as_str(),
            db.as_str()
        ));
        if self.fail_grant {
            return Err(AdminError::InsufficientPrivilege);
        }
        Ok(())
    }
    fn drop_user(&self, user: &Identifier) -> AdminResult<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("drop_user:{}", user.as_str()));
        Ok(())
    }
    fn reset_superuser_password(&self, sink: &dyn CredentialSink) -> AdminResult<SecretString> {
        self.calls.lock().unwrap().push("reset".to_owned());
        if self.fail_reset {
            return Err(AdminError::PasswordVerify);
        }
        let password = self
            .password
            .clone()
            .unwrap_or_else(|| SecretString::new("reset-once-pw"));
        // Mirrors the real state machine's final step: persist, and only then
        // report success.
        sink.store(&password)
            .map_err(|_| AdminError::CredentialStore)?;
        Ok(password)
    }
}

/// A harness whose admin calls hit `FakeAdmin`.
fn harness_with_admin(engine: EngineKey, admin: Arc<FakeAdmin>) -> (Harness, InstanceId) {
    let h = Harness::with_options(engine, move |options| {
        options.admin_factory = Some(Arc::new(move |_instance, _password| {
            let admin = Arc::clone(&admin);
            Ok(Box::new(SharedAdmin(admin)) as Box<dyn super::super::admin_ops::AdminExecutor>)
        }));
    });
    let name = format!("adm{}", test_port() % 1000);
    let instance = created(&h, &name);
    (h, instance.id().clone())
}

/// A boxed executor that borrows the shared fake.
struct SharedAdmin(Arc<FakeAdmin>);

impl super::super::admin_ops::AdminExecutor for SharedAdmin {
    fn list_databases(&self) -> AdminResult<Vec<DatabaseInfo>> {
        self.0.list_databases()
    }
    fn list_users(&self) -> AdminResult<Vec<UserInfo>> {
        self.0.list_users()
    }
    fn create_database(&self, db: &Identifier) -> AdminResult<()> {
        self.0.create_database(db)
    }
    fn drop_database(&self, db: &Identifier, force: bool) -> AdminResult<()> {
        self.0.drop_database(db, force)
    }
    fn create_user(&self, user: &Identifier) -> AdminResult<SecretString> {
        self.0.create_user(user)
    }
    fn grant(&self, user: &Identifier, db: &Identifier, level: PrivilegeLevel) -> AdminResult<()> {
        self.0.grant(user, db, level)
    }
    fn drop_user(&self, user: &Identifier) -> AdminResult<()> {
        self.0.drop_user(user)
    }
    fn reset_superuser_password(&self, sink: &dyn CredentialSink) -> AdminResult<SecretString> {
        self.0.reset_superuser_password(sink)
    }
}

/// A fetcher that returns one canned body.
struct FixedFetcher(std::result::Result<FetchResponse, String>);

impl UpdateFetcher for FixedFetcher {
    fn get(
        &self,
        _url: String,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::result::Result<FetchResponse, String>>
                + Send
                + '_,
        >,
    > {
        let response = self.0.clone();
        Box::pin(async move { response })
    }
}

fn release_body(tag: &str) -> Vec<u8> {
    serde_json::json!([{
        "tag_name": tag,
        "draft": false,
        "prerelease": false,
        "body": "notes",
        "assets": [
            {"name": "dblocal-linux-x86_64.tar.gz",
             "browser_download_url": "https://github.com/a/b/releases/download/v0.9.0/dblocal-linux-x86_64.tar.gz",
             "size": 100},
            {"name": "checksums.txt",
             "browser_download_url": "https://github.com/a/b/releases/download/v0.9.0/checksums.txt",
             "size": 80}
        ]
    }])
    .to_string()
    .into_bytes()
}

#[derive(Default)]
struct FakeInstaller {
    calls: Mutex<Vec<String>>,
}

impl super::super::update_ops::UpdateInstaller for FakeInstaller {
    fn install(
        &self,
        update: &crate::update::AvailableUpdate,
        _kind: &InstallKind,
    ) -> UpdateResult<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("install:{}", update.version()));
        Ok(())
    }
}

/// A host with one psql and one TablePlus, and no terminal.
struct FakeHost {
    clients: Vec<&'static str>,
}

impl HostProbe for FakeHost {
    fn which(&self, name: &str) -> Option<std::path::PathBuf> {
        self.clients
            .contains(&name)
            .then(|| std::path::PathBuf::from(format!("/usr/bin/{name}")))
    }
    fn path_exists(&self, _path: &std::path::Path) -> bool {
        false
    }
    fn desktop_entries(&self) -> Vec<String> {
        Vec::new()
    }
    fn macos_applications(&self) -> Vec<String> {
        Vec::new()
    }
    fn windows_app_paths(&self) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Default)]
struct FakeSpawner {
    plans: Mutex<Vec<LaunchPlan>>,
}

impl Spawner for FakeSpawner {
    fn spawn(&self, plan: &LaunchPlan) -> crate::clients::ClientResult<()> {
        self.plans.lock().unwrap().push(plan.clone());
        Ok(())
    }
}

// --- Admin -------------------------------------------------------------------

#[test]
fn list_databases_and_users_go_through_the_executor() {
    let admin = Arc::new(FakeAdmin::default());
    let (h, id) = harness_with_admin(EngineKey::Postgresql, Arc::clone(&admin));
    let dbs = h.service.list_databases(&id).expect("databases");
    assert_eq!(dbs.len(), 1);
    assert_eq!(dbs[0].name(), "postgres");
    let users = h.service.list_users(&id).expect("users");
    assert_eq!(users[0].name(), "postgres");
    assert_eq!(admin.calls(), vec!["list_databases", "list_users"]);
}

#[test]
fn create_database_rejects_a_bad_name_before_calling_the_engine() {
    let admin = Arc::new(FakeAdmin::default());
    let (h, id) = harness_with_admin(EngineKey::Postgresql, Arc::clone(&admin));
    let err = h
        .service
        .create_database(&id, "bad name")
        .expect_err("rejected");
    assert_eq!(err.code, crate::admin::error::CODE_INVALID_IDENTIFIER);
    assert!(
        admin.calls().is_empty(),
        "no engine call for an invalid name: {:?}",
        admin.calls()
    );
}

#[test]
fn create_database_announces_the_admin_change() {
    let admin = Arc::new(FakeAdmin::default());
    let (h, id) = harness_with_admin(EngineKey::Postgresql, Arc::clone(&admin));
    let mut rx = h.service.subscribe();
    h.service.create_database(&id, "app_db").expect("created");
    assert_eq!(admin.calls(), vec!["create_database:app_db"]);
    let event = rx.try_recv().expect("an AdminChanged event");
    assert!(matches!(
        event,
        crate::service::ServiceEvent::AdminChanged { .. }
    ));
}

#[test]
fn drop_database_passes_force_through() {
    let admin = Arc::new(FakeAdmin::default());
    let (h, id) = harness_with_admin(EngineKey::Postgresql, Arc::clone(&admin));
    h.service
        .drop_database(&id, "app_db", true)
        .expect("dropped");
    assert_eq!(admin.calls(), vec!["drop_database:app_db:true"]);
}

#[test]
fn create_user_grants_and_returns_the_password_once() {
    let admin = Arc::new(FakeAdmin::default());
    let (h, id) = harness_with_admin(EngineKey::Postgresql, Arc::clone(&admin));
    let password = h
        .service
        .create_user(&id, "report_user", "app_dev", PrivilegeLevel::ReadOnly)
        .expect("created");
    assert_eq!(password.expose(), "user-once-pw");
    assert_eq!(
        admin.calls(),
        vec![
            "create_user:report_user",
            "grant:report_user:app_dev:ReadOnly"
        ]
    );
}

#[test]
fn a_failed_grant_drops_the_half_created_role() {
    let admin = Arc::new(FakeAdmin {
        fail_grant: true,
        ..FakeAdmin::default()
    });
    let (h, id) = harness_with_admin(EngineKey::Postgresql, Arc::clone(&admin));
    let err = h
        .service
        .create_user(&id, "ghost", "app_dev", PrivilegeLevel::ReadWrite)
        .expect_err("grant fails");
    assert_eq!(err.code, crate::admin::error::CODE_INSUFFICIENT_PRIVILEGE);
    assert_eq!(
        admin.calls(),
        vec![
            "create_user:ghost",
            "grant:ghost:app_dev:ReadWrite",
            "drop_user:ghost"
        ],
        "the half-created role must be dropped"
    );
}

#[test]
fn a_failed_grant_announces_nothing() {
    let admin = Arc::new(FakeAdmin {
        fail_grant: true,
        ..FakeAdmin::default()
    });
    let (h, id) = harness_with_admin(EngineKey::Postgresql, Arc::clone(&admin));
    let mut rx = h.service.subscribe();
    let _ = h
        .service
        .create_user(&id, "ghost", "app_dev", PrivilegeLevel::ReadOnly);
    assert!(
        rx.try_recv().is_err(),
        "a failed create must not claim the admin view changed"
    );
}

#[test]
fn reset_success_persists_the_new_pointer_and_announces() {
    let admin = Arc::new(FakeAdmin::default());
    let (h, id) = harness_with_admin(EngineKey::Postgresql, Arc::clone(&admin));
    let mut rx = h.service.subscribe();
    let password = h.service.reset_superuser_password(&id).expect("reset");
    assert_eq!(password.expose(), "reset-once-pw");
    // The pointer was written after the successful reset.
    let stored = h
        .store()
        .get_credential(&id)
        .expect("credential read")
        .expect("credential pointer");
    assert_eq!(stored.backend(), crate::model::CredentialBackend::File);
    let event = rx.try_recv().expect("an AdminChanged event");
    assert!(matches!(
        event,
        crate::service::ServiceEvent::AdminChanged { .. }
    ));
}

#[test]
fn a_failed_reset_leaves_the_credential_untouched() {
    let admin = Arc::new(FakeAdmin {
        fail_reset: true,
        ..FakeAdmin::default()
    });
    let (h, id) = harness_with_admin(EngineKey::Postgresql, Arc::clone(&admin));
    let before = h
        .store()
        .get_credential(&id)
        .expect("read")
        .expect("pointer");
    let err = h
        .service
        .reset_superuser_password(&id)
        .expect_err("reset fails");
    assert_eq!(err.code, crate::admin::error::CODE_PASSWORD_VERIFY);
    let after = h
        .store()
        .get_credential(&id)
        .expect("read")
        .expect("pointer");
    assert_eq!(before, after, "the pointer must not move on a failed reset");
}

/// Seeds a row directly through the store, so an engine test does not have to
/// download its runtime. Returns the harness and the new id.
fn seed_row(h: &Harness, engine: EngineKey, name: &str) -> InstanceId {
    let spec = h.spec(name, engine, test_port());
    let id = InstanceId::new();
    let instance = Instance::new(
        &spec,
        id.clone(),
        crate::model::InstanceStatus::Stopped,
        std::time::SystemTime::now(),
    );
    let reference = crate::secrets::credential_ref_for(
        &id,
        crate::model::CredentialBackend::File,
        engine.default_superuser(),
    );
    h.secrets
        .set(&reference, &SecretString::new("seeded-pw"))
        .expect("seed credential");
    h.store()
        .insert_instance_with_credential(&instance, &reference)
        .expect("seed row");
    id
}

#[test]
fn mysql_and_valkey_report_not_supported_per_engine() {
    for engine in [EngineKey::Mysql, EngineKey::Mariadb, EngineKey::Valkey] {
        let admin = Arc::new(FakeAdmin::default());
        let h = Harness::with_options(EngineKey::Postgresql, {
            let admin = Arc::clone(&admin);
            move |options| {
                options.admin_factory = Some(Arc::new(move |_instance, _password| {
                    let admin = Arc::clone(&admin);
                    Ok(Box::new(SharedAdmin(admin))
                        as Box<dyn super::super::admin_ops::AdminExecutor>)
                }));
            }
        });
        let name = format!("eng{}", test_port() % 1000);
        let id = seed_row(&h, engine, &name);
        let err = h.service.list_databases(&id).expect_err("unsupported");
        assert_eq!(
            err.code,
            crate::admin::error::CODE_UNSUPPORTED,
            "{} must be typed unsupported, not an empty list",
            engine.display_name()
        );
        let err = h
            .service
            .create_database(&id, "x_db")
            .expect_err("unsupported");
        assert_eq!(err.code, crate::admin::error::CODE_UNSUPPORTED);
        let err = h.service.list_users(&id).expect_err("unsupported");
        assert_eq!(err.code, crate::admin::error::CODE_UNSUPPORTED);
        assert!(
            admin.calls().is_empty(),
            "{} must not reach the executor",
            engine.display_name()
        );
    }
}

#[test]
fn an_unknown_instance_is_a_typed_not_found() {
    let admin = Arc::new(FakeAdmin::default());
    let (h, _id) = harness_with_admin(EngineKey::Postgresql, admin);
    let missing = InstanceId::new();
    let err = h.service.list_databases(&missing).expect_err("missing");
    assert_eq!(err.code, crate::error::CODE_NOT_FOUND);
}

#[test]
fn the_admin_event_carries_no_secret() {
    let admin = Arc::new(FakeAdmin::default());
    let (h, id) = harness_with_admin(EngineKey::Postgresql, Arc::clone(&admin));
    let mut rx = h.service.subscribe();
    let password = h.service.reset_superuser_password(&id).expect("reset");
    let canary = password.expose().to_owned();
    while let Ok(event) = rx.try_recv() {
        let rendered = format!("{event:?}");
        assert!(
            !rendered.contains(&canary),
            "an event leaked the password: {rendered}"
        );
    }
}

#[test]
fn an_instance_without_a_credential_is_typed_not_found() {
    // A PostgreSQL instance with an unreadable credential must not panic and
    // must not call the executor.
    let admin = Arc::new(FakeAdmin::default());
    let h = Harness::with_options(EngineKey::Postgresql, {
        let admin = Arc::clone(&admin);
        move |options| {
            options.admin_factory = Some(Arc::new(move |_instance, _password| {
                let admin = Arc::clone(&admin);
                Ok(Box::new(SharedAdmin(admin)) as Box<dyn super::super::admin_ops::AdminExecutor>)
            }));
        }
    });
    let name = format!("nocred{}", test_port() % 1000);
    let instance = created(&h, &name);
    // Drop the pointer the admin path reads.
    h.secrets
        .unreadable
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let err = h
        .service
        .list_databases(instance.id())
        .expect_err("unreadable credential");
    // An unreadable backend surfaces its typed `Unsupported`, never a panic.
    assert_eq!(err.code, crate::error::CODE_UNSUPPORTED);
    assert!(admin.calls().is_empty());
}

// --- Update ------------------------------------------------------------------

fn harness_with_update(
    fetcher: Arc<dyn UpdateFetcher>,
    installer: Arc<dyn super::super::update_ops::UpdateInstaller>,
    kind: Option<InstallKind>,
    current: semver::Version,
) -> Harness {
    Harness::with_options(EngineKey::Postgresql, move |options| {
        options.update_fetcher = Some(fetcher);
        options.update_installer = Some(installer);
        options.install_kind = kind.clone();
        options.current_version = Some(current);
        options.update_repo = Some("a/b".to_owned());
    })
}

#[test]
fn check_update_reports_a_newer_release() {
    let fetcher = Arc::new(FixedFetcher(Ok(FetchResponse::new(
        200,
        release_body("v0.9.0"),
    ))));
    let installer = Arc::new(FakeInstaller::default());
    let h = harness_with_update(
        fetcher,
        installer,
        Some(InstallKind::PortableTarball {
            exe_path: "/opt/dblocal".into(),
        }),
        semver::Version::parse("0.1.0").unwrap(),
    );
    let mut rx = h.service.subscribe();
    let status = h.service.check_update().expect("check");
    let available = status.available.expect("an update is available");
    assert_eq!(available.version, "0.9.0");
    assert!(available.can_self_update);
    assert!(status.package_manager_hint.is_none());
    let event = rx.try_recv().expect("an UpdateChecked event");
    match event {
        crate::service::ServiceEvent::UpdateChecked {
            version,
            can_self_update,
        } => {
            assert_eq!(version.as_deref(), Some("0.9.0"));
            assert!(can_self_update);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn check_update_reports_up_to_date() {
    let fetcher = Arc::new(FixedFetcher(Ok(FetchResponse::new(
        200,
        release_body("v0.9.0"),
    ))));
    let installer = Arc::new(FakeInstaller::default());
    let h = harness_with_update(
        fetcher,
        installer,
        Some(InstallKind::PortableTarball {
            exe_path: "/opt/dblocal".into(),
        }),
        semver::Version::parse("0.9.0").unwrap(),
    );
    let status = h.service.check_update().expect("check");
    assert!(status.available.is_none());
}

#[test]
fn check_update_keeps_the_network_code_on_failure() {
    let fetcher = Arc::new(FixedFetcher(Err("connection refused".to_owned())));
    let installer = Arc::new(FakeInstaller::default());
    let h = harness_with_update(
        fetcher,
        installer,
        Some(InstallKind::PortableTarball {
            exe_path: "/opt/dblocal".into(),
        }),
        semver::Version::parse("0.1.0").unwrap(),
    );
    let err = h.service.check_update().expect_err("network fails");
    assert_eq!(err.code, crate::update::CODE_UPDATE_NETWORK);
}

#[test]
fn check_update_carries_the_package_manager_hint() {
    let fetcher = Arc::new(FixedFetcher(Ok(FetchResponse::new(
        200,
        release_body("v0.9.0"),
    ))));
    let installer = Arc::new(FakeInstaller::default());
    let h = harness_with_update(
        fetcher,
        installer,
        Some(InstallKind::DistPackage {
            exe_path: "/usr/bin/dblocal".into(),
            hint: PackageManagerHint::Rpm,
        }),
        semver::Version::parse("0.1.0").unwrap(),
    );
    let status = h.service.check_update().expect("check");
    let hint = status.package_manager_hint.expect("a hint");
    assert!(hint.contains("rpm"), "hint: {hint}");
    assert!(!status.install_kind.can_self_update());
}

#[test]
fn apply_update_refuses_a_package_manager_install_before_downloading() {
    let fetcher = Arc::new(FixedFetcher(Ok(FetchResponse::new(
        200,
        release_body("v0.9.0"),
    ))));
    let installer = Arc::new(FakeInstaller::default());
    let h = harness_with_update(
        Arc::clone(&fetcher) as Arc<dyn UpdateFetcher>,
        Arc::clone(&installer) as Arc<dyn super::super::update_ops::UpdateInstaller>,
        Some(InstallKind::Homebrew {
            exe_path: "/opt/homebrew/bin/dblocal".into(),
        }),
        semver::Version::parse("0.1.0").unwrap(),
    );
    let err = h.service.apply_update().expect_err("refused");
    assert_eq!(err.code, crate::update::CODE_UPDATE_UNSUPPORTED_KIND);
    assert!(
        installer.calls.lock().unwrap().is_empty(),
        "the installer must never run for a managed install"
    );
}

#[test]
fn apply_update_runs_the_installer_when_self_update_is_allowed() {
    let fetcher = Arc::new(FixedFetcher(Ok(FetchResponse::new(
        200,
        release_body("v0.9.0"),
    ))));
    let installer = Arc::new(FakeInstaller::default());
    let h = harness_with_update(
        fetcher,
        Arc::clone(&installer) as Arc<dyn super::super::update_ops::UpdateInstaller>,
        Some(InstallKind::PortableTarball {
            exe_path: "/opt/dblocal".into(),
        }),
        semver::Version::parse("0.1.0").unwrap(),
    );
    h.service.apply_update().expect("applied");
    assert_eq!(
        installer.calls.lock().unwrap().as_slice(),
        &["install:0.9.0".to_owned()]
    );
}

// --- Clients -----------------------------------------------------------------

fn harness_with_clients(
    host: Arc<dyn HostProbe>,
    spawner: Arc<dyn Spawner>,
) -> (Harness, InstanceId) {
    let h = Harness::with_options(EngineKey::Postgresql, move |options| {
        options.host_probe = Some(host);
        options.spawner = Some(spawner);
    });
    let name = format!("opn{}", test_port() % 1000);
    let instance = created(&h, &name);
    (h, instance.id().clone())
}

#[test]
fn detect_clients_reports_only_the_installed_ones() {
    let host = Arc::new(FakeHost {
        clients: vec!["psql", "tableplus"],
    });
    let spawner = Arc::new(FakeSpawner::default());
    let (h, _id) = harness_with_clients(host, spawner);
    let clients = h.service.detect_clients(false).expect("detected");
    let kinds: Vec<ClientKind> = clients.iter().map(|c| c.kind()).collect();
    assert!(kinds.contains(&ClientKind::Psql));
    assert!(kinds.contains(&ClientKind::TablePlus));
    assert!(
        !kinds.contains(&ClientKind::DBeaver),
        "dbeaver is not installed"
    );
    let listed: Vec<DetectedClient> = clients;
    assert!(listed.iter().all(|c| c.path().starts_with("/usr/bin")));
}

#[test]
fn open_in_client_dry_run_returns_a_plan_without_launching() {
    let host = Arc::new(FakeHost {
        clients: vec!["tableplus"],
    });
    let spawner = Arc::new(FakeSpawner::default());
    let (h, id) = harness_with_clients(host, Arc::clone(&spawner) as Arc<dyn Spawner>);
    let plan = h
        .service
        .open_in_client(&id, ClientKind::TablePlus, true)
        .expect("plan");
    assert!(
        spawner.plans.lock().unwrap().is_empty(),
        "a dry run must not launch"
    );
    assert_eq!(
        plan.required_path().map(|p| p.display().to_string()),
        Some("/usr/bin/tableplus".to_owned())
    );
}

#[test]
fn open_in_client_launches_through_the_spawner() {
    let host = Arc::new(FakeHost {
        clients: vec!["tableplus"],
    });
    let spawner = Arc::new(FakeSpawner::default());
    let (h, id) = harness_with_clients(host, Arc::clone(&spawner) as Arc<dyn Spawner>);
    h.service
        .open_in_client(&id, ClientKind::TablePlus, false)
        .expect("launched");
    assert_eq!(spawner.plans.lock().unwrap().len(), 1);
}

#[test]
fn opening_a_missing_client_is_a_typed_not_found() {
    let host = Arc::new(FakeHost {
        clients: vec!["psql"],
    });
    let spawner = Arc::new(FakeSpawner::default());
    let (h, id) = harness_with_clients(host, spawner);
    let err = h
        .service
        .open_in_client(&id, ClientKind::TablePlus, false)
        .expect_err("not installed");
    assert_eq!(
        err.code,
        crate::clients::ClientError::ClientNotFound {
            name: String::new()
        }
        .code()
    );
}

#[test]
fn the_launch_plan_never_carries_the_password() {
    // A CLI client needs a terminal; `xterm` is in the resolution order.
    let host = Arc::new(FakeHost {
        clients: vec!["psql", "xterm"],
    });
    let spawner = Arc::new(FakeSpawner::default());
    let (h, id) = harness_with_clients(host, spawner);
    // The password a CLI plan would export must not appear in argv/url/command.
    let password = h
        .service
        .connection_uri(&id, crate::service::UriFormat::Uri)
        .expect("uri")
        .expose()
        .to_owned();
    let pinned = password
        .split('@')
        .next()
        .and_then(|userinfo| userinfo.split_once(':'))
        .map(|(_, secret)| secret.to_owned())
        .unwrap_or_default();
    assert!(!pinned.is_empty(), "the seeded instance has a password");
    let plan = h
        .service
        .open_in_client(&id, ClientKind::Psql, true)
        .expect("plan");
    assert!(
        !plan.argv().iter().any(|arg| arg.contains(&pinned)),
        "argv leaked the password: {:?}",
        plan.argv()
    );
    assert!(
        plan.url().is_none_or(|url| !url.contains(&pinned)),
        "url leaked the password"
    );
    assert!(
        plan.command_string()
            .is_none_or(|cmd| !cmd.contains(&pinned)),
        "command string leaked the password"
    );
}

// --- DownloadInstaller preflight and the trait defaults ----------------------

#[test]
fn download_installer_preflight_refuses_a_managed_install_before_downloading() {
    use super::super::update_ops::UpdateInstaller;
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = crate::paths::DblPaths::with_root(dir.path());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let installer = super::super::update_ops::DownloadInstaller::new(
        paths,
        runtime.handle().clone(),
        std::path::PathBuf::from("/opt/dblocal"),
    );
    let asset = crate::update::ReleaseAsset::new(
        "dblocal-linux-x86_64.tar.gz",
        "https://github.com/a/b/releases/download/v0.9.0/dblocal-linux-x86_64.tar.gz",
        100,
        crate::update::UpdateAssetKind::TarGz,
    );
    let checksums = crate::update::ReleaseAsset::new(
        "checksums.txt",
        "https://github.com/a/b/releases/download/v0.9.0/checksums.txt",
        80,
        crate::update::UpdateAssetKind::Checksums,
    );
    let update = crate::update::AvailableUpdate::new(
        semver::Version::parse("0.9.0").expect("version"),
        "v0.9.0",
        "notes",
        asset,
        checksums,
    );
    let kind = InstallKind::DistPackage {
        exe_path: "/usr/bin/dblocal".into(),
        hint: PackageManagerHint::Rpm,
    };
    let err = installer.install(&update, &kind).expect_err("refused");
    assert_eq!(err.code(), crate::update::CODE_UPDATE_UNSUPPORTED_KIND);
    // Nothing was written to the staging directory.
    assert!(
        !dir.path().join("update").exists(),
        "a refused install stages nothing"
    );
}

#[test]
fn the_default_trait_methods_return_a_typed_not_implemented() {
    // A minimal implementor that relies on every provided default.
    struct Bare;
    impl DblService for Bare {
        fn list_instances(&self) -> crate::error::Result<Vec<crate::service::InstanceView>> {
            Ok(Vec::new())
        }
        fn catalog(&self) -> crate::error::Result<crate::service::CatalogView> {
            Err(crate::DblError::NotImplemented("catalog".into()))
        }
        fn refresh_catalog(
            &self,
            _force: bool,
        ) -> crate::error::Result<crate::service::OperationId> {
            Err(crate::DblError::NotImplemented("catalog".into()))
        }
        fn validate_spec(&self, _spec: &crate::model::InstanceSpec) -> crate::error::Result<()> {
            Ok(())
        }
        fn suggest_port(&self, _e: EngineKey) -> crate::error::Result<crate::model::Port> {
            Err(crate::DblError::NotImplemented("port".into()))
        }
        fn create_instance(
            &self,
            _spec: crate::model::InstanceSpec,
        ) -> crate::error::Result<crate::service::OperationId> {
            Err(crate::DblError::NotImplemented("create".into()))
        }
        fn start_instance(
            &self,
            _id: &InstanceId,
        ) -> crate::error::Result<crate::service::OperationId> {
            Err(crate::DblError::NotImplemented("start".into()))
        }
        fn stop_instance(
            &self,
            _id: &InstanceId,
        ) -> crate::error::Result<crate::service::OperationId> {
            Err(crate::DblError::NotImplemented("stop".into()))
        }
        fn delete_instance(
            &self,
            _id: &InstanceId,
            _mode: crate::model::DeleteMode,
        ) -> crate::error::Result<crate::service::OperationId> {
            Err(crate::DblError::NotImplemented("delete".into()))
        }
        fn change_port(
            &self,
            _id: &InstanceId,
            _port: crate::model::Port,
        ) -> crate::error::Result<()> {
            Err(crate::DblError::NotImplemented("port".into()))
        }
        fn data_size(&self, _id: &InstanceId) -> crate::error::Result<u64> {
            Err(crate::DblError::NotImplemented("size".into()))
        }
        fn set_autostart(&self, _id: &InstanceId, _enabled: bool) -> crate::error::Result<()> {
            Err(crate::DblError::NotImplemented("autostart".into()))
        }
        fn cancel_operation(&self, _id: &crate::service::OperationId) -> crate::error::Result<()> {
            Err(crate::DblError::NotImplemented("cancel".into()))
        }
        fn connection_uri(
            &self,
            _id: &InstanceId,
            _format: crate::service::UriFormat,
        ) -> crate::error::Result<SecretString> {
            Err(crate::DblError::NotImplemented("uri".into()))
        }
        fn tail_logs(&self, _id: &InstanceId, _lines: usize) -> crate::error::Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn watch_logs(&self, _id: &InstanceId, _enabled: bool) -> crate::error::Result<()> {
            Ok(())
        }
        fn list_runtimes(&self) -> crate::error::Result<Vec<crate::model::Runtime>> {
            Ok(Vec::new())
        }
        fn remove_runtime(&self, _runtime: &str) -> crate::error::Result<()> {
            Ok(())
        }
        fn list_archives(&self) -> crate::error::Result<Vec<crate::model::ArchivedData>> {
            Ok(Vec::new())
        }
        fn purge_archive(&self, _archive: &str) -> crate::error::Result<()> {
            Ok(())
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<crate::service::ServiceEvent> {
            let (tx, rx) = tokio::sync::broadcast::channel(1);
            drop(tx);
            rx
        }
    }
    let bare = Bare;
    let id = InstanceId::new();
    for code in [
        bare.list_databases(&id).expect_err("default").code,
        bare.create_database(&id, "x").expect_err("default").code,
        bare.drop_database(&id, "x", false)
            .expect_err("default")
            .code,
        bare.list_users(&id).expect_err("default").code,
        bare.create_user(&id, "u", "d", PrivilegeLevel::ReadOnly)
            .expect_err("default")
            .code,
        bare.reset_superuser_password(&id)
            .expect_err("default")
            .code,
        bare.check_update().expect_err("default").code,
        bare.apply_update().expect_err("default").code,
        bare.detect_clients(false).expect_err("default").code,
    ] {
        assert_eq!(code, crate::error::CODE_NOT_IMPLEMENTED);
    }
    let err = bare
        .open_in_client(&id, ClientKind::Psql, true)
        .expect_err("default");
    assert_eq!(err.code, crate::error::CODE_NOT_IMPLEMENTED);
}

// --- Production glue reachable without a live engine ------------------------

#[test]
fn the_pg_admin_factory_builds_a_loopback_executor() {
    use super::super::admin_ops;
    let factory = admin_ops::pg_admin_factory();
    let instance = Instance::new(
        &crate::model::InstanceSpec::new(
            crate::model::InstanceName::new("svc").expect("name"),
            EngineKey::Postgresql,
            "18.7",
            Some(crate::model::Port::new(5432).expect("port")),
            crate::model::AuthMode::Password,
        )
        .expect("spec"),
        InstanceId::new(),
        crate::model::InstanceStatus::Running,
        std::time::SystemTime::now(),
    );
    let executor = factory(&instance, SecretString::new("pw")).expect("factory");
    // The plan target is a loopback admin client; drop it so nothing lingers.
    drop(executor);
}

#[test]
fn os_kind_of_maps_every_platform() {
    use super::super::clients_ops::os_kind_of;
    assert_eq!(
        os_kind_of(crate::platform::Os::Linux),
        crate::clients::OsKind::Linux
    );
    assert_eq!(
        os_kind_of(crate::platform::Os::Macos),
        crate::clients::OsKind::Macos
    );
    assert_eq!(
        os_kind_of(crate::platform::Os::Windows),
        crate::clients::OsKind::Windows
    );
}

#[test]
fn the_system_host_resolves_a_real_binary_on_path() {
    let host = super::super::clients_ops::default_host();
    // `sh` is on PATH on every Unix; the probe must find it and its path exists.
    if let Some(path) = host.which("sh") {
        assert!(host.path_exists(&path));
    }
    // A name that cannot exist resolves to nothing.
    assert!(host.which("dblocal-definitely-not-installed-xyz").is_none());
    assert!(host.desktop_entries().is_empty());
}

#[test]
fn the_default_spawner_is_constructible() {
    let _spawner = super::super::clients_ops::default_spawner();
}

#[test]
fn running_version_parses() {
    let version = super::super::update_ops::running_version();
    assert!(!version.to_string().is_empty());
}

#[test]
fn a_no_password_instance_has_no_launch_credential() {
    // The engine gate for admin already rejects AuthMode::None; this proves the
    // client path likewise returns no credential for a no-password instance.
    let admin = Arc::new(FakeAdmin::default());
    let h = Harness::with_options(EngineKey::Postgresql, {
        let admin = Arc::clone(&admin);
        move |options| {
            options.admin_factory = Some(Arc::new(move |_instance, _password| {
                let admin = Arc::clone(&admin);
                Ok(Box::new(SharedAdmin(admin)) as Box<dyn super::super::admin_ops::AdminExecutor>)
            }));
        }
    });
    // A password instance for which the credential row is deleted: the admin
    // path reports a typed NotFound, never a panic.
    let name = format!("nocred2{}", test_port() % 1000);
    let instance = created(&h, &name);
    h.store()
        .connection()
        .execute(
            "DELETE FROM credentials WHERE instance_id = ?1",
            [instance.id().as_str()],
        )
        .expect("delete credential row");
    let err = h
        .service
        .list_databases(instance.id())
        .expect_err("missing credential");
    assert_eq!(err.code, crate::error::CODE_NOT_FOUND);
    assert!(admin.calls().is_empty());
}

#[test]
fn download_installer_rejects_a_disallowed_asset_host_before_any_request() {
    use super::super::update_ops::UpdateInstaller;
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = crate::paths::DblPaths::with_root(dir.path());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let installer = super::super::update_ops::DownloadInstaller::new(
        paths,
        runtime.handle().clone(),
        std::path::PathBuf::from("/opt/dblocal"),
    );
    // A self-update kind passes preflight, then the download rejects the host
    // synchronously (no network is touched), proving the tail is wired.
    let asset = crate::update::ReleaseAsset::new(
        "dblocal-linux-x86_64.tar.gz",
        "https://evil.example.com/dblocal.tar.gz",
        100,
        crate::update::UpdateAssetKind::TarGz,
    );
    let checksums = crate::update::ReleaseAsset::new(
        "checksums.txt",
        "https://evil.example.com/checksums.txt",
        80,
        crate::update::UpdateAssetKind::Checksums,
    );
    let update = crate::update::AvailableUpdate::new(
        semver::Version::parse("0.9.0").expect("version"),
        "v0.9.0",
        "notes",
        asset,
        checksums,
    );
    let kind = InstallKind::PortableTarball {
        exe_path: "/opt/dblocal".into(),
    };
    let err = installer
        .install(&update, &kind)
        .expect_err("disallowed host");
    // A disallowed host is a network-policy failure, not an apply success.
    assert_eq!(err.code(), crate::update::CODE_UPDATE_NETWORK);
}

#[test]
fn check_update_detects_the_install_kind_when_none_is_set() {
    // With no injected kind, `check_update` runs `detect_install_kind` on this
    // test binary, which is a portable build, so the result carries a kind and
    // never panics.
    let fetcher = Arc::new(FixedFetcher(Ok(FetchResponse::new(
        200,
        release_body("v0.9.0"),
    ))));
    let installer = Arc::new(FakeInstaller::default());
    let h = harness_with_update(
        fetcher,
        installer,
        None,
        semver::Version::parse("0.1.0").unwrap(),
    );
    let status = h.service.check_update().expect("check");
    // The kind is whatever this machine's build is; the call must not error.
    let _ = status.install_kind;
}
