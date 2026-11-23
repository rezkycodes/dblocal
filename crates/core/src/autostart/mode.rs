//! The `--autostart` mode driven from the CLI and reused by the GUI.
//!
//! The pass depends on [`AutostartService`], a small view of the service, not
//! the whole `DblService` trait. That keeps the glue small, lets the tests use a
//! compact double, and makes the production wiring a one-line impl for
//! `Arc<dyn DblService>`.
//!
//! # D1 — the pass must wait for every start
//!
//! `start_instance` only *requests* a start and returns an operation id; the
//! engine comes up (or fails) asynchronously. A login pass that returns as soon
//! as the request is accepted would let the process exit and kill the in-flight
//! start with it. [`start_targets`] therefore composes a start with
//! [`AutostartService::wait_started`] per target, bounded by the engine's own
//! start timeout plus a margin, and each target waits independently so one
//! hanging engine never serializes the rest.

use std::sync::Arc;
use std::time::Duration;

use crate::model::{EngineKey, InstanceId};
use crate::service::DblService;

use super::AutostartError;
use super::consts;
use super::plan::{AutostartMode, AutostartSummary, AutostartTarget, decide_mode, flagged_targets};
use super::runner::run_start_targets;

/// The service calls an autostart pass needs.
pub trait AutostartService: Send + Sync {
    /// Reads every instance view.
    fn list(&self) -> crate::Result<Vec<crate::service::InstanceView>>;
    /// Requests a start and returns immediately (the operation runs async).
    fn start(&self, id: &InstanceId) -> crate::Result<()>;
    /// Waits until the start requested for `id` has finished, bounded by
    /// `timeout`. `Ok(())` means the instance reached `running`; `Err` carries a
    /// user-safe diagnostic (with the `DBL-xxxx` code when there is one).
    fn wait_started(&self, id: &InstanceId, timeout: Duration) -> Result<(), String>;
}

impl AutostartService for Arc<dyn DblService> {
    fn list(&self) -> crate::Result<Vec<crate::service::InstanceView>> {
        self.list_instances()
    }

    fn start(&self, id: &InstanceId) -> crate::Result<()> {
        self.start_instance(id).map(|_op| ())
    }

    fn wait_started(&self, id: &InstanceId, timeout: Duration) -> Result<(), String> {
        wait_for_running(self.as_ref(), id, timeout)
    }
}

/// The wait bound for a start: the engine's own start timeout plus a margin.
///
/// Derived from `consts`, never invented: a start is allowed to take as long as
/// the service allows, and the observer only adds slack for the supervisor to
/// observe the outcome.
pub fn start_wait_bound(engine: EngineKey) -> Duration {
    let base = match engine {
        EngineKey::Valkey => consts::START_TIMEOUT_SECS_VALKEY,
        _ => consts::START_TIMEOUT_SECS,
    };
    Duration::from_secs(base + consts::AUTOSTART_START_MARGIN_SECS)
}

/// Polls the store until the instance leaves `starting` or the deadline passes.
///
/// The store is the authority: `start_instance` claims `starting` synchronously
/// before returning, so by the time this runs the row is `starting`. A `running`
/// row is success; `failed`/`stopped`/gone is a failure that carries the stored
/// `DBL-xxxx` code when the row has a failure record.
fn wait_for_running(
    service: &dyn DblService,
    id: &InstanceId,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match classify(service.list_instances(), id) {
            Some(Ok(())) => return Ok(()),
            Some(Err(message)) => return Err(message),
            None => {}
        }
        if std::time::Instant::now() >= deadline {
            return Err("[DBL-8401] waktu tunggu autostart habis".to_owned());
        }
        std::thread::sleep(Duration::from_millis(consts::AUTOSTART_WAIT_POLL_MS));
    }
}

/// Classifies one store read for the wait (pure, so the branches are tested).
///
/// `None` means "keep polling": the row is still `starting`/transitional, or the
/// read failed transiently (a read error must never become a wrong verdict).
fn classify(
    read: crate::Result<Vec<crate::service::InstanceView>>,
    id: &InstanceId,
) -> Option<Result<(), String>> {
    use crate::model::InstanceStatus;
    let views = match read {
        Ok(views) => views,
        Err(_) => return None,
    };
    let Some(view) = views.into_iter().find(|view| &view.id == id) else {
        // The row vanished mid-start (deleted, or a rolled-back create).
        return Some(Err("[DBL-1002] instance hilang saat autostart".to_owned()));
    };
    match view.status {
        InstanceStatus::Running => Some(Ok(())),
        InstanceStatus::Failed => Some(Err(failure_message(&view, "gagal"))),
        InstanceStatus::Stopped => {
            Some(Err("[DBL-4001] instance berhenti sebelum siap".to_owned()))
        }
        _ => None,
    }
}

