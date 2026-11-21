#![cfg(test)]

use dblocal_core::model::{
    AuthMode, EngineKey, InstanceId, InstanceName, InstanceSpec, InstanceStatus, Port,
};
use dblocal_core::service::mock::MockService;
use dblocal_core::service::{DblService, UriFormat};

use crate::cli::{Command, EXIT_BAD_ARGS, EXIT_GENERAL, EXIT_NOT_FOUND, EXIT_OK};
use crate::commands::{Ctx, dispatch};
use crate::output;

fn spec(name: &str, engine: EngineKey, version: &str, port: u16) -> InstanceSpec {
    InstanceSpec::new(
        InstanceName::new(name).expect("name"),
        engine,
        version,
        Some(Port::new(port).expect("port")),
        AuthMode::Password,
    )
    .expect("spec")
}

fn run_cmd(service: &dyn DblService, cmd: &Command, interactive: bool) -> (u8, String, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut ctx = Ctx {
        service,
        out: &mut out,
        err: &mut err,
        is_interactive: interactive,
        stdout_is_tty: false,
        stderr_is_tty: false,
    };
    let code = dispatch(&mut ctx, cmd);
    (
        code,
        String::from_utf8_lossy(&out).to_string(),
        String::from_utf8_lossy(&err).to_string(),
    )
}

fn assert_no_secret(code: u8, out: &str, err: &str, secret: &str) {
    assert!(
        !out.contains(secret),
        "stdout leaked secret on exit {code}: {out}"
    );
    assert!(
        !err.contains(secret),
        "stderr leaked secret on exit {code}: {err}"
    );
}

#[test]
fn list_empty_table() {
    let svc = MockService::deterministic();
    let (code, out, err) = run_cmd(&svc, &Command::List { json: false }, false);
    assert_eq!(code, EXIT_OK);
    assert!(out.contains("NAME"));
    assert!(err.is_empty());
}

#[test]
fn list_empty_json() {
    let svc = MockService::deterministic();
    let (code, out, _) = run_cmd(&svc, &Command::List { json: true }, false);
    assert_eq!(code, EXIT_OK);
    assert_eq!(out.trim(), "[]");
}

#[test]
fn list_json_has_stable_keys() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let (code, out, _) = run_cmd(&svc, &Command::List { json: true }, false);
    assert_eq!(code, EXIT_OK);
    let v: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
    let arr = v.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    let obj = arr[0].as_object().expect("obj");
    for k in ["name", "engine", "version", "port", "status"] {
        assert!(obj.contains_key(k), "missing {k}");
    }
    assert_eq!(obj["name"], "app");
    assert_eq!(obj["engine"], "postgresql");
    assert_eq!(obj["status"], "running");
}

#[test]
fn status_missing_is_exit_3() {
    let svc = MockService::deterministic();
    let (code, _, err) = run_cmd(
        &svc,
        &Command::Status {
            name: "nope".into(),
        },
        false,
    );
    assert_eq!(code, EXIT_NOT_FOUND);
    assert!(err.contains("DBL-1002"));
}

#[test]
fn status_running_exit_0() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let (code, out, _) = run_cmd(&svc, &Command::Status { name: "app".into() }, false);
    assert_eq!(code, EXIT_OK);
    assert_eq!(out.trim(), "running");
}

#[test]
fn status_stopped_exit_1() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let (code, out, _) = run_cmd(&svc, &Command::Status { name: "app".into() }, false);
    assert_eq!(code, EXIT_GENERAL);
    assert_eq!(out.trim(), "stopped");
}

