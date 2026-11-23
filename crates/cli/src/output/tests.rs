//! Tests for the output module: formatters, exit-code mapping and the
//! event-stream waiters. Kept beside `output.rs` so that file stays under the
//! 800-line ceiling.

use super::*;
use dblocal_core::model::{
    AuthMode, DownloadProgress, EngineKey, InstanceId, InstanceName, InstanceSpec, InstanceStatus,
    Port,
};
use dblocal_core::service::mock::MockService;
use dblocal_core::service::{
    DblService, FailureInfo, InstanceView, OperationId, OperationStage, ServiceEvent,
    UserFacingError,
};

fn op() -> OperationId {
    OperationId::new("op-test")
}

fn instance_id() -> InstanceId {
    InstanceId::parse("00000000-0000-4000-8000-000000000002").expect("uuid")
}

fn spec() -> InstanceSpec {
    InstanceSpec::new(
        InstanceName::new("output-app").expect("name"),
        EngineKey::Postgresql,
        "17.4",
        Some(Port::new(5432).expect("port")),
        AuthMode::Password,
    )
    .expect("spec")
}

/// A service over the mock whose reported status (and optional failure) is
/// scripted, so the wait paths are reachable without a real engine.
struct FakeService {
    inner: MockService,
    status: InstanceStatus,
    fail: bool,
}

impl FakeService {
    fn new(status: InstanceStatus, fail: bool) -> Self {
        let inner = MockService::deterministic();
        inner.seed_instance(spec(), status);
        Self {
            inner,
            status,
            fail,
        }
    }

    fn id(&self) -> InstanceId {
        DblService::list_instances(&self.inner)
            .expect("views")
            .remove(0)
            .id
    }

    fn views(&self) -> Vec<InstanceView> {
        let mut views = DblService::list_instances(&self.inner).expect("views");
        for view in &mut views {
            view.status = self.status;
            if self.fail {
                view.failure = Some(FailureInfo {
                    code: "DBL-4001".to_owned(),
                    message: "engine failed".to_owned(),
                    diagnosis: dblocal_core::model::StartFailure::PortInUse,
                    log_tail: Vec::new(),
                    restore_available: false,
                });
            }
        }
        views
    }
}

