//! The F-CLI-02 AC3 start race and the cross-process wait.

use super::*;

/// A service wrapper that models the F-CLI-02 AC3 race: a second process loses
/// the compare-and-set, then observes the winner's `running` state.
struct RacyService {
    inner: MockService,
    target: InstanceId,
    polls: std::sync::Mutex<usize>,
}

impl DblService for RacyService {
    fn list_instances(&self) -> dblocal_core::Result<Vec<dblocal_core::service::InstanceView>> {
        let mut polls = self.polls.lock().expect("polls");
        *polls += 1;
        let running = *polls > 1;
        let mut views = self.inner.list_instances()?;
        for view in &mut views {
            if view.id == self.target {
                view.status = if running {
                    InstanceStatus::Running
                } else {
                    InstanceStatus::Starting
                };
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
        _id: &InstanceId,
    ) -> dblocal_core::Result<dblocal_core::service::OperationId> {
        Err(dblocal_core::DblError::InvalidTransition {
            from: "busy",
            to: "starting",
        })
    }
    fn stop_instance(
        &self,
        id: &InstanceId,
    ) -> dblocal_core::Result<dblocal_core::service::OperationId> {
        self.inner.stop_instance(id)
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

#[test]
fn start_losing_the_race_prints_sudah_berjalan_and_exits_0() {
    let inner = MockService::deterministic();
    let id = inner.seed_instance(
        spec("raced", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let service = RacyService {
        inner,
        target: id,
        polls: std::sync::Mutex::new(0),
    };
    let (code, _out, err) = run_cmd(
        &service,
        &Command::Start {
            name: "raced".into(),
            timeout: 5,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "err={err}");
    assert!(err.contains("sudah berjalan"), "err={err}");
}

#[test]
fn start_of_an_instance_another_process_is_starting_says_sudah_berjalan() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("busy", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Starting,
    );
    // Nothing advances the mock, so the bounded cross-process wait times out;
    // the point is that it never asks for a second start and says so first.
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Start {
            name: "busy".into(),
            timeout: 1,
        },
        false,
    );
    assert!(err.contains("sudah berjalan"), "err={err}");
    assert_ne!(code, EXIT_OK);
}

/// A service that loses the start race with `busy` and then reports a scripted
/// end state, so the cross-process wait paths are reachable.
struct CrossProcess {
    inner: MockService,
    target: InstanceId,
    scripted: &'static str,
    /// How often `list_instances` has answered; the first answer is `Starting`
    /// (what the racing process sees), later ones are the scripted end state.
    polls: std::sync::Mutex<usize>,
}

impl DblService for CrossProcess {
    fn list_instances(&self) -> dblocal_core::Result<Vec<dblocal_core::service::InstanceView>> {
        let mut views = self.inner.list_instances()?;
        let mut polls = self.polls.lock().expect("polls");
        *polls += 1;
        let settled = *polls > 1;
        if !settled {
            for view in &mut views {
                if view.id == self.target {
                    view.status = InstanceStatus::Starting;
                }
            }
            return Ok(views);
        }
        match self.scripted {
            "gone" => views.retain(|v| v.id != self.target),
            "failed" => {
                for view in &mut views {
                    if view.id == self.target {
                        view.status = InstanceStatus::Failed;
                        view.failure = Some(dblocal_core::service::FailureInfo {
                            code: "DBL-4001".to_owned(),
                            message: "engine failed".to_owned(),
                            diagnosis: dblocal_core::model::StartFailure::PortInUse,
                            log_tail: Vec::new(),
                        });
                    }
                }
            }
            _ => {
                for view in &mut views {
                    if view.id == self.target {
                        view.status = InstanceStatus::Starting;
                    }
                }
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
        _id: &InstanceId,
    ) -> dblocal_core::Result<dblocal_core::service::OperationId> {
        // The loser of the compare-and-set: the row was claimed by someone else.
        if self.scripted == "stopped" {
            Err(dblocal_core::DblError::InvalidTransition {
                from: "busy",
                to: "starting",
            })
        } else {
            Ok(dblocal_core::service::OperationId::new("op-unused"))
        }
    }
    fn stop_instance(
        &self,
        id: &InstanceId,
    ) -> dblocal_core::Result<dblocal_core::service::OperationId> {
        self.inner.stop_instance(id)
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

fn cross_process(scripted: &'static str, status: InstanceStatus) -> CrossProcess {
    let inner = MockService::deterministic();
    let target = inner.seed_instance(spec("raced", EngineKey::Postgresql, "17.4", 5432), status);
    CrossProcess {
        inner,
        target,
        scripted,
        polls: std::sync::Mutex::new(0),
    }
}

#[test]
fn start_busy_loser_reports_sudah_berjalan() {
    let service = cross_process("stopped", InstanceStatus::Stopped);
    let (code, _o, err) = run_cmd(
        &service,
        &Command::Start {
            name: "raced".into(),
            timeout: 1,
        },
        false,
    );
    assert!(err.contains("sudah berjalan"), "err: {err}");
    assert_ne!(code, EXIT_OK);
}

#[test]
fn cross_process_wait_reports_a_failed_winner() {
    let service = cross_process("failed", InstanceStatus::Starting);
    let (code, _o, err) = run_cmd(
        &service,
        &Command::Start {
            name: "raced".into(),
            timeout: 5,
        },
        false,
    );
    assert_eq!(code, 4, "err: {err}");
    assert!(err.contains("DBL-4001"), "err: {err}");
    assert!(err.contains("diagnosis:"), "err: {err}");
}

#[test]
fn cross_process_wait_reports_a_vanished_instance() {
    let service = cross_process("gone", InstanceStatus::Starting);
    let (code, _o, _err) = run_cmd(
        &service,
        &Command::Start {
            name: "raced".into(),
            timeout: 1,
        },
        false,
    );
    assert_eq!(code, EXIT_NOT_FOUND);
}

/// A service that reports the row already `Starting` when the start is refused,
/// i.e. the loser read the row just after the winner persisted it.
struct AlreadyClaimed {
    inner: MockService,
    target: InstanceId,
    polls: std::sync::Mutex<usize>,
}

impl DblService for AlreadyClaimed {
    fn list_instances(&self) -> dblocal_core::Result<Vec<dblocal_core::service::InstanceView>> {
        let mut views = self.inner.list_instances()?;
        let mut polls = self.polls.lock().expect("polls");
        *polls += 1;
        for view in &mut views {
            if view.id == self.target {
                view.status = if *polls == 1 {
                    InstanceStatus::Stopped
                } else {
                    InstanceStatus::Running
                };
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
        _id: &InstanceId,
    ) -> dblocal_core::Result<dblocal_core::service::OperationId> {
        // What the real service returns when the row already says `Starting`.
        Err(dblocal_core::DblError::InvalidTransition {
            from: "starting",
            to: "starting",
        })
    }
    fn stop_instance(
        &self,
        id: &InstanceId,
    ) -> dblocal_core::Result<dblocal_core::service::OperationId> {
        self.inner.stop_instance(id)
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

#[test]
fn start_refused_with_a_concrete_process_status_is_sudah_berjalan() {
    let inner = MockService::deterministic();
    let target = inner.seed_instance(
        spec("claimed", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let service = AlreadyClaimed {
        inner,
        target,
        polls: std::sync::Mutex::new(0),
    };
    let (code, _o, err) = run_cmd(
        &service,
        &Command::Start {
            name: "claimed".into(),
            timeout: 5,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "err: {err}");
    assert!(err.contains("sudah berjalan"), "err: {err}");
}

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

/// Runs `start`/`stop` on its own thread and fails the test if it has not
/// finished within `limit`, so a command that waits for a timeout is a red test
/// rather than a hang of the whole test binary.
fn within(
    limit: std::time::Duration,
    service: std::sync::Arc<StatusScript>,
    cmd: Command,
) -> (u8, String) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = run_cmd(service.as_ref(), &cmd, false);
        let _ = tx.send((result.0, result.2));
    });
    match rx.recv_timeout(limit) {
        Ok(result) => result,
        Err(_) => panic!("the command did not return within {limit:?}; a timeout was ignored"),
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
    let service = StatusScript::new(
        InstanceStatus::Stopped,
        vec![InstanceStatus::Stopped],
        Some(("stopping", true)),
    );
    let (code, out, err) = run_cmd(
        &service,
        &Command::Start {
            name: "raced".into(),
            timeout: 60,
        },
        false,
    );
    assert_eq!(code, EXIT_GENERAL, "err: {err}");
    assert!(
        err.contains("sedang dihentikan oleh proses lain"),
        "err: {err}"
    );
    assert!(!err.contains("sudah berjalan"), "err: {err}");
    assert!(out.is_empty(), "out: {out}");
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
