//! W4-A/W4-B admin, update and open-in-client scenarios on the mock service.
//!
//! These are the cases the CLI and the GUI both drive (databases, users,
//! password reset, update check/apply, client detection and launch). Split from
//! `tests.rs` to keep both files under the 800-line cap; included as a submodule
//! so the shared `spec`/`seeded_pg` helpers and `super::*` apply.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::super::MockService;
use super::{no_port_spec, spec};
use crate::model::{EngineKey, InstanceStatus};
use crate::service::{DblService, ServiceEvent, UriFormat};

// --- W4-A: admin, update and open-in-client on the mock ----------------------

use crate::admin::PrivilegeLevel;
use crate::clients::ClientKind;
use crate::update::{InstallKind, PackageManagerHint};

fn seeded_pg() -> (MockService, crate::model::InstanceId) {
    let service = MockService::deterministic();
    let id = service.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    (service, id)
}

#[test]
fn mock_lists_postgres_databases_with_a_size() {
    let (service, id) = seeded_pg();
    let databases = service.list_databases(&id).expect("databases");
    assert!(databases.iter().any(|db| db.name() == "postgres"));
    assert!(databases.iter().any(|db| db.name() == "app_development"));
    assert!(databases.iter().all(|db| db.size_bytes() > 0));
}

#[test]
fn mock_create_and_drop_database_round_trip_and_announce() {
    let (service, id) = seeded_pg();
    service.create_database(&id, "shop_db").expect("create");
    assert!(
        service
            .list_databases(&id)
            .expect("list")
            .iter()
            .any(|db| db.name() == "shop_db")
    );
    // A second create of the same name is a typed conflict.
    let err = service
        .create_database(&id, "shop_db")
        .expect_err("duplicate");
    assert_eq!(err.code, crate::admin::error::CODE_DATABASE_EXISTS);
    service.drop_database(&id, "shop_db", false).expect("drop");
    assert!(
        !service
            .list_databases(&id)
            .expect("list")
            .iter()
            .any(|db| db.name() == "shop_db")
    );
    let err = service
        .drop_database(&id, "shop_db", false)
        .expect_err("gone");
    assert_eq!(err.code, crate::admin::error::CODE_DATABASE_NOT_FOUND);
}

#[test]
fn mock_create_user_returns_a_password_and_lists_the_role() {
    let (service, id) = seeded_pg();
    let password = service
        .create_user(
            &id,
            "report_user",
            "app_development",
            PrivilegeLevel::ReadOnly,
        )
        .expect("create user");
    assert!(password.expose().starts_with("pwd-user-"));
    assert!(
        service
            .list_users(&id)
            .expect("users")
            .iter()
            .any(|user| user.name() == "report_user")
    );
    let err = service
        .create_user(
            &id,
            "report_user",
            "app_development",
            PrivilegeLevel::ReadOnly,
        )
        .expect_err("duplicate");
    assert_eq!(err.code, crate::admin::error::CODE_USER_EXISTS);
}

#[test]
fn mock_reset_rotates_the_uri_password() {
    let (service, id) = seeded_pg();
    let before = service
        .connection_uri(&id, UriFormat::Uri)
        .expect("uri before")
        .expose()
        .to_owned();
    let password = service.reset_superuser_password(&id).expect("reset");
    let after = service
        .connection_uri(&id, UriFormat::Uri)
        .expect("uri after")
        .expose()
        .to_owned();
    assert_ne!(before, after, "the URI must carry the new password");
    assert!(after.contains(password.expose()));
}

#[test]
fn mock_reports_unsupported_for_valkey() {
    let service = MockService::deterministic();
    let id = service.seed_instance(
        spec("cache", EngineKey::Valkey, "8.1.0", 6379),
        InstanceStatus::Running,
    );
    let err = service.list_databases(&id).expect_err("unsupported");
    assert_eq!(err.code, crate::admin::error::CODE_UNSUPPORTED);
    let err = service.list_users(&id).expect_err("unsupported");
    assert_eq!(err.code, crate::admin::error::CODE_UNSUPPORTED);
    let err = service
        .reset_superuser_password(&id)
        .expect_err("unsupported");
    assert_eq!(err.code, crate::admin::error::CODE_UNSUPPORTED);
}