impl DblService for FakeService {
    fn list_instances(&self) -> dblocal_core::Result<Vec<InstanceView>> {
        Ok(self.views())
    }
    fn catalog(&self) -> dblocal_core::Result<dblocal_core::service::CatalogView> {
        self.inner.catalog()
    }
    fn refresh_catalog(&self, force: bool) -> dblocal_core::Result<OperationId> {
        self.inner.refresh_catalog(force)
    }
    fn validate_spec(&self, spec: &InstanceSpec) -> dblocal_core::Result<()> {
        self.inner.validate_spec(spec)
    }
    fn suggest_port(&self, engine: EngineKey) -> dblocal_core::Result<Port> {
        self.inner.suggest_port(engine)
    }
    fn create_instance(&self, spec: InstanceSpec) -> dblocal_core::Result<OperationId> {
        self.inner.create_instance(spec)
    }
    fn start_instance(&self, id: &InstanceId) -> dblocal_core::Result<OperationId> {
        self.inner.start_instance(id)
    }
    fn stop_instance(&self, id: &InstanceId) -> dblocal_core::Result<OperationId> {
        self.inner.stop_instance(id)
    }
    fn delete_instance(
        &self,
        id: &InstanceId,
        mode: dblocal_core::model::DeleteMode,
    ) -> dblocal_core::Result<OperationId> {
        self.inner.delete_instance(id, mode)
    }
    fn change_port(&self, id: &InstanceId, port: Port) -> dblocal_core::Result<()> {
        self.inner.change_port(id, port)
    }
    fn data_size(&self, id: &InstanceId) -> dblocal_core::Result<u64> {
        self.inner.data_size(id)
    }
    fn set_autostart(&self, id: &InstanceId, enabled: bool) -> dblocal_core::Result<()> {
        self.inner.set_autostart(id, enabled)
    }
    fn cancel_operation(&self, op: &OperationId) -> dblocal_core::Result<()> {
        self.inner.cancel_operation(op)
    }
    fn connection_uri(
        &self,
        id: &InstanceId,
        format: dblocal_core::service::UriFormat,
    ) -> dblocal_core::Result<dblocal_core::secret::SecretString> {
        self.inner.connection_uri(id, format)
    }
    fn tail_logs(&self, id: &InstanceId, lines: usize) -> dblocal_core::Result<Vec<String>> {
        self.inner.tail_logs(id, lines)
    }
    fn watch_logs(&self, id: &InstanceId, enabled: bool) -> dblocal_core::Result<()> {
        self.inner.watch_logs(id, enabled)
    }
    fn list_runtimes(&self) -> dblocal_core::Result<Vec<dblocal_core::model::Runtime>> {
        self.inner.list_runtimes()
    }
    fn remove_runtime(&self, runtime: &str) -> dblocal_core::Result<()> {
        self.inner.remove_runtime(runtime)
    }
    fn list_archives(&self) -> dblocal_core::Result<Vec<dblocal_core::model::ArchivedData>> {
        self.inner.list_archives()
    }
    fn purge_archive(&self, archive: &str) -> dblocal_core::Result<()> {
        self.inner.purge_archive(archive)
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ServiceEvent> {
        self.inner.subscribe()
    }
}

/// A service whose single instance is always in one status and never fails.
struct StaticService {
    status: InstanceStatus,
}

impl DblService for StaticService {
    fn list_instances(&self) -> dblocal_core::Result<Vec<InstanceView>> {
        let instance = dblocal_core::model::Instance::new(
            &spec(),
            instance_id(),
            self.status,
            std::time::SystemTime::UNIX_EPOCH,
        );
        Ok(vec![InstanceView::from_instance(&instance)])
    }
    fn catalog(&self) -> dblocal_core::Result<dblocal_core::service::CatalogView> {
        Err(dblocal_core::DblError::NotImplemented("catalog".into()))
    }
    fn refresh_catalog(&self, _f: bool) -> dblocal_core::Result<OperationId> {
        Err(dblocal_core::DblError::NotImplemented("refresh".into()))
    }
    fn validate_spec(&self, _s: &InstanceSpec) -> dblocal_core::Result<()> {
        Ok(())
    }
    fn suggest_port(&self, _e: EngineKey) -> dblocal_core::Result<Port> {
        Err(dblocal_core::DblError::NotImplemented("port".into()))
    }
    fn create_instance(&self, _s: InstanceSpec) -> dblocal_core::Result<OperationId> {
        Err(dblocal_core::DblError::NotImplemented("create".into()))
    }
    fn start_instance(&self, _i: &InstanceId) -> dblocal_core::Result<OperationId> {
        Err(dblocal_core::DblError::NotImplemented("start".into()))
    }
    fn stop_instance(&self, _i: &InstanceId) -> dblocal_core::Result<OperationId> {
        Err(dblocal_core::DblError::NotImplemented("stop".into()))
    }
    fn delete_instance(
        &self,
        _i: &InstanceId,
        _m: dblocal_core::model::DeleteMode,
    ) -> dblocal_core::Result<OperationId> {
        Err(dblocal_core::DblError::NotImplemented("delete".into()))
    }
    fn change_port(&self, _i: &InstanceId, _p: Port) -> dblocal_core::Result<()> {
        Ok(())
    }
    fn data_size(&self, _i: &InstanceId) -> dblocal_core::Result<u64> {
        Ok(0)
    }
    fn set_autostart(&self, _i: &InstanceId, _e: bool) -> dblocal_core::Result<()> {
        Ok(())
    }
    fn cancel_operation(&self, _o: &OperationId) -> dblocal_core::Result<()> {
        Ok(())
    }
    fn connection_uri(
        &self,
        _i: &InstanceId,
        _f: dblocal_core::service::UriFormat,
    ) -> dblocal_core::Result<dblocal_core::secret::SecretString> {
        Err(dblocal_core::DblError::NotImplemented("uri".into()))
    }
    fn tail_logs(&self, _i: &InstanceId, _n: usize) -> dblocal_core::Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn watch_logs(&self, _i: &InstanceId, _e: bool) -> dblocal_core::Result<()> {
        Ok(())
    }
    fn list_runtimes(&self) -> dblocal_core::Result<Vec<dblocal_core::model::Runtime>> {
        Ok(Vec::new())
    }
    fn remove_runtime(&self, _r: &str) -> dblocal_core::Result<()> {
        Ok(())
    }
    fn list_archives(&self) -> dblocal_core::Result<Vec<dblocal_core::model::ArchivedData>> {
        Ok(Vec::new())
    }
    fn purge_archive(&self, _a: &str) -> dblocal_core::Result<()> {
        Ok(())
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ServiceEvent> {
        let (tx, rx) = tokio::sync::broadcast::channel(4);
        drop(tx);
        rx
    }
}

/// Runs `f` on its own thread and fails the test, rather than hanging the test
/// binary, if it has not finished within `limit`.
///
/// A mutation that ignores a timeout turns the wait into "wait until the channel
/// closes", which never happens in these tests; without this wrapper the whole
/// test binary would hang for minutes and the mutation would look like a hang
/// instead of a red test.
fn with_deadline<T, F>(limit: Duration, f: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(limit) {
        Ok(value) => value,
        Err(_) => panic!("the wait did not return within {limit:?}; a timeout was ignored"),
    }
}

#[test]
fn wait_operation_rx_reports_progress_and_returns_the_outcome() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let op = op();
    tx.send(ServiceEvent::OperationProgress {
        id: op.clone(),
        stage: OperationStage::Downloading,
    })
    .expect("send");
    tx.send(ServiceEvent::DownloadProgress {
        id: op.clone(),
        progress: DownloadProgress::new(5, 10, 1, None),
    })
    .expect("send");
    tx.send(ServiceEvent::OperationFinished {
        id: op.clone(),
        result: Ok(()),
    })
    .expect("send");
    drop(tx);
    let service = StaticService {
        status: InstanceStatus::Stopped,
    };
    let mut err = Vec::new();
    let result = wait_operation_rx(
        &mut rx,
        &op,
        &service,
        Target::Name("never"),
        Completion::StatusIn(&[InstanceStatus::Stopped]),
        &mut err,
        true,
    );
    assert!(result.is_ok());
    let text = String::from_utf8_lossy(&err);
    assert!(text.contains("downloading"), "err: {text}");
    assert!(text.contains("unduhan 50%"), "err: {text}");
}

#[test]
fn wait_operation_rx_propagates_a_failure() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(4);
    let op = op();
    tx.send(ServiceEvent::OperationFinished {
        id: op.clone(),
        result: Err(UserFacingError::new("DBL-2001", "bad")),
    })
    .expect("send");
    drop(tx);
    let service = StaticService {
        status: InstanceStatus::Stopped,
    };
    let mut err = Vec::new();
    let failure = wait_operation_rx(
        &mut rx,
        &op,
        &service,
        Target::Name("never"),
        Completion::StatusIn(&[InstanceStatus::Stopped]),
        &mut err,
        false,
    )
    .expect_err("must fail");
    assert_eq!(failure.code, "DBL-2001");
}

