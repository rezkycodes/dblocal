//! W4-A unit tests: the admin, update and open-in-client wiring on the real
//! `LocalService`, driven through the public `DblService` surface with injected
//! seams so no engine and no network are needed.
//!
//! Split into two files so neither exceeds the 800-line limit: this one owns the
//! shared fakes and the admin wiring; [`super::service_update_clients`] owns the
//! update, client, installer and production-glue tests. The real PostgreSQL
//! executor is proved by the ignored `tests/service_admin_e2e.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::sync::{Arc, Mutex};

use crate::admin::{
    AdminError, AdminResult, CredentialSink, DatabaseInfo, Identifier, PrivilegeLevel, UserInfo,
};
use crate::clients::{DetectedClient, HostProbe, LaunchPlan, Spawner};
use crate::model::{EngineKey, Instance, InstanceId};
use crate::secret::SecretString;
use crate::service::DblService;
use crate::service::SecretAccess;
use crate::update::{FetchResponse, InstallKind, UpdateFetcher, UpdateResult};

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
    /// When set, `database_exists` reports the target database as missing, so a
    /// create-user is refused before any role is created (F2).
    database_missing: bool,
    password: Option<SecretString>,
}

impl FakeAdmin {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

impl crate::admin::AdminExecutor for FakeAdmin {
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
    fn database_exists(&self, db: &Identifier) -> AdminResult<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("database_exists:{}", db.as_str()));
        Ok(!self.database_missing)
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
            Ok(Box::new(SharedAdmin(admin)) as Box<dyn crate::admin::AdminExecutor>)
        }));
    });
    let name = format!("adm{}", test_port() % 1000);
    let instance = created(&h, &name);
    (h, instance.id().clone())
}

/// A boxed executor that borrows the shared fake.
struct SharedAdmin(Arc<FakeAdmin>);

impl crate::admin::AdminExecutor for SharedAdmin {
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
    fn database_exists(&self, db: &Identifier) -> AdminResult<bool> {
        self.0.database_exists(db)
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

mod update_clients;

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
            "database_exists:app_dev",
            "create_user:report_user",
            "grant:report_user:app_dev:ReadOnly"
        ]
    );
}

