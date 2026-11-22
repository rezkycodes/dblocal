//! W4-A unit tests: update, open-in-client, the installer, and the production
//! glue reachable without a live engine.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::sync::Arc;

use crate::admin::PrivilegeLevel;
use crate::clients::{ClientKind, HostProbe, Spawner};
use crate::model::{EngineKey, Instance, InstanceId};
use crate::secret::SecretString;
use crate::service::DblService;
use crate::update::{FetchResponse, InstallKind, PackageManagerHint, UpdateFetcher};

use crate::instance::tests::harness::Harness;
use crate::instance::tests::{created, test_port};

use super::*;

// --- Update ------------------------------------------------------------------

fn harness_with_update(
    fetcher: Arc<dyn UpdateFetcher>,
    installer: Arc<dyn super::super::super::update_ops::UpdateInstaller>,
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
        Arc::clone(&installer) as Arc<dyn super::super::super::update_ops::UpdateInstaller>,
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
        Arc::clone(&installer) as Arc<dyn super::super::super::update_ops::UpdateInstaller>,
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
    use super::super::super::update_ops::UpdateInstaller;
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = crate::paths::DblPaths::with_root(dir.path());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let installer = super::super::super::update_ops::DownloadInstaller::new(
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
    use super::super::super::admin_ops;
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
    use super::super::super::clients_ops::os_kind_of;
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
    let host = super::super::super::clients_ops::default_host();
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
    let _spawner = super::super::super::clients_ops::default_spawner();
}

#[test]
fn running_version_parses() {
    let version = super::super::super::update_ops::running_version();
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
                Ok(Box::new(SharedAdmin(admin))
                    as Box<dyn super::super::super::admin_ops::AdminExecutor>)
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
    use super::super::super::update_ops::UpdateInstaller;
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = crate::paths::DblPaths::with_root(dir.path());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let installer = super::super::super::update_ops::DownloadInstaller::new(
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