#[test]
fn wait_operation_rx_reports_a_closed_stream() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(2);
    drop(tx);
    let service = StaticService {
        status: InstanceStatus::Stopped,
    };
    let mut err = Vec::new();
    let failure = wait_operation_rx(
        &mut rx,
        &op(),
        &service,
        Target::Name("never"),
        Completion::StatusIn(&[InstanceStatus::Stopped]),
        &mut err,
        false,
    )
    .expect_err("closed");
    assert_eq!(failure.code, "DBL-3003");
}

/// A dropped `OperationFinished` must not become a hang or a wrong verdict: on
/// `Lagged` the store is re-read and decides. The channel has capacity 1 and the
/// only message after the burst is irrelevant, so the fix is the only way out.
#[test]
fn wait_operation_rx_resolves_a_lagged_finish_from_the_store() {
    let service = FakeService::new(InstanceStatus::Stopped, false);
    let (tx, mut rx) = tokio::sync::broadcast::channel(1);
    tx.send(ServiceEvent::CatalogUpdated {
        status: dblocal_core::model::CatalogStatus::Fresh,
    })
    .expect("send");
    tx.send(ServiceEvent::LogLines {
        id: instance_id(),
        lines: Vec::new(),
    })
    .expect("send");
    drop(tx);
    let mut err = Vec::new();
    let result = with_deadline(Duration::from_secs(5), move || {
        wait_operation_rx(
            &mut rx,
            &op(),
            &service,
            Target::Name("output-app"),
            Completion::StatusIn(&[InstanceStatus::Stopped]),
            &mut err,
            false,
        )
    });
    assert!(
        result.is_ok(),
        "the completed create must resolve: {result:?}"
    );
}

