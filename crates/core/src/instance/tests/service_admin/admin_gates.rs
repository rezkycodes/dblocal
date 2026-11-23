//! F1/F2/F3 regression tests for the admin wiring (`W4-A3` fix 1).
//!
//! Split from `mod.rs` to keep both files inside the 800-line limit.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::sync::Arc;

use crate::admin::AdminError;
use crate::model::{EngineKey, Instance, InstanceId};
use crate::secret::SecretString;
use crate::service::DblService;
use crate::service::SecretAccess;

use super::{
    FakeAdmin, PrivilegeLevel, SharedAdmin, harness_with_admin, mark_running, seed_row, test_port,
};
use crate::instance::tests::harness::Harness;

// --- F2: the running gate before any dial, for every admin call --------------
//
// `docs/architecture/admin.md` claimed admin only dials a Running instance; the
// code never checked. A stopped MySQL's freed port may now belong to another
// process, and admin would hand it the superuser password. Every admin call must
// refuse a non-running row with the same DBL-8100 a stopped server already gave,
// before the credential is read and before the factory builds an executor.

/// A harness whose admin factory counts its calls and never dials.
fn harness_counting_admin(
    engine: EngineKey,
) -> (Harness, InstanceId, Arc<std::sync::atomic::AtomicUsize>) {
    let factory_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&factory_calls);
    let admin = Arc::new(FakeAdmin::default());
    let h = Harness::with_options(engine, {
        let admin = Arc::clone(&admin);
        move |options| {
            let admin = Arc::clone(&admin);
            let counter = Arc::clone(&counter);
            options.admin_factory = Some(Arc::new(move |_instance, _password| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Box::new(SharedAdmin(Arc::clone(&admin)))
                    as Box<dyn crate::admin::AdminExecutor>)
            }));
        }
    });
    let name = format!("gate{}", test_port() % 1000);
    let id = seed_row(&h, engine, &name);
    (h, id, factory_calls)
}

/// Every entry point into admin, as a thunk over `id`.
fn every_admin_call(
    h: &Harness,
    id: &InstanceId,
) -> Vec<(
    &'static str,
    crate::service::ServiceResult<Box<dyn std::any::Any>>,
)> {
    let boxed = |r: crate::service::ServiceResult<Box<dyn std::any::Any>>| r;
    vec![
        (
            "list_databases",
            boxed(
                h.service
                    .list_databases(id)
                    .map(|v| Box::new(v) as Box<dyn std::any::Any>),
            ),
        ),
        (
            "list_users",
            boxed(
                h.service
                    .list_users(id)
                    .map(|v| Box::new(v) as Box<dyn std::any::Any>),
            ),
        ),
        (
            "create_database",
            boxed(
                h.service
                    .create_database(id, "gate_db")
                    .map(|()| Box::new(()) as Box<dyn std::any::Any>),
            ),
        ),
        (
            "drop_database",
            boxed(
                h.service
                    .drop_database(id, "gate_db", true)
                    .map(|()| Box::new(()) as Box<dyn std::any::Any>),
            ),
        ),
        (
            "create_user",
            boxed(
                h.service
                    .create_user(id, "gate_user", "gate_db", PrivilegeLevel::ReadOnly)
                    .map(|p| Box::new(p) as Box<dyn std::any::Any>),
            ),
        ),
        (
            "reset_superuser_password",
            boxed(
                h.service
                    .reset_superuser_password(id)
                    .map(|p| Box::new(p) as Box<dyn std::any::Any>),
            ),
        ),
    ]
}

#[test]
fn every_admin_call_refuses_a_non_running_instance_before_reading_or_dialling() {
    for engine in [EngineKey::Postgresql, EngineKey::Mysql, EngineKey::Mariadb] {
        for status in [
            crate::model::InstanceStatus::Stopped,
            crate::model::InstanceStatus::Starting,
            crate::model::InstanceStatus::Stopping,
            crate::model::InstanceStatus::Failed,
        ] {
            let (h, id, factory_calls) = harness_counting_admin(engine);
            mark_running(&h, &id);
            h.store().update_status(&id, status).expect("set status");
            // No credential reads may reach the secrets backend.
            let gets_before = h.secrets.file.get_calls();

            for (name, result) in every_admin_call(&h, &id) {
                let err = result.expect_err(name);
                assert_eq!(
                    err.code,
                    crate::admin::error::CODE_CONNECT,
                    "{} in {status:?} must refuse {name} with DBL-8100",
                    engine.display_name()
                );
                assert!(
                    err.message.contains("Pastikan server sedang berjalan"),
                    "{name}: the message must say the server is not running: {}",
                    err.message
                );
            }

            assert_eq!(
                factory_calls.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "{} in {status:?}: the executor factory must never be built",
                engine.display_name()
            );
            assert_eq!(
                h.secrets.file.get_calls(),
                gets_before,
                "{} in {status:?}: the credential must never be read",
                engine.display_name()
            );
        }
    }
}