#[test]
fn uri_prints_secret_only_on_stdout() {
    let svc = MockService::deterministic();
    let id = svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let uri = svc.connection_uri(&id, UriFormat::Uri).expect("uri");
    let secret = uri.expose().to_owned();
    assert!(
        secret.contains("pwd-") || secret.contains(":"),
        "uri should contain credential"
    );
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Uri {
            name: "app".into(),
            env: false,
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    assert!(out.trim().contains(secret.as_str()));
    assert!(!err.contains(&secret), "stderr must not contain uri secret");
}

#[test]
fn non_uri_commands_never_print_password() {
    let svc = MockService::deterministic();
    let id = svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let uri = svc.connection_uri(&id, UriFormat::Uri).expect("uri");
    let secret = uri.expose().to_owned();
    // extract password portion roughly
    let pwd = secret
        .split(':')
        .nth(2)
        .unwrap_or("")
        .split('@')
        .next()
        .unwrap_or("");
    assert!(!pwd.is_empty(), "expected password in uri");

    let cmds = vec![
        Command::List { json: false },
        Command::List { json: true },
        Command::Status { name: "app".into() },
        Command::Logs {
            name: "app".into(),
            follow: false,
            lines: 50,
        },
    ];
    for cmd in cmds {
        let (code, out, err) = run_cmd(&svc, &cmd, false);
        assert_no_secret(code, &out, &err, pwd);
    }
}

#[test]
fn delete_refuses_without_yes_when_not_interactive() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Delete {
            name: "app".into(),
            yes: false,
            purge: false,
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert!(err.contains("--yes"));
    // instance still exists
    assert_eq!(svc.list_instances().unwrap().len(), 1);
}

#[test]
fn uri_env_uses_dotenv_format() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Uri {
            name: "app".into(),
            env: true,
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    assert!(out.contains("DATABASE_URL=") || out.contains("DB_HOST="));
    assert!(!err.contains("DATABASE_URL=")); // stderr should be clean
}

#[test]
fn start_already_running_prints_sudah_berjalan() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Start {
            name: "app".into(),
            timeout: 5,
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    assert!(err.contains("sudah berjalan"));
}

#[test]
fn stop_already_stopped_is_ok() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let (code, _out, err) = run_cmd(&svc, &Command::Stop { name: "app".into() }, false);
    assert_eq!(code, EXIT_OK);
    assert!(err.contains("sudah berhenti"));
}

#[test]
fn delete_with_yes_purges() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Delete {
            name: "app".into(),
            yes: true,
            purge: true,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "err={err}");
    assert!(svc.list_instances().unwrap().is_empty());
}

#[test]
fn delete_with_yes_archives_reports_path() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Delete {
            name: "app".into(),
            yes: true,
            purge: false,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "err={err}");
    assert!(err.contains("Data diarsipkan ke") || err.to_lowercase().contains("arsip"));
}

#[test]
fn logs_returns_lines() {
    let svc = MockService::deterministic();
    let id = svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let _ = id;
    let (code, out, _) = run_cmd(
        &svc,
        &Command::Logs {
            name: "app".into(),
            follow: false,
            lines: 200,
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    // mock seeds at least one log line
    assert!(!out.trim().is_empty());
}

#[test]
fn formatters_table_and_json_golden() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let views = svc.list_instances().unwrap();
    let table = output::format_list_table(&views, false);
    assert!(table.contains("NAME"));
    assert!(table.contains("ENGINE"));
    assert!(table.contains("app"));
    assert!(table.contains("postgresql"));
    assert!(table.contains("5432"));
    assert!(table.contains("running"));

    let json = output::format_list_json(&views).unwrap();
    assert_eq!(
        json,
        "[{\"name\":\"app\",\"engine\":\"postgresql\",\"version\":\"17.4\",\"port\":5432,\"status\":\"running\"}]"
    );
}

#[test]
fn exit_code_table_maps_codes() {
    use dblocal_core::DblError;
    assert_eq!(
        output::code_for(&DblError::NotFound {
            kind: "instance",
            name: "x".into()
        }),
        3
    );
    assert_eq!(
        output::code_for(&DblError::Engine { reason: "x".into() }),
        4
    );
    assert_eq!(
        output::code_for(&DblError::InvalidInput {
            field: "x",
            reason: "y".into()
        }),
        2
    );
    assert_eq!(
        output::code_for(&DblError::InvalidTransition { from: "a", to: "b" }),
        2
    );
    assert_eq!(
        output::code_for(&DblError::Storage { reason: "x".into() }),
        1
    );
}

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
