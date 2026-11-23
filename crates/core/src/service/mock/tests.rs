//! Behavioural tests for the in-memory service.
//!
//! Every operation is awaited through the broadcast channel rather than a
//! clock, so the tests prove the same event flow the GUI consumes.

use std::time::Duration;

use tokio::sync::broadcast::{self, error::RecvError};

use crate::model::{
    AuthMode, CatalogStatus, DeleteMode, EngineKey, InstanceName, InstanceSpec, InstanceStatus,
    Port,
};
use crate::service::{DblService, OperationStage, ServiceEvent, UriFormat};

fn spec(name: &str, engine: EngineKey, version: &str, port: u16) -> InstanceSpec {
    InstanceSpec::new(
        InstanceName::new(name).expect("valid name"),
        engine,
        version,
        Some(Port::new(port).expect("valid port")),
        AuthMode::Password,
    )
    .expect("valid spec")
}

fn no_port_spec(name: &str, engine: EngineKey, version: &str) -> InstanceSpec {
    InstanceSpec::new(
        InstanceName::new(name).expect("valid name"),
        engine,
        version,
        None,
        AuthMode::None,
    )
    .expect("valid spec")
}

/// Collects events until `OperationFinished` arrives, with a hard timeout so a
/// stuck task fails the test instead of hanging the suite.
async fn drain_until_finished(
    rx: &mut broadcast::Receiver<ServiceEvent>,
) -> Result<(), crate::service::UserFacingError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(ServiceEvent::OperationFinished { result, .. })) => return result,
            Ok(Ok(_)) => continue,
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for OperationFinished"),
        }
    }
}

/// Collects every event up to and including `OperationFinished`.
async fn collect_until_finished(
    rx: &mut broadcast::Receiver<ServiceEvent>,
) -> (
    Vec<ServiceEvent>,
    Result<(), crate::service::UserFacingError>,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(ServiceEvent::OperationFinished { result, .. })) => {
                return (events, result);
            }
            Ok(Ok(event)) => events.push(event),
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for OperationFinished"),
        }
    }
}

#[tokio::test]
async fn create_with_installed_runtime_goes_straight_to_stopped() {
    let service = super::MockService::deterministic();
    let mut rx = service.subscribe();
    // Postgres 17.4 is pre-installed by the mock.
    let spec = spec("app-dev", EngineKey::Postgresql, "17.4", 5432);
    let op = service.create_instance(spec).expect("create accepted");
    drain_until_finished(&mut rx)
        .await
        .expect("create must succeed");
    service.cancel_operation(&op).expect("operation exists");

    let views = service.list_instances().expect("list");
    let view = views.iter().find(|v| v.name == "app-dev").expect("created");
    assert_eq!(view.status, InstanceStatus::Stopped);
    assert_eq!(view.port.get(), 5432);
    assert_eq!(view.auth, AuthMode::Password);
    assert!(view.credential_backend.is_some());
}

#[tokio::test]
async fn create_without_runtime_downloads_and_reports_progress() {
    let service = super::MockService::deterministic();
    let mut rx = service.subscribe();
    // MySQL 8.4.4 is not pre-installed.
    let spec = spec("mysql-shop", EngineKey::Mysql, "8.4.4", 3306);
    service.create_instance(spec).expect("create accepted");

    let mut stages = Vec::new();
    let mut saw_progress = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(ServiceEvent::OperationProgress { stage, .. })) => stages.push(stage),
            Ok(Ok(ServiceEvent::DownloadProgress { progress, .. })) => {
                saw_progress = true;
                assert!(progress.bytes_total > 0);
                assert!(progress.eta.is_some());
                assert!((0.0..=1.0).contains(&progress.fraction()));
            }
            Ok(Ok(ServiceEvent::OperationFinished { .. })) => break,
            Ok(Ok(_)) => continue,
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) => panic!("closed"),
            Err(_) => panic!("timed out"),
        }
    }
    assert!(saw_progress, "download progress must be emitted");
    assert!(stages.contains(&OperationStage::Downloading));
    assert!(stages.contains(&OperationStage::Verifying));
    assert!(stages.contains(&OperationStage::Extracting));
    assert!(stages.contains(&OperationStage::Initializing));

    let view = service
        .list_instances()
        .expect("list")
        .into_iter()
        .find(|v| v.name == "mysql-shop")
        .expect("created");
    assert_eq!(view.status, InstanceStatus::Stopped);
}