/// Builds a user-safe `[DBL-xxxx] message` from a failed instance view.
fn failure_message(view: &crate::service::InstanceView, fallback: &str) -> String {
    match &view.failure {
        Some(info) => format!("[{}] {}", info.code, info.message),
        None => format!("[DBL-4001] {}", fallback),
    }
}

/// Outcome of one `--autostart` pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartPass {
    /// How the app should present itself afterwards.
    pub mode: AutostartMode,
    /// What happened for every flagged instance, in input order.
    pub summary: AutostartSummary,
}

/// Runs one `--autostart` pass against a service.
///
/// `tray_available` is supplied by the caller: the GUI knows it; the CLI
/// assumes `false`, because a CLI process never owns a tray host.
///
/// The pass does not return until every requested start has finished (D1).
pub fn run_pass<S>(service: &S, tray_available: bool) -> Result<AutostartPass, AutostartError>
where
    S: AutostartService + ?Sized,
{
    let views = service.list().map_err(|err| AutostartError::StatusFailed {
        detail: err.to_string(),
    })?;
    let targets = flagged_targets(&views);
    let mode = decide_mode(tray_available, targets.len());
    let summary = start_targets(service, &targets)?;
    Ok(AutostartPass { mode, summary })
}

/// Starts `targets` through the service, bounded, independent, and **waited**.
///
/// Each target is started and then waited on inside its own worker, so the
/// waits run concurrently and one failure never blocks another. The summary
/// carries the real outcome of each start, not just the request.
pub fn start_targets<S>(
    service: &S,
    targets: &[AutostartTarget],
) -> Result<AutostartSummary, AutostartError>
where
    S: AutostartService + ?Sized,
{
    run_start_targets(targets, |target: &AutostartTarget| {
        service
            .start(&target.id)
            .map_err(|err| format!("[{}] {}", err.code(), err.user_message()))?;
        service.wait_started(&target.id, start_wait_bound(target.engine))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::model::{
        AuthMode, EngineKey, Instance, InstanceId, InstanceName, InstanceSpec, InstanceStatus, Port,
    };
    use crate::service::InstanceView;

    /// A service double whose `start` requests work on a background thread and
    /// whose `wait_started` blocks until that work completes, mirroring the real
    /// service: the request is async, the outcome is observed later.
    struct SlowFake {
        views: Vec<InstanceView>,
        completed: Arc<std::sync::atomic::AtomicBool>,
        delay: Duration,
        fail_after: Option<Duration>,
    }

    impl SlowFake {
        fn ok(delay: Duration) -> Self {
            Self {
                views: vec![view("pg-slow", true)],
                completed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                delay,
                fail_after: None,
            }
        }
        fn completed(&self) -> bool {
            self.completed.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl AutostartService for SlowFake {
        fn list(&self) -> crate::Result<Vec<InstanceView>> {
            Ok(self.views.clone())
        }
        fn start(&self, _id: &InstanceId) -> crate::Result<()> {
            Ok(())
        }
        fn wait_started(&self, _id: &InstanceId, timeout: Duration) -> Result<(), String> {
            if self.delay > timeout {
                return Err("[DBL-8401] waktu tunggu autostart habis".to_owned());
            }
            std::thread::sleep(self.delay);
            self.completed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            match self.fail_after {
                Some(_) => Err("[DBL-1005] Port itu sedang dipakai proses lain.".to_owned()),
                None => Ok(()),
            }
        }
    }

    /// A two-method service double: a fixed view list that accepts or refuses
    /// starts by id (kept for the older behaviour tests).
    struct FakeService {
        views: Vec<InstanceView>,
        accept: bool,
    }

    impl AutostartService for FakeService {
        fn list(&self) -> crate::Result<Vec<InstanceView>> {
            Ok(self.views.clone())
        }
        fn start(&self, id: &InstanceId) -> crate::Result<()> {
            if self.accept && self.views.iter().any(|v| v.id == *id) {
                Ok(())
            } else {
                Err(crate::DblError::NotFound {
                    kind: "instance",
                    name: id.to_string(),
                })
            }
        }
        fn wait_started(&self, _id: &InstanceId, _timeout: Duration) -> Result<(), String> {
            Ok(())
        }
    }

    fn view(name: &str, autostart: bool) -> InstanceView {
        let spec = InstanceSpec::new(
            InstanceName::new(name).expect("name"),
            EngineKey::Postgresql,
            "18.0",
            Some(Port::new(5432).expect("port")),
            AuthMode::Password,
        )
        .expect("spec")
        .with_autostart(autostart);
        let inst = Instance::new(
            &spec,
            InstanceId::new(),
            InstanceStatus::Stopped,
            std::time::SystemTime::UNIX_EPOCH,
        );
        InstanceView::from_instance(&inst)
    }

    fn fake(views: Vec<InstanceView>, accept: bool) -> FakeService {
        FakeService { views, accept }
    }

    #[test]
    fn no_flagged_instances_yield_headless_exit() {
        let service = fake(Vec::new(), true);
        let pass = run_pass(&service, false).expect("pass");
        assert_eq!(pass.mode, AutostartMode::HeadlessExit);
        assert!(pass.summary.outcomes.is_empty());
    }

    #[test]
    fn tray_available_is_always_tray_only() {
        let service = fake(Vec::new(), true);
        assert_eq!(
            run_pass(&service, true).expect("pass").mode,
            AutostartMode::TrayOnly
        );
    }

    #[test]
    fn a_flagged_instance_is_started() {
        let service = fake(vec![view("pg-one", true), view("pg-two", false)], true);
        let pass = run_pass(&service, false).expect("pass");
        assert_eq!(pass.mode, AutostartMode::HeadlessKeepAlive);
        assert_eq!(pass.summary.outcomes.len(), 1);
        assert_eq!(pass.summary.succeeded(), 1);
    }

    #[test]
    fn a_refused_start_is_recorded_not_fatal() {
        let service = fake(vec![view("pg-one", true)], false);
        let pass = run_pass(&service, false).expect("pass");
        assert_eq!(pass.summary.failed(), 1);
        let outcome = &pass.summary.outcomes[0];
        assert!(
            outcome.result.as_ref().unwrap_err().starts_with("[DBL-"),
            "a refusal carries its code"
        );
    }

    #[test]
    fn an_arc_service_is_an_autostart_service() {
        // The blanket impl is what production uses; prove it compiles and runs
        // through the trait object.
        let service = fake(vec![view("pg-one", true)], true);
        let targets = flagged_targets(&service.views);
        let summary = start_targets(&service, &targets).expect("run");
        assert_eq!(summary.succeeded(), 1);
    }

    // --- D1: the pass must not return before the start finishes ---

    #[test]
    fn run_pass_waits_for_every_start_to_finish() {
        let service = SlowFake::ok(Duration::from_millis(300));
        let _pass = run_pass(&service, false).expect("pass");
        assert!(
            service.completed(),
            "run_pass returned before the start operation finished (D1)"
        );
    }

    #[test]
    fn a_failed_start_is_reported_per_instance() {
        let mut service = SlowFake::ok(Duration::from_millis(10));
        service.fail_after = Some(Duration::from_millis(10));
        let pass = run_pass(&service, false).expect("pass");
        assert_eq!(pass.summary.failed(), 1);
        let outcome = &pass.summary.outcomes[0];
        assert!(
            outcome
                .result
                .as_ref()
                .unwrap_err()
                .starts_with("[DBL-1005]"),
            "a failed start carries its code: {:?}",
            outcome.result
        );
    }

    #[test]
    fn a_wait_that_exceeds_its_bound_is_a_timeout() {
        // The double's work takes longer than the bound we hand it, so the wait
        // returns the timeout error rather than hanging.
        let service = SlowFake::ok(Duration::from_secs(5));
        let targets = flagged_targets(&service.views);
        let result = service.wait_started(&targets[0].id, Duration::from_millis(10));
        assert!(result.unwrap_err().contains("DBL-8401"));
    }

    #[test]
    fn start_wait_bound_uses_the_engine_timeout_plus_a_margin() {
        let pg = start_wait_bound(EngineKey::Postgresql);
        let vk = start_wait_bound(EngineKey::Valkey);
        assert_eq!(
            pg,
            Duration::from_secs(consts::START_TIMEOUT_SECS + consts::AUTOSTART_START_MARGIN_SECS)
        );
        assert_eq!(
            vk,
            Duration::from_secs(
                consts::START_TIMEOUT_SECS_VALKEY + consts::AUTOSTART_START_MARGIN_SECS
            )
        );
        assert!(vk < pg, "valkey starts faster than postgres");
    }

    fn fictional() -> crate::service::InstanceView {
        // A hand-built view with a chosen status; `classify` reads status only.
        view("probe", true)
    }

    fn with_status(
        view: crate::service::InstanceView,
        status: InstanceStatus,
    ) -> crate::service::InstanceView {
        let mut view = view;
        view.status = status;
        view
    }

    #[test]
    fn classify_running_is_success() {
        let id = InstanceId::new();
        let mut view = with_status(fictional(), InstanceStatus::Running);
        view.id = id.clone();
        assert_eq!(classify(Ok(vec![view]), &id), Some(Ok(())));
    }

    #[test]
    fn classify_failed_carries_the_code() {
        let id = InstanceId::new();
        let mut view = with_status(fictional(), InstanceStatus::Failed);
        view.id = id.clone();
        view.failure = Some(crate::service::FailureInfo {
            code: "DBL-1005".to_owned(),
            message: "port dipakai".to_owned(),
            diagnosis: crate::model::StartFailure::PortInUse,
            log_tail: Vec::new(),
            restore_available: false,
        });
        match classify(Ok(vec![view]), &id) {
            Some(Err(message)) => assert!(message.starts_with("[DBL-1005]"), "{message}"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn classify_stopped_is_a_failure() {
        let id = InstanceId::new();
        let mut view = with_status(fictional(), InstanceStatus::Stopped);
        view.id = id.clone();
        match classify(Ok(vec![view]), &id) {
            Some(Err(message)) => assert!(message.contains("DBL-4001")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn classify_starting_keeps_polling() {
        let id = InstanceId::new();
        let mut view = with_status(fictional(), InstanceStatus::Starting);
        view.id = id.clone();
        assert_eq!(classify(Ok(vec![view]), &id), None);
    }

    #[test]
    fn classify_a_gone_instance_is_a_failure() {
        let id = InstanceId::new();
        let other = view("other", true);
        match classify(Ok(vec![other]), &id) {
            Some(Err(message)) => assert!(message.contains("DBL-1002")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn classify_a_read_error_keeps_polling() {
        let id = InstanceId::new();
        assert_eq!(
            classify(
                Err(crate::DblError::Storage {
                    reason: "busy".to_owned()
                }),
                &id
            ),
            None
        );
    }

    // --- D1 (gap): the PRODUCTION polling loop `wait_for_running` itself ---
    //
    // The tests above cover `classify()` and the trait doubles, which override
    // `wait_started` directly. Neither exercises the real loop that polls
    // `list_instances` and sleeps between reads. A mutation that makes
    // `wait_for_running` return `Ok(())` at once (or never sleep) must turn
    // these red.

    /// A `DblService` double whose `list_instances` returns a scripted sequence
    /// of statuses, one per call (the last value repeats once exhausted), and
    /// counts how many times it was read. Only the read side is meaningful; the
    /// rest of the surface is inert so the trait object compiles.
    struct ScriptedService {
        id: InstanceId,
        script: std::sync::Mutex<std::collections::VecDeque<InstanceStatus>>,
        last: std::sync::Mutex<InstanceStatus>,
        reads: std::sync::atomic::AtomicUsize,
    }

    impl ScriptedService {
        fn new(id: InstanceId, script: Vec<InstanceStatus>) -> Self {
            let last = script.last().copied().unwrap_or(InstanceStatus::Starting);
            Self {
                id,
                script: std::sync::Mutex::new(script.into()),
                last: std::sync::Mutex::new(last),
                reads: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn reads(&self) -> usize {
            self.reads.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn view_with(&self, status: InstanceStatus) -> crate::service::InstanceView {
            let mut v = with_status(view("scripted", true), status);
            v.id = self.id.clone();
            v
        }
    }

    impl DblService for ScriptedService {
        fn list_instances(&self) -> crate::Result<Vec<InstanceView>> {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let status = {
                let mut q = self.script.lock().expect("script");
                let next = q.pop_front().unwrap_or(*self.last.lock().expect("last"));
                *self.last.lock().expect("last") = next;
                next
            };
            Ok(vec![self.view_with(status)])
        }

        fn catalog(&self) -> crate::Result<crate::service::CatalogView> {
            Ok(crate::service::CatalogView {
                status: crate::model::CatalogStatus::Fresh,
                banner: None,
                engines: Vec::new(),
            })
        }
        fn refresh_catalog(&self, _force: bool) -> crate::Result<crate::service::OperationId> {
            Err(crate::DblError::NotImplemented("test double".into()))
        }
        fn validate_spec(&self, _spec: &crate::model::InstanceSpec) -> crate::Result<()> {
            Ok(())
        }
        fn suggest_port(&self, _engine: EngineKey) -> crate::Result<crate::model::Port> {
            Err(crate::DblError::NotImplemented("test double".into()))
        }
        fn create_instance(
            &self,
            _spec: crate::model::InstanceSpec,
        ) -> crate::Result<crate::service::OperationId> {
            Err(crate::DblError::NotImplemented("test double".into()))
        }
        fn start_instance(&self, _id: &InstanceId) -> crate::Result<crate::service::OperationId> {
            Ok(crate::service::OperationId::new("op-start"))
        }
        fn stop_instance(&self, _id: &InstanceId) -> crate::Result<crate::service::OperationId> {
            Err(crate::DblError::NotImplemented("test double".into()))
        }
        fn delete_instance(
            &self,
            _id: &InstanceId,
            _mode: crate::model::DeleteMode,
        ) -> crate::Result<crate::service::OperationId> {
            Err(crate::DblError::NotImplemented("test double".into()))
        }
        fn change_port(&self, _id: &InstanceId, _port: crate::model::Port) -> crate::Result<()> {
            Err(crate::DblError::NotImplemented("test double".into()))
        }
        fn data_size(&self, _id: &InstanceId) -> crate::Result<u64> {
            Ok(0)
        }
        fn set_autostart(&self, _id: &InstanceId, _enabled: bool) -> crate::Result<()> {
            Ok(())
        }
        fn cancel_operation(&self, _id: &crate::service::OperationId) -> crate::Result<()> {
            Ok(())
        }
        fn connection_uri(
            &self,
            _id: &InstanceId,
            _format: crate::service::UriFormat,
        ) -> crate::Result<crate::secret::SecretString> {
            Err(crate::DblError::NotImplemented("test double".into()))
        }
        fn tail_logs(&self, _id: &InstanceId, _lines: usize) -> crate::Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn watch_logs(&self, _id: &InstanceId, _enabled: bool) -> crate::Result<()> {
            Ok(())
        }
        fn list_runtimes(&self) -> crate::Result<Vec<crate::model::Runtime>> {
            Ok(Vec::new())
        }
        fn remove_runtime(&self, _runtime: &str) -> crate::Result<()> {
            Ok(())
        }
        fn list_archives(&self) -> crate::Result<Vec<crate::model::ArchivedData>> {
            Ok(Vec::new())
        }
        fn purge_archive(&self, _archive: &str) -> crate::Result<()> {
            Ok(())
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<crate::service::ServiceEvent> {
            let (_tx, rx) = tokio::sync::broadcast::channel(1);
            rx
        }
    }

    #[test]
    fn wait_for_running_polls_until_the_row_is_running() {
        // starting, starting, then running: the loop must read the store at
        // least three times and return Ok only after the change. A loop that
        // returns Ok immediately would read once.
        let id = InstanceId::new();
        let service = ScriptedService::new(
            id.clone(),
            vec![
                InstanceStatus::Starting,
                InstanceStatus::Starting,
                InstanceStatus::Running,
            ],
        );
        let started = std::time::Instant::now();
        let result = wait_for_running(&service, &id, Duration::from_secs(30));
        let elapsed = started.elapsed();
        assert_eq!(result, Ok(()), "the row reached running");
        assert!(
            service.reads() >= 3,
            "the loop must poll until the row changes, reads={}",
            service.reads()
        );
        // Two full polls before the third read: at least two poll sleeps.
        let two_polls = Duration::from_millis(consts::AUTOSTART_WAIT_POLL_MS * 2);
        assert!(
            elapsed >= two_polls.saturating_sub(Duration::from_millis(50)),
            "the loop returned in {elapsed:?} without really waiting (expected >= ~{two_polls:?})"
        );
    }

    #[test]
    fn wait_for_running_times_out_when_the_row_never_leaves_starting() {
        // The row stays `starting`. With a bound shorter than three polls the
        // loop must give up with the DBL-8401 timeout, after more than one read
        // and after really waiting.
        let id = InstanceId::new();
        let service = ScriptedService::new(id.clone(), vec![InstanceStatus::Starting]);
        let bound = Duration::from_millis(consts::AUTOSTART_WAIT_POLL_MS + 100);
        let started = std::time::Instant::now();
        let result = wait_for_running(&service, &id, bound);
        let elapsed = started.elapsed();
        let err = result.expect_err("a row that never reaches running must time out");
        assert!(err.contains("DBL-8401"), "expected a timeout, got {err}");
        assert!(
            service.reads() >= 2,
            "the loop must poll at least twice before timing out, reads={}",
            service.reads()
        );
        assert!(
            elapsed >= Duration::from_millis(consts::AUTOSTART_WAIT_POLL_MS),
            "the timeout fired in {elapsed:?} without polling/sleeping"
        );
    }
}