/// The same for a delete: on `Lagged`, a vanished instance means success.
#[test]
fn wait_operation_rx_resolves_a_lagged_delete_from_the_store() {
    let service = StaticService {
        status: InstanceStatus::Deleting,
    };
    let (tx, mut rx) = tokio::sync::broadcast::channel(1);
    tx.send(ServiceEvent::CatalogUpdated {
        status: dblocal_core::model::CatalogStatus::Fresh,
    })
    .expect("send");
    tx.send(ServiceEvent::LogLines {
        id: instance_id(),
        lines: Vec::new(),
    })
    .expect("send");
    drop(tx);
    let mut err = Vec::new();
    let result = with_deadline(Duration::from_secs(5), move || {
        wait_operation_rx(
            &mut rx,
            &op(),
            &service,
            Target::Id(&instance_id()),
            Completion::Gone,
            &mut err,
            false,
        )
    });
    // StaticService always lists the instance, so `Gone` cannot be proven and
    // the closed stream is the honest answer.
    assert_eq!(result.expect_err("still listed").code, "DBL-3003");
}

#[test]
fn wait_for_status_rx_times_out_at_the_deadline() {
    let failure = with_deadline(Duration::from_secs(5), || {
        let (_tx, mut rx) = tokio::sync::broadcast::channel(2);
        let service = StaticService {
            status: InstanceStatus::Starting,
        };
        let op = op();
        let id = instance_id();
        let wait = StatusWait {
            op: &op,
            instance_id: &id,
            ok_statuses: &[InstanceStatus::Running],
            timeout: Duration::from_millis(50),
            show_progress: false,
        };
        let mut err = Vec::new();
        wait_for_status_rx(&mut rx, &service, wait, &mut err).expect_err("timeout")
    });
    assert_eq!(failure.code, crate::messages::CODE_TIMEOUT);
}

#[test]
fn wait_for_status_rx_reports_a_failed_view() {
    let service = FakeService::new(InstanceStatus::Failed, true);
    let id = service.id();
    let (tx, mut rx) = tokio::sync::broadcast::channel(4);
    let view = service.views().remove(0);
    tx.send(ServiceEvent::InstanceChanged { view })
        .expect("send");
    let op = op();
    let wait = StatusWait {
        op: &op,
        instance_id: &id,
        ok_statuses: &[InstanceStatus::Running],
        timeout: Duration::from_secs(1),
        show_progress: false,
    };
    let mut err = Vec::new();
    let failure = wait_for_status_rx(&mut rx, &service, wait, &mut err).expect_err("failed");
    assert_eq!(failure.code, "DBL-4001");
    assert_eq!(
        failure.diagnosis,
        Some(dblocal_core::model::StartFailure::PortInUse)
    );
}