#[tokio::test]
async fn start_then_stop_moves_through_running() {
    let service = super::MockService::deterministic();
    let id = service.seed_instance(
        spec("pg-main", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );

    let mut rx = service.subscribe();
    service.start_instance(&id).expect("start accepted");
    drain_until_finished(&mut rx).await.expect("start ok");
    let view = service
        .list_instances()
        .expect("list")
        .into_iter()
        .find(|v| v.id == id)
        .expect("instance");
    assert_eq!(view.status, InstanceStatus::Running);
    assert!(view.failure.is_none());

    service.stop_instance(&id).expect("stop accepted");
    drain_until_finished(&mut rx).await.expect("stop ok");
    let view = service
        .list_instances()
        .expect("list")
        .into_iter()
        .find(|v| v.id == id)
        .expect("instance");
    assert_eq!(view.status, InstanceStatus::Stopped);
}

#[tokio::test]
async fn stopping_a_stopped_instance_is_an_invalid_transition() {
    let service = super::MockService::deterministic();
    let id = service.seed_instance(
        spec("pg-idle", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let err = service.stop_instance(&id).expect_err("cannot stop");
    assert_eq!(err.code(), "DBL-1004");
}

#[tokio::test]
async fn port_in_use_failure_surfaces_a_code_and_diagnosis() {
    let service = super::MockService::deterministic();
    let mut rx = service.subscribe();
    service.push_failure(super::ScriptedFailure::PortInUse { port: 5432 });
    service
        .create_instance(spec("conflict", EngineKey::Postgresql, "17.4", 5432))
        .expect("accepted");

    let err = drain_until_finished(&mut rx)
        .await
        .expect_err("must fail with port in use");
    assert_eq!(err.code, "DBL-1005");
    assert_eq!(err.diagnosis, Some(crate::model::StartFailure::PortInUse));

    let view = service
        .list_instances()
        .expect("list")
        .into_iter()
        .find(|v| v.name == "conflict")
        .expect("instance");
    assert_eq!(view.status, InstanceStatus::Failed);
    let failure = view.failure.expect("failure detail");
    assert_eq!(failure.code, "DBL-1005");
    assert_eq!(failure.diagnosis, crate::model::StartFailure::PortInUse);
    assert!(!failure.log_tail.is_empty());
}

#[tokio::test]
async fn download_failure_marks_the_instance_failed() {
    let service = super::MockService::deterministic();
    let mut rx = service.subscribe();
    service.push_failure(super::ScriptedFailure::DownloadFailed);
    service
        .create_instance(spec("broken-dl", EngineKey::Mariadb, "11.4.2", 3307))
        .expect("accepted");
    let err = drain_until_finished(&mut rx)
        .await
        .expect_err("download must fail");
    assert_eq!(err.code, "DBL-3002");
    let view = service
        .list_instances()
        .expect("list")
        .into_iter()
        .find(|v| v.name == "broken-dl")
        .expect("instance");
    assert_eq!(view.status, InstanceStatus::Failed);
}

#[tokio::test]
async fn start_timeout_failure_is_reported() {
    let service = super::MockService::deterministic();
    let id = service.seed_instance(
        spec("slow-start", EngineKey::Valkey, "8.1.0", 6379),
        InstanceStatus::Stopped,
    );
    let mut rx = service.subscribe();
    service.push_failure(super::ScriptedFailure::StartTimeout);
    service.start_instance(&id).expect("accepted");
    let err = drain_until_finished(&mut rx)
        .await
        .expect_err("start must time out");
    assert_eq!(err.code, "DBL-4001");
    let view = service
        .list_instances()
        .expect("list")
        .into_iter()
        .find(|v| v.id == id)
        .expect("instance");
    assert_eq!(view.status, InstanceStatus::Failed);
}

#[tokio::test]
async fn delete_archives_or_purges() {
    let service = super::MockService::deterministic();
    let archived_id = service.seed_instance(
        spec("keep-data", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let purged_id = service.seed_instance(
        spec("drop-data", EngineKey::Postgresql, "17.4", 5433),
        InstanceStatus::Stopped,
    );
    let mut rx = service.subscribe();

    service
        .delete_instance(&archived_id, DeleteMode::ArchiveData)
        .expect("delete accepted");
    drain_until_finished(&mut rx).await.expect("archive delete");
    service
        .delete_instance(&purged_id, DeleteMode::PurgeData)
        .expect("delete accepted");
    drain_until_finished(&mut rx).await.expect("purge delete");

    assert!(service.list_instances().expect("list").is_empty());
    let archives = service.list_archives().expect("archives");
    assert_eq!(archives.len(), 1, "only the archive-mode delete keeps data");
    assert_eq!(archives[0].instance_name().as_str(), "keep-data");
    service
        .purge_archive(archives[0].id())
        .expect("purge archive");
    assert!(service.list_archives().expect("archives").is_empty());
}

#[tokio::test]
async fn validate_spec_rejects_duplicate_names_and_ports() {
    let service = super::MockService::deterministic();
    service.seed_instance(
        spec("taken", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let duplicate_name = service
        .validate_spec(&spec("taken", EngineKey::Postgresql, "17.4", 5433))
        .expect_err("name taken");
    assert_eq!(duplicate_name.code(), "DBL-1003");
    let duplicate_port = service
        .validate_spec(&spec("fresh", EngineKey::Postgresql, "17.4", 5432))
        .expect_err("port taken");
    assert_eq!(duplicate_port.code(), "DBL-1001");
    assert!(duplicate_port.to_string().contains("taken"));
    service
        .validate_spec(&spec("fresh", EngineKey::Postgresql, "17.4", 5500))
        .expect("free name and port");
}

#[tokio::test]
async fn suggest_port_skips_allocated_ports() {
    let service = super::MockService::deterministic();
    assert_eq!(
        service
            .suggest_port(EngineKey::Postgresql)
            .expect("suggest")
            .get(),
        5432
    );
    service.seed_instance(
        spec("first", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    assert_eq!(
        service
            .suggest_port(EngineKey::Postgresql)
            .expect("suggest")
            .get(),
        5433
    );
}

#[tokio::test]
async fn connection_uri_and_dotenv_are_secret_bearing() {
    let service = super::MockService::deterministic();
    let id = service.seed_instance(
        spec("uri-test", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let uri = service
        .connection_uri(&id, UriFormat::Uri)
        .expect("uri")
        .expose()
        .to_owned();
    assert!(uri.starts_with("postgresql://"));
    assert!(uri.contains("127.0.0.1:5432"));

    let env = service
        .connection_uri(&id, UriFormat::DotEnv)
        .expect("env")
        .expose()
        .to_owned();
    assert!(env.contains("DATABASE_URL=postgresql://"));
    assert!(env.contains("DB_DATABASE=postgres"));
    assert!(env.contains("DB_PASSWORD="));
}

#[tokio::test]
async fn no_auth_instance_has_no_password() {
    let service = super::MockService::deterministic();
    let id = service.seed_instance(
        no_port_spec("trusted", EngineKey::Postgresql, "17.4"),
        InstanceStatus::Stopped,
    );
    let uri = service
        .connection_uri(&id, UriFormat::Uri)
        .expect("uri")
        .expose()
        .to_owned();
    assert!(!uri.contains(":hunter2"));
    assert!(uri.starts_with("postgresql://postgres@"));
    let view = service
        .list_instances()
        .expect("list")
        .into_iter()
        .find(|v| v.id == id)
        .expect("instance");
    assert_eq!(view.auth, AuthMode::None);
    assert!(view.credential_backend.is_none());
}

#[tokio::test]
async fn no_auth_uri_and_dotenv_match_the_core_rendering() {
    // Item 10: the mock must not hide a core defect. Render through both and
    // require the identical bytes for each engine and format (PRD F-CRD-03).
    for (engine, user, port) in [
        (EngineKey::Postgresql, "postgres", 5432u16),
        (EngineKey::Valkey, "default", 6379),
    ] {
        let service = super::MockService::deterministic();
        let spec = InstanceSpec::new(
            InstanceName::new("nopass").expect("name"),
            engine,
            "17.4",
            Some(Port::new(port).expect("port")),
            AuthMode::None,
        )
        .expect("spec");
        let id = service.seed_instance(spec, InstanceStatus::Stopped);
        for format in [UriFormat::Uri, UriFormat::DotEnv] {
            let mock = service
                .connection_uri(&id, format)
                .expect("uri")
                .expose()
                .to_owned();
            let core = crate::engine::uri::render_no_password(engine, port, user, format)
                .expose()
                .to_owned();
            assert_eq!(mock, core, "{engine:?} {format:?} diverged");
        }
    }
}

#[tokio::test]
async fn password_instance_without_a_stored_secret_is_a_typed_not_found() {
    // The mock always has a password today; a password instance with no stored
    // secret must fail typed (like the real service), never render an empty one.
    let service = super::MockService::deterministic();
    let id = service.seed_instance(
        spec("pwless", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    super::lock(&service.inner).passwords.remove(&id);
    let err = service
        .connection_uri(&id, UriFormat::Uri)
        .expect_err("must be typed");
    assert_eq!(err.code(), crate::error::CODE_NOT_FOUND);
}

#[tokio::test]
async fn logs_stream_only_while_watched() {
    let service = super::MockService::deterministic();
    let id = service.seed_instance(
        spec("log-test", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let mut rx = service.subscribe();

    service.start_instance(&id).expect("start");
    drain_until_finished(&mut rx).await.expect("start ok");

    service.watch_logs(&id, true).expect("watch");
    service.stop_instance(&id).expect("stop");
    let mut saw_lines = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(ServiceEvent::LogLines { id: line_id, lines })) => {
                assert_eq!(line_id, id);
                assert!(!lines.is_empty());
                saw_lines = true;
            }
            Ok(Ok(ServiceEvent::OperationFinished { .. })) => break,
            Ok(Ok(_)) => continue,
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) => panic!("closed"),
            Err(_) => panic!("timed out"),
        }
    }
    assert!(saw_lines, "watched instance must stream log lines");

    let tail = service.tail_logs(&id, 100).expect("tail");
    assert!(!tail.is_empty());
    let short = service.tail_logs(&id, 1).expect("tail 1");
    assert_eq!(short.len(), 1);
}

#[tokio::test]
async fn catalog_reports_every_freshness_with_its_banner() {
    let service = super::MockService::deterministic();
    let fresh = service.catalog().expect("catalog");
    assert_eq!(fresh.status, CatalogStatus::Fresh);
    assert!(fresh.banner.is_none());

    service.set_catalog_status(CatalogStatus::Stale);
    let stale = service.catalog().expect("catalog");
    assert_eq!(stale.status, CatalogStatus::Stale);
    assert_eq!(stale.banner.as_deref(), Some(crate::strings::CATALOG_STALE));

    service.set_catalog_status(CatalogStatus::Offline);
    let offline = service.catalog().expect("catalog");
    assert_eq!(offline.status, CatalogStatus::Offline);
    assert_eq!(
        offline.banner.as_deref(),
        Some(crate::strings::CATALOG_OFFLINE)
    );
}

#[tokio::test]
async fn refresh_catalog_emits_catalog_updated() {
    let service = super::MockService::deterministic();
    service.set_catalog_status(CatalogStatus::Offline);
    let mut rx = service.subscribe();
    service.refresh_catalog(true).expect("refresh accepted");
    drain_until_finished(&mut rx).await.expect("refresh ok");
    assert_eq!(
        service.catalog().expect("catalog").status,
        CatalogStatus::Fresh
    );
}

#[tokio::test]
async fn cancel_marks_the_operation() {
    let service = super::MockService::deterministic();
    let op = service
        .create_instance(spec("cancelled", EngineKey::Mysql, "8.4.4", 3306))
        .expect("accepted");
    service.cancel_operation(&op).expect("cancel accepted");
    let mut rx = service.subscribe();
    let err = drain_until_finished(&mut rx)
        .await
        .expect_err("cancelled create reports an error");
    assert_eq!(err.code, "DBL-7001");
}

#[tokio::test]
async fn clock_and_random_source_are_seedable() {
    let service = super::MockService::with_config(super::MockConfig::deterministic().with_seed(99));
    let start = service.now();
    service.advance_clock(Duration::from_secs(5));
    assert_eq!(service.now(), start + Duration::from_secs(5));
    let a = service.next_deterministic_id("x");
    let b = service.next_deterministic_id("x");
    assert_ne!(a, b, "the RNG must advance");
    assert!(a.starts_with("x-"));
}

#[tokio::test]
async fn change_port_rejects_a_port_owned_by_another_instance() {
    let service = super::MockService::deterministic();
    let a = service.seed_instance(
        spec("aa-inst", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let _b = service.seed_instance(
        spec("bb-inst", EngineKey::Postgresql, "17.4", 5433),
        InstanceStatus::Stopped,
    );
    let err = service
        .change_port(&a, Port::new(5433).expect("valid"))
        .expect_err("port owned");
    assert_eq!(err.code(), "DBL-1001");
    service
        .change_port(&a, Port::new(5440).expect("valid"))
        .expect("free port");
}

#[tokio::test]
async fn runtimes_and_archives_round_trip() {
    let service = super::MockService::deterministic();
    let runtimes = service.list_runtimes().expect("runtimes");
    assert!(!runtimes.is_empty());
    let first = runtimes[0].id().to_owned();
    service.remove_runtime(&first).expect("remove runtime");
    assert_eq!(
        service.list_runtimes().expect("runtimes").len(),
        runtimes.len() - 1
    );
    assert!(service.purge_archive("missing").is_err());
}

#[tokio::test]
async fn data_size_and_autostart_only_touch_real_instances() {
    let service = super::MockService::deterministic();
    let id = service.seed_instance(
        spec("size", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    assert_eq!(service.data_size(&id).expect("size"), 41 * 1024 * 1024);
    service.set_autostart(&id, true).expect("autostart");
    let view = service
        .list_instances()
        .expect("list")
        .into_iter()
        .find(|v| v.id == id)
        .expect("instance");
    assert!(view.autostart);
    let missing = crate::model::InstanceId::new();
    assert!(service.data_size(&missing).is_err());
    assert!(service.set_autostart(&missing, true).is_err());
    assert!(service.tail_logs(&missing, 10).is_err());
    assert!(service.watch_logs(&missing, true).is_err());
    assert!(service.connection_uri(&missing, UriFormat::Uri).is_err());
}

#[tokio::test]
async fn delete_emits_instance_removed_before_finishing() {
    let service = super::MockService::deterministic();
    let id = service.seed_instance(
        spec("gone-one", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let mut rx = service.subscribe();
    service
        .delete_instance(&id, DeleteMode::ArchiveData)
        .expect("delete accepted");
    let (events, result) = collect_until_finished(&mut rx).await;
    result.expect("delete succeeds");

    let removed_at = events.iter().position(
        |event| matches!(event, ServiceEvent::InstanceRemoved { id: gone } if gone == &id),
    );
    assert!(
        removed_at.is_some(),
        "delete must emit InstanceRemoved: {events:?}"
    );
    // The row is gone from the service's own read model too.
    assert!(
        service
            .list_instances()
            .expect("list")
            .iter()
            .all(|v| v.id != id)
    );
}

#[tokio::test]
async fn stop_needs_force_emits_stop_forced_before_finishing() {
    let service = super::MockService::deterministic();
    let id = service.seed_instance(
        spec("forced-one", EngineKey::Valkey, "8.1.0", 6379),
        InstanceStatus::Running,
    );
    let mut rx = service.subscribe();
    service.push_failure(super::ScriptedFailure::StopNeedsForce);
    service.stop_instance(&id).expect("stop accepted");
    let (events, result) = collect_until_finished(&mut rx).await;
    result.expect("stop succeeds");

    let forced_at = events
        .iter()
        .position(|event| matches!(event, ServiceEvent::StopForced { id: f } if f == &id));
    assert!(
        forced_at.is_some(),
        "a forced stop must emit StopForced: {events:?}"
    );
    // StopForced arrives before the InstanceChanged that carries Stopped.
    let changed_at = events.iter().position(|event| {
        matches!(event, ServiceEvent::InstanceChanged { view } if view.id == id && view.status == InstanceStatus::Stopped)
    });
    if let Some(changed_at) = changed_at {
        assert!(
            forced_at.unwrap() < changed_at,
            "StopForced must precede the final InstanceChanged"
        );
    }
    let view = service
        .list_instances()
        .expect("list")
        .into_iter()
        .find(|v| v.id == id)
        .expect("instance");
    assert_eq!(view.status, InstanceStatus::Stopped);
}

#[tokio::test]
async fn rolled_back_create_emits_instance_removed() {
    let service = super::MockService::deterministic();
    let mut rx = service.subscribe();
    service.push_failure(super::ScriptedFailure::CreateRolledBack);
    service
        .create_instance(spec("rollback-me", EngineKey::Postgresql, "17.4", 5432))
        .expect("create accepted");
    let (events, result) = collect_until_finished(&mut rx).await;
    assert!(result.is_err(), "a rolled-back create reports failure");

    let removed = events
        .iter()
        .filter_map(|event| match event {
            ServiceEvent::InstanceRemoved { id } => Some(id),
            _ => None,
        })
        .count();
    assert_eq!(
        removed, 1,
        "the half-created row must be removed exactly once"
    );
    // And it is gone from the read model: no ghost row survives.
    assert!(
        service
            .list_instances()
            .expect("list")
            .iter()
            .all(|v| v.name != "rollback-me"),
        "a rolled-back create must not leave a row"
    );
}

// --- W4-A: admin, update and open-in-client on the mock ----------------------

use crate::admin::PrivilegeLevel;
use crate::clients::ClientKind;
use crate::update::{InstallKind, PackageManagerHint};

fn seeded_pg() -> (super::MockService, crate::model::InstanceId) {
    let service = super::MockService::deterministic();
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
    let service = super::MockService::deterministic();
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
        let service = super::MockService::deterministic();
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
    let service = super::MockService::deterministic();
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
    let service = super::MockService::deterministic();
    let mut rx = service.subscribe();
    let status = service.check_update().expect("check");
    assert!(status.available.is_some());
    assert!(status.install_kind.can_self_update());
    let event = rx.try_recv().expect("an UpdateChecked event");
    assert!(matches!(event, ServiceEvent::UpdateChecked { .. }));
}

#[test]
fn mock_check_update_reports_up_to_date_at_the_mock_version() {
    let service = super::MockService::deterministic();
    service.set_current_version(semver::Version::parse("0.9.0").expect("version"));
    let status = service.check_update().expect("check");
    assert!(status.available.is_none());
}

#[test]
fn mock_apply_update_refuses_a_package_manager_install() {
    let service = super::MockService::deterministic();
    service.set_install_kind(InstallKind::DistPackage {
        exe_path: "/usr/bin/dblocal".into(),
        hint: PackageManagerHint::Dpkg,
    });
    let err = service.apply_update().expect_err("refused");
    assert_eq!(err.code, crate::update::CODE_UPDATE_UNSUPPORTED_KIND);
}

#[test]
fn mock_apply_update_succeeds_for_a_self_update_kind() {
    let service = super::MockService::deterministic();
    service.apply_update().expect("applied");
}

#[test]
fn mock_detects_a_fixed_client_cast() {
    let service = super::MockService::deterministic();
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