#[test]
fn create_user_reports_database_not_found_without_creating_a_role() {
    // F2: a missing target database must be DBL-8105, not a connect failure, and
    // no role may be created.
    let admin = Arc::new(FakeAdmin {
        database_missing: true,
        ..FakeAdmin::default()
    });
    let (h, id) = harness_with_admin(EngineKey::Postgresql, Arc::clone(&admin));
    let err = h
        .service
        .create_user(&id, "ghost", "nosuchdb", PrivilegeLevel::ReadOnly)
        .expect_err("unknown database");
    assert_eq!(err.code, crate::admin::error::CODE_DATABASE_NOT_FOUND);
    assert_eq!(
        admin.calls(),
        vec!["database_exists:nosuchdb"],
        "no role is created when the database is missing"
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
            "database_exists:app_dev",
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
fn reset_moves_the_credential_pointer_to_the_backend_that_stored_it() {
    // F1 (F-DBU-03 with a keychain that became unavailable): the pointer says
    // keychain, but the write falls back to the file, so after the reset the
    // pointer MUST say file. Otherwise the engine has the new password while
    // DBLocal reads the old backend — an instance DBLocal can never log in to
    // again.
    use crate::secrets::SecretBackend as _;
    let admin = Arc::new(FakeAdmin::default());
    let (h, id) = harness_with_admin(EngineKey::Postgresql, Arc::clone(&admin));

    // The instance was created while the keychain was available: the old
    // password lives in the keychain and the pointer names that backend.
    let keychain_ref = crate::secrets::credential_ref_for(
        &id,
        crate::model::CredentialBackend::Keychain,
        "postgres",
    );
    h.secrets
        .keychain
        .set(&keychain_ref, &SecretString::new("old-keychain-pw"))
        .expect("seed the keychain backend");
    h.store()
        .set_credential(&keychain_ref)
        .expect("seed the keychain pointer");
    assert_eq!(
        h.store()
            .get_credential(&id)
            .expect("read")
            .expect("pointer")
            .backend(),
        crate::model::CredentialBackend::Keychain,
        "precondition: the pointer names the keychain"
    );
    // The keychain is now unavailable, so the reset's write falls back to the
    // file backend and reports `File`.
    h.secrets
        .keychain_available
        .store(false, std::sync::atomic::Ordering::SeqCst);

    let new_password = h.service.reset_superuser_password(&id).expect("reset");

    let pointer = h
        .store()
        .get_credential(&id)
        .expect("read")
        .expect("pointer");
    assert_eq!(
        pointer.backend(),
        crate::model::CredentialBackend::File,
        "the pointer must follow the password into the file backend"
    );
    // The credential the pointer names now holds the new password, and the URI
    // built from it carries that password.
    let stored = h.secrets.get(&pointer).expect("stored");
    assert_eq!(stored.expose(), new_password.expose());
    let uri = h
        .service
        .connection_uri(&id, crate::service::UriFormat::Uri)
        .expect("uri");
    assert!(
        uri.expose().contains(new_password.expose()),
        "the URI must carry the new password from the file backend"
    );
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
fn valkey_reports_not_supported() {
    // Valkey has no named databases or login roles: a typed "not supported",
    // never an empty list, and the executor is never built.
    let admin = Arc::new(FakeAdmin::default());
    let h = Harness::with_options(EngineKey::Postgresql, {
        let admin = Arc::clone(&admin);
        move |options| {
            options.admin_factory = Some(Arc::new(move |_instance, _password| {
                let admin = Arc::clone(&admin);
                Ok(Box::new(SharedAdmin(admin)) as Box<dyn crate::admin::AdminExecutor>)
            }));
        }
    });
    let name = format!("vk{}", test_port() % 1000);
    let id = seed_row(&h, EngineKey::Valkey, &name);
    let err = h.service.list_databases(&id).expect_err("unsupported");
    assert_eq!(err.code, crate::admin::error::CODE_UNSUPPORTED);
    let err = h
        .service
        .create_database(&id, "x_db")
        .expect_err("unsupported");
    assert_eq!(err.code, crate::admin::error::CODE_UNSUPPORTED);
    let err = h.service.list_users(&id).expect_err("unsupported");
    assert_eq!(err.code, crate::admin::error::CODE_UNSUPPORTED);
    assert!(
        admin.calls().is_empty(),
        "Valkey must not reach the executor"
    );
}

#[test]
fn mysql_and_mariadb_route_to_the_executor() {
    // W4-A2: MySQL and MariaDB are supported engines now; the engine gate lets
    // them through to the injected executor (the real one dials the socket).
    for engine in [EngineKey::Mysql, EngineKey::Mariadb] {
        let admin = Arc::new(FakeAdmin::default());
        let h = Harness::with_options(EngineKey::Postgresql, {
            let admin = Arc::clone(&admin);
            move |options| {
                options.admin_factory = Some(Arc::new(move |_instance, _password| {
                    let admin = Arc::clone(&admin);
                    Ok(Box::new(SharedAdmin(admin)) as Box<dyn crate::admin::AdminExecutor>)
                }));
            }
        });
        let name = format!("my{}", test_port() % 1000);
        let id = seed_row(&h, engine, &name);
        let dbs = h.service.list_databases(&id).expect("routed to executor");
        assert_eq!(dbs.len(), 1, "{}", engine.display_name());
        h.service
            .create_database(&id, "x_db")
            .expect("routed to executor");
        h.service.list_users(&id).expect("routed to executor");
        assert_eq!(
            admin.calls(),
            vec![
                "list_databases".to_owned(),
                "create_database:x_db".to_owned(),
                "list_users".to_owned()
            ],
            "{} must reach the executor",
            engine.display_name()
        );
    }
}

#[test]
fn a_no_password_postgres_instance_reports_its_own_error() {
    // F3: a PostgreSQL instance created with --no-password is a supported
    // engine, so it must NOT say "engine not supported" (DBL-8111); it reports a
    // distinct, actionable code that says the instance has no password.
    let admin = Arc::new(FakeAdmin::default());
    let h = Harness::with_options(EngineKey::Postgresql, {
        let admin = Arc::clone(&admin);
        move |options| {
            options.admin_factory = Some(Arc::new(move |_instance, _password| {
                let admin = Arc::clone(&admin);
                Ok(Box::new(SharedAdmin(admin)) as Box<dyn crate::admin::AdminExecutor>)
            }));
        }
    });
    let name = format!("nopw{}", test_port() % 1000);
    // Seed a no-password row directly: the create path needs a runtime, and the
    // gate under test reads only the row's engine and auth.
    let spec = crate::model::InstanceSpec::new(
        crate::model::InstanceName::new(&name).expect("name"),
        EngineKey::Postgresql,
        "17.4",
        Some(crate::model::Port::new(test_port()).expect("port")),
        crate::model::AuthMode::None,
    )
    .expect("spec");
    let id = InstanceId::new();
    let instance = Instance::new(
        &spec,
        id.clone(),
        crate::model::InstanceStatus::Stopped,
        std::time::SystemTime::now(),
    );
    h.store().insert_instance(&instance).expect("seed row");

    let err = h.service.list_databases(&id).expect_err("no-password");
    assert_eq!(
        err.code,
        crate::admin::error::CODE_NO_PASSWORD,
        "the message must name the missing password, not the engine"
    );
    assert_ne!(
        err.code,
        crate::admin::error::CODE_UNSUPPORTED,
        "a no-password PostgreSQL instance is not an unsupported engine"
    );
    let err = h
        .service
        .reset_superuser_password(&id)
        .expect_err("no-password");
    assert_eq!(err.code, crate::admin::error::CODE_NO_PASSWORD);
    assert!(
        admin.calls().is_empty(),
        "the engine gate must reject before the executor is built"
    );
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
                Ok(Box::new(SharedAdmin(admin)) as Box<dyn crate::admin::AdminExecutor>)
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
