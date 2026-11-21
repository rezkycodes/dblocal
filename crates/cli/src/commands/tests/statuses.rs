//! The `(command, concrete status) -> (exit, message)` table from `cli.md`.
//!
//! Every row is driven by a service that scripts one status per read, with its
//! own deadline: a command that ignores `--timeout` must fail as a red test, not
//! hang the test binary.

use super::*;

/// One scripted answer per read, plus a chosen transition error, so every
/// `(command, concrete status)` row of the table in `cli.md` is reachable.
///
/// `statuses` is consumed one entry per `list_instances` call; the last entry
/// repeats. `refuse` is what `start_instance`/`stop_instance` answer instead of
/// the mock's behaviour.
struct StatusScript {
    inner: MockService,
    target: InstanceId,
    statuses: Vec<InstanceStatus>,
    polls: std::sync::Mutex<usize>,
    refuse: Option<(&'static str, bool)>,
}

impl StatusScript {
    fn new(
        status: InstanceStatus,
        statuses: Vec<InstanceStatus>,
        refuse: Option<(&'static str, bool)>,
    ) -> Self {
        let inner = MockService::deterministic();
        let target =
            inner.seed_instance(spec("raced", EngineKey::Postgresql, "17.4", 5432), status);
        Self {
            inner,
            target,
            statuses,
            polls: std::sync::Mutex::new(0),
            refuse,
        }
    }

    fn transition(
        &self,
        id: &InstanceId,
    ) -> Option<dblocal_core::Result<dblocal_core::service::OperationId>> {
        self.refuse.map(|(from, _)| {
            let to = if from == "stopping" {
                "stopping"
            } else {
                "starting"
            };
            let _ = id;
            Err(dblocal_core::DblError::InvalidTransition { from, to })
        })
    }
}

impl DblService for StatusScript {
    fn list_instances(&self) -> dblocal_core::Result<Vec<dblocal_core::service::InstanceView>> {
        let mut views = self.inner.list_instances()?;
        let mut polls = self.polls.lock().expect("polls");
        let index = (*polls).min(self.statuses.len().saturating_sub(1));
        *polls += 1;
        let status = self.statuses[index];
        for view in &mut views {
            if view.id == self.target {
                view.status = status;
            }
        }
        Ok(views)
    }
    fn catalog(&self) -> dblocal_core::Result<dblocal_core::service::CatalogView> {
        self.inner.catalog()
    }
    fn refresh_catalog(
        &self,
        force: bool,
    ) -> dblocal_core::Result<dblocal_core::service::OperationId> {
        self.inner.refresh_catalog(force)
    }
    fn validate_spec(&self, spec: &InstanceSpec) -> dblocal_core::Result<()> {
        self.inner.validate_spec(spec)
    }
    fn suggest_port(&self, engine: EngineKey) -> dblocal_core::Result<Port> {
        self.inner.suggest_port(engine)
    }
    fn create_instance(
        &self,
        spec: InstanceSpec,
    ) -> dblocal_core::Result<dblocal_core::service::OperationId> {
        self.inner.create_instance(spec)
    }
    fn start_instance(
        &self,
        id: &InstanceId,
    ) -> dblocal_core::Result<dblocal_core::service::OperationId> {
        match self.transition(id) {
            Some(result) => result,
            None => self.inner.start_instance(id),
        }
    }
    fn stop_instance(
        &self,
        id: &InstanceId,
    ) -> dblocal_core::Result<dblocal_core::service::OperationId> {
        match self.transition(id) {
            Some(result) => result,
            None => self.inner.stop_instance(id),
        }
    }
    fn delete_instance(
        &self,
        id: &InstanceId,
        mode: dblocal_core::model::DeleteMode,
    ) -> dblocal_core::Result<dblocal_core::service::OperationId> {
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
    fn cancel_operation(
        &self,
        id: &dblocal_core::service::OperationId,
    ) -> dblocal_core::Result<()> {
        self.inner.cancel_operation(id)
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
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<dblocal_core::service::ServiceEvent> {
        self.inner.subscribe()
    }
}

/// `start` on an instance another process is *stopping* must not wait: it has
/// no winner to wait for, so it says so and exits 1 immediately.
#[test]
fn start_of_a_stopping_instance_says_stopping_and_exits_1() {
    let service = std::sync::Arc::new(StatusScript::new(
        InstanceStatus::Stopping,
        vec![InstanceStatus::Stopping],
        None,
    ));
    let (code, err) = within(
        std::time::Duration::from_secs(5),
        service,
        Command::Start {
            name: "raced".into(),
            timeout: 60,
        },
    );
    assert_eq!(code, EXIT_GENERAL, "err: {err}");
    assert!(
        err.contains("sedang dihentikan oleh proses lain"),
        "err: {err}"
    );
}

/// The same when the concrete `stopping` is only visible through the refused
/// compare-and-set (the row moved between our read and our write).
#[test]
fn start_refused_from_stopping_says_stopping_and_exits_1() {
    let service = std::sync::Arc::new(StatusScript::new(
        InstanceStatus::Stopped,
        vec![InstanceStatus::Stopped],
        Some(("stopping", true)),
    ));
    // Under the mutation this becomes a cross-process wait; the deadline makes
    // that a red assertion in seconds instead of a 60 s `--timeout`.
    let (code, err) = within(
        std::time::Duration::from_secs(5),
        service,
        Command::Start {
            name: "raced".into(),
            timeout: 60,
        },
    );
    assert_eq!(code, EXIT_GENERAL, "err: {err}");
    assert!(
        err.contains("sedang dihentikan oleh proses lain"),
        "err: {err}"
    );
    assert!(!err.contains("sudah berjalan"), "err: {err}");
}

/// `stop` when another process is already stopping it: idempotent success.
#[test]
fn stop_of_a_stopping_instance_waits_then_exits_0() {
    let service = StatusScript::new(
        InstanceStatus::Stopping,
        vec![
            InstanceStatus::Stopping,
            InstanceStatus::Stopping,
            InstanceStatus::Stopped,
        ],
        None,
    );
    let (_code, out, err) = run_cmd(
        &service,
        &Command::Stop {
            name: "raced".into(),
        },
        false,
    );
    assert!(
        err.contains("sedang dihentikan oleh proses lain"),
        "err: {err}"
    );
    assert_eq!(_code, EXIT_OK, "err: {err}");
    assert!(out.is_empty(), "out: {out}");
}

/// `stop` refused because the row became `stopped` under us: success, not 2.
#[test]
fn stop_refused_from_stopped_is_idempotent_success() {
    let service = StatusScript::new(
        InstanceStatus::Running,
        vec![InstanceStatus::Running, InstanceStatus::Stopped],
        Some(("stopped", false)),
    );
    let (code, _out, err) = run_cmd(
        &service,
        &Command::Stop {
            name: "raced".into(),
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "err: {err}");
    assert!(err.contains("sudah berhenti"), "err: {err}");
}