#[test]
fn mock_supports_mysql_and_mariadb_admin() {
    for engine in [EngineKey::Mysql, EngineKey::Mariadb] {
        let service = MockService::deterministic();
        let id =
            service.seed_instance(spec("shop", engine, "8.4.4", 3306), InstanceStatus::Running);
        assert!(
            !service.list_databases(&id).expect("databases").is_empty(),
            "{} admin must be supported",
            engine.display_name()
        );
        service.create_database(&id, "shop_db").expect("create");
        assert!(
            service
                .list_databases(&id)
                .expect("list")
                .iter()
                .any(|db| db.name() == "shop_db")
        );
        let password = service
            .create_user(&id, "shop_user", "shop_db", PrivilegeLevel::ReadOnly)
            .expect("create user");
        assert!(password.expose().starts_with("pwd-user-"));
    }
}

#[test]
fn mock_no_password_instance_reports_no_password() {
    let service = MockService::deterministic();
    let id = service.seed_instance(
        no_port_spec("nopw", EngineKey::Postgresql, "17.4"),
        InstanceStatus::Running,
    );
    let err = service.list_databases(&id).expect_err("no password");
    assert_eq!(err.code, crate::admin::error::CODE_NO_PASSWORD);
}

#[test]
fn mock_reports_a_not_found_instance() {
    let (service, _id) = seeded_pg();
    let missing = crate::model::InstanceId::new();
    let err = service.list_databases(&missing).expect_err("missing");
    assert_eq!(err.code, crate::error::CODE_NOT_FOUND);
}

#[test]
fn mock_check_update_reports_a_newer_version_and_emits() {
    let service = MockService::deterministic();
    let mut rx = service.subscribe();
    let status = service.check_update().expect("check");
    assert!(status.available.is_some());
    assert!(status.install_kind.can_self_update());
    let event = rx.try_recv().expect("an UpdateChecked event");
    assert!(matches!(event, ServiceEvent::UpdateChecked { .. }));
}

#[test]
fn mock_check_update_reports_up_to_date_at_the_mock_version() {
    let service = MockService::deterministic();
    service.set_current_version(semver::Version::parse("0.9.0").expect("version"));
    let status = service.check_update().expect("check");
    assert!(status.available.is_none());
}

#[test]
fn mock_apply_update_refuses_a_package_manager_install() {
    let service = MockService::deterministic();
    service.set_install_kind(InstallKind::DistPackage {
        exe_path: "/usr/bin/dblocal".into(),
        hint: PackageManagerHint::Dpkg,
    });
    let err = service.apply_update().expect_err("refused");
    assert_eq!(err.code, crate::update::CODE_UPDATE_UNSUPPORTED_KIND);
}

#[test]
fn mock_apply_update_succeeds_for_a_self_update_kind() {
    let service = MockService::deterministic();
    service.apply_update().expect("applied");
}

#[test]
fn mock_detects_a_fixed_client_cast() {
    let service = MockService::deterministic();
    let clients = service.detect_clients(false).expect("detect");
    let kinds: Vec<ClientKind> = clients.iter().map(|c| c.kind()).collect();
    assert!(kinds.contains(&ClientKind::Psql));
    assert!(kinds.contains(&ClientKind::TablePlus));
    // Refresh is a no-op but must not error.
    assert_eq!(
        service.detect_clients(true).expect("refresh").len(),
        clients.len()
    );
}

#[test]
fn mock_open_in_client_returns_a_plan() {
    let (service, id) = seeded_pg();
    let plan = service
        .open_in_client(&id, ClientKind::TablePlus, true)
        .expect("plan");
    assert!(plan.required_path().is_some());
    let err = service
        .open_in_client(&id, ClientKind::DBeaver, true)
        .expect_err("dbeaver is not detected");
    assert_eq!(
        err.code,
        crate::clients::ClientError::ClientNotFound {
            name: String::new()
        }
        .code()
    );
}
