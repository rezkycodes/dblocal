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