#[test]
fn wait_for_status_rx_reports_a_failed_view_without_details() {
    let service = StaticService {
        status: InstanceStatus::Running,
    };
    let id = instance_id();
    let (tx, mut rx) = tokio::sync::broadcast::channel(4);
    let instance = dblocal_core::model::Instance::new(
        &spec(),
        id.clone(),
        InstanceStatus::Failed,
        std::time::SystemTime::UNIX_EPOCH,
    );
    tx.send(ServiceEvent::InstanceChanged {
        view: InstanceView::from_instance(&instance),
    })
    .expect("send");
    let op = op();
    let wait = StatusWait {
        op: &op,
        instance_id: &id,
        ok_statuses: &[InstanceStatus::Running],
        timeout: Duration::from_secs(1),
        show_progress: false,
    };
    let mut err = Vec::new();
    let failure = wait_for_status_rx(&mut rx, &service, wait, &mut err).expect_err("failed");
    assert_eq!(failure.code, "DBL-4001");
    assert!(failure.diagnosis.is_none());
}

#[test]
fn wait_for_status_rx_accepts_the_ok_status() {
    let service = FakeService::new(InstanceStatus::Running, false);
    let id = service.id();
    let (tx, mut rx) = tokio::sync::broadcast::channel(4);
    let view = service.views().remove(0);
    tx.send(ServiceEvent::InstanceChanged { view })
        .expect("send");
    let op = op();
    let wait = StatusWait {
        op: &op,
        instance_id: &id,
        ok_statuses: &[InstanceStatus::Running],
        timeout: Duration::from_secs(1),
        show_progress: true,
    };
    let mut err = Vec::new();
    let status = wait_for_status_rx(&mut rx, &service, wait, &mut err).expect("running");
    assert_eq!(status, InstanceStatus::Running);
}

#[test]
fn wait_for_status_rx_resolves_from_finish_by_re_reading_the_status() {
    let service = FakeService::new(InstanceStatus::Running, false);
    let id = instance_id();
    let op = op();
    let (tx, mut rx) = tokio::sync::broadcast::channel(8);
    tx.send(ServiceEvent::OperationProgress {
        id: op.clone(),
        stage: OperationStage::Starting,
    })
    .expect("send");
    tx.send(ServiceEvent::DownloadProgress {
        id: op.clone(),
        progress: DownloadProgress::new(5, 10, 1, None),
    })
    .expect("send");
    tx.send(ServiceEvent::OperationFinished {
        id: op.clone(),
        result: Ok(()),
    })
    .expect("send");
    let wait = StatusWait {
        op: &op,
        instance_id: &id,
        ok_statuses: &[InstanceStatus::Running],
        timeout: Duration::from_secs(1),
        show_progress: true,
    };
    let mut err = Vec::new();
    let status = wait_for_status_rx(&mut rx, &service, wait, &mut err).expect("running");
    assert_eq!(status, InstanceStatus::Running);
    let text = String::from_utf8_lossy(&err);
    assert!(text.contains("starting"), "err: {text}");
    assert!(text.contains("unduhan 50%"), "err: {text}");
}

#[test]
fn wait_for_status_rx_propagates_an_operation_error() {
    let service = StaticService {
        status: InstanceStatus::Starting,
    };
    let id = instance_id();
    let op = op();
    let (tx, mut rx) = tokio::sync::broadcast::channel(4);
    tx.send(ServiceEvent::OperationFinished {
        id: op.clone(),
        result: Err(UserFacingError::new("DBL-1005", "port")),
    })
    .expect("send");
    let wait = StatusWait {
        op: &op,
        instance_id: &id,
        ok_statuses: &[InstanceStatus::Running],
        timeout: Duration::from_secs(1),
        show_progress: false,
    };
    let mut err = Vec::new();
    let failure = wait_for_status_rx(&mut rx, &service, wait, &mut err).expect_err("port");
    assert_eq!(failure.code, "DBL-1005");
}