#[test]
fn a_running_instance_still_reaches_the_executor() {
    // The gate must not block the normal path: the same harness, left Running,
    // reaches the fake for every call.
    let admin = Arc::new(FakeAdmin::default());
    let (h, id) = harness_with_admin(EngineKey::Postgresql, Arc::clone(&admin));
    h.service.list_databases(&id).expect("running lists");
    h.service.list_users(&id).expect("running lists users");
    h.service.create_database(&id, "app_db").expect("create");
    h.service.drop_database(&id, "app_db", true).expect("drop");
    h.service
        .create_user(&id, "app_user", "app_db", PrivilegeLevel::ReadOnly)
        .expect("create user");
    h.service.reset_superuser_password(&id).expect("reset");
    assert!(
        !admin.calls().is_empty(),
        "a Running instance must reach the executor"
    );
}

// --- F1/F3: the port and socket that reach MysqlTarget -----------------------
//
// The instance port reaching the admin target was untested: replacing it with
// `0` left every lib test green, because on unix the socket is what is dialled.
// On Windows the target dials that port, so it must be provable here.

/// The pure target builder the production factory uses, built from a seeded row.
fn mysql_target_for(engine: EngineKey, port: u16) -> (Harness, crate::admin::MysqlTarget) {
    let h = Harness::with_options(engine, |_| {});
    let name = format!("tgt{}", test_port() % 1000);
    let id = seed_row_port(&h, engine, &name, port);
    let instance = h.instance(&id).expect("row");
    let target = crate::instance::admin_ops::mysql_target(
        &h.paths,
        &instance,
        SecretString::new("target-pw"),
    )
    .expect("target builds");
    (h, target)
}

/// Seeds a `Running` MySQL/MariaDB row on an explicit port.
fn seed_row_port(h: &Harness, engine: EngineKey, name: &str, port: u16) -> InstanceId {
    let spec = h.spec(name, engine, port);
    let id = InstanceId::new();
    let instance = Instance::new(
        &spec,
        id.clone(),
        crate::model::InstanceStatus::Running,
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
fn the_mysql_target_carries_the_instance_port_not_zero() {
    // Two different ports, one per engine, so a hardcoded value cannot pass.
    for (engine, port) in [
        (EngineKey::Mysql, 41_001u16),
        (EngineKey::Mariadb, 41_002u16),
    ] {
        let (_h, target) = mysql_target_for(engine, port);
        assert_eq!(
            target.port(),
            port,
            "{}: the target must carry the instance port",
            engine.display_name()
        );
        assert_ne!(
            target.port(),
            0,
            "{}: port 0 is the bug this test exists for",
            engine.display_name()
        );
    }
}

#[test]
fn the_mysql_target_carries_the_configured_socket_on_unix() {
    for engine in [EngineKey::Mysql, EngineKey::Mariadb] {
        let (h, target) = mysql_target_for(engine, 41_010);
        let name = target
            .socket()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        // The adapter names the socket `<engine>-<port>.sock`, so the name proves
        // the target got the adapter's configured path for this instance and port.
        assert!(
            name.ends_with("-41010.sock"),
            "{}: socket {:?} must name the instance port",
            engine.display_name(),
            target.socket()
        );
        // The path lives under this harness's own root, not a shared location.
        assert!(
            target.socket().starts_with(h.paths.root()),
            "{}: socket {:?} must be inside the harness root {}",
            engine.display_name(),
            target.socket(),
            h.paths.root().display()
        );
        // And the transport takes the socket arm on this unix host.
        assert!(
            matches!(
                target.transport(),
                crate::admin::mysql::Transport::Socket(_)
            ),
            "{}: a unix host must use the socket arm",
            engine.display_name()
        );
    }
}

#[test]
fn a_non_mysql_engine_is_unsupported_not_a_connect_failure() {
    // F3: the `_ =>` arm of the old builder turned into DBL-8100; it must stay
    // "not supported" (DBL-8111).
    let h = Harness::with_options(EngineKey::Postgresql, |_| {});
    let name = format!("tgtpg{}", test_port() % 1000);
    let id = seed_row_port(&h, EngineKey::Postgresql, &name, 41_020);
    let instance = h.instance(&id).expect("row");
    let err = crate::instance::admin_ops::mysql_target(
        &h.paths,
        &instance,
        SecretString::new("target-pw"),
    )
    .expect_err("a PostgreSQL row has no MySQL socket or port");
    assert_eq!(
        err,
        AdminError::Unsupported,
        "a wrong engine is not supported, never a connect failure"
    );
    assert_ne!(err, AdminError::Connect);
    assert_eq!(err.code(), crate::admin::error::CODE_UNSUPPORTED);
}