#[test]
fn wait_for_status_rx_reports_a_closed_stream() {
    let service = StaticService {
        status: InstanceStatus::Starting,
    };
    let id = instance_id();
    let op = op();
    let (tx, mut rx) = tokio::sync::broadcast::channel(2);
    drop(tx);
    let wait = StatusWait {
        op: &op,
        instance_id: &id,
        ok_statuses: &[InstanceStatus::Running],
        timeout: Duration::from_secs(1),
        show_progress: false,
    };
    let mut err = Vec::new();
    let failure = wait_for_status_rx(&mut rx, &service, wait, &mut err).expect_err("closed");
    assert_eq!(failure.code, "DBL-3003");
}

/// A burst that drops the final `InstanceChanged` must not end as a spurious
/// timeout: on `Lagged` the status is re-read from the store. The channel holds
/// one event, the service says `Running`, and the only remaining message is
/// irrelevant, so ignoring `Lagged` would block until `Closed`.
#[test]
fn wait_for_status_rx_recovers_a_dropped_final_event_after_a_burst() {
    let service = FakeService::new(InstanceStatus::Running, false);
    let id = service.id();
    let status = with_deadline(Duration::from_secs(5), move || {
        let (tx, mut rx) = tokio::sync::broadcast::channel(1);
        tx.send(ServiceEvent::CatalogUpdated {
            status: dblocal_core::model::CatalogStatus::Fresh,
        })
        .expect("send");
        tx.send(ServiceEvent::LogLines {
            id: instance_id(),
            lines: Vec::new(),
        })
        .expect("send");
        drop(tx);
        let op = op();
        let wait = StatusWait {
            op: &op,
            instance_id: &id,
            ok_statuses: &[InstanceStatus::Running],
            timeout: Duration::from_secs(3),
            show_progress: false,
        };
        let mut err = Vec::new();
        wait_for_status_rx(&mut rx, &service, wait, &mut err)
    });
    assert_eq!(
        status.expect("running from the store"),
        InstanceStatus::Running
    );
}

#[test]
fn status_label_covers_every_variant() {
    for status in [
        InstanceStatus::Draft,
        InstanceStatus::Provisioning,
        InstanceStatus::Initializing,
        InstanceStatus::Stopped,
        InstanceStatus::Starting,
        InstanceStatus::Running,
        InstanceStatus::Stopping,
        InstanceStatus::Failed,
        InstanceStatus::Deleting,
    ] {
        assert!(!status_label(status).is_empty());
    }
}

#[test]
fn table_widths_follow_the_longest_cell() {
    let views = FakeService::new(InstanceStatus::Running, false).views();
    let table = format_list_table(&views);
    assert!(table.contains("output-app"));
    assert!(table.contains("running"));
    assert!(!table.ends_with('\n'), "caller adds the newline");
}

#[test]
fn report_error_writes_the_diagnostic_and_returns_the_code() {
    let mut err = Vec::new();
    let code = report_error(
        &mut err,
        &dblocal_core::DblError::Engine { reason: "x".into() },
    );
    assert_eq!(code, EXIT_ENGINE_FAILED);
    assert!(String::from_utf8_lossy(&err).contains("DBL-4001"));
}

#[test]
fn service_open_error_is_never_empty() {
    let text = crate::messages::service_open_error("detail");
    assert!(text.contains("detail"));
}

#[test]
fn message_helpers_carry_their_codes() {
    assert!(crate::messages::timeout_line().contains(crate::messages::CODE_TIMEOUT));
    assert!(crate::messages::runtime_line("x").contains(crate::messages::CODE_RUNTIME));
    assert!(
        crate::messages::stopping_elsewhere_line()
            .contains(dblocal_core::error::CODE_INVALID_TRANSITION)
    );
    assert_eq!(
        crate::messages::diagnosis_line("status"),
        "diagnosis: status"
    );
    assert_eq!(crate::messages::code_line("DBL-1002", "x"), "[DBL-1002] x");
    assert_eq!(
        crate::messages::download_progress(50, 5, 10),
        "... unduhan 50% (5/10)"
    );
    assert_eq!(crate::messages::stage_line("starting"), "... starting");
}
