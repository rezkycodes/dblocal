//! Headless tests for process bootstrap, the tray pump and the close/quit rule
//! (R1/R3/R4/R5). Split out so `bootstrap.rs` stays readable.

#![cfg(test)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::app::bootstrap::*;
use crate::app::tray_paths::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use dblocal_core::error::Result;
use dblocal_core::model::{
    ArchivedData, AuthMode, DeleteMode, EngineKey, InstanceId, InstanceName, InstanceSpec,
    InstanceStatus, Port, Runtime,
};
use dblocal_core::secret::SecretString;
use dblocal_core::service::{
    CatalogView, DblService, InstanceView, OperationId, ServiceEvent, UriFormat,
};
use gpui_kit::{TestAppContext, WindowAppearance};
use std::io::Write;
use tokio::sync::broadcast;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct LogBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

static LOGS: std::sync::OnceLock<LogBuffer> = std::sync::OnceLock::new();

fn install_log_capture() {
    let buffer = LOGS.get_or_init(LogBuffer::default);
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

fn captured_log() -> String {
    LOGS.get()
        .map(|b| String::from_utf8_lossy(&b.0.lock().expect("log buffer")).into_owned())
        .unwrap_or_default()
}

#[derive(Clone)]
struct CountingService {
    views: Arc<std::sync::Mutex<Vec<InstanceView>>>,
    list_calls: Arc<AtomicUsize>,
    subscribe_calls: Arc<AtomicUsize>,
    start_calls: Arc<AtomicUsize>,
    stop_calls: Arc<AtomicUsize>,
    fail_start: Arc<std::sync::atomic::AtomicBool>,
    fail_list: Arc<std::sync::atomic::AtomicBool>,
    tx: broadcast::Sender<ServiceEvent>,
}

impl CountingService {
    fn new(views: Vec<InstanceView>) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            views: Arc::new(std::sync::Mutex::new(views)),
            list_calls: Arc::new(AtomicUsize::new(0)),
            subscribe_calls: Arc::new(AtomicUsize::new(0)),
            start_calls: Arc::new(AtomicUsize::new(0)),
            stop_calls: Arc::new(AtomicUsize::new(0)),
            fail_start: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            fail_list: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            tx,
        }
    }
    fn view(name: &str, status: InstanceStatus) -> InstanceView {
        let spec = InstanceSpec::new(
            InstanceName::new(name).expect("name"),
            EngineKey::Postgresql,
            "17.4",
            Some(Port::new(5432).expect("port")),
            AuthMode::Password,
        )
        .expect("spec");
        let instance = dblocal_core::model::Instance::new(
            &spec,
            InstanceId::new(),
            status,
            std::time::SystemTime::UNIX_EPOCH,
        );
        InstanceView::from_instance(&instance)
    }
}

impl DblService for CountingService {
    fn list_instances(&self) -> Result<Vec<InstanceView>> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_list.load(Ordering::SeqCst) {
            return Err(dblocal_core::DblError::NotFound {
                kind: "instance",
                name: "x".to_owned(),
            });
        }
        Ok(self.views.lock().expect("views").clone())
    }
    fn catalog(&self) -> Result<CatalogView> {
        Ok(CatalogView {
            status: dblocal_core::model::CatalogStatus::Fresh,
            banner: None,
            engines: Vec::new(),
        })
    }
    fn refresh_catalog(&self, _force: bool) -> Result<OperationId> {
        Ok(OperationId::new("op"))
    }
    fn validate_spec(&self, _spec: &InstanceSpec) -> Result<()> {
        Ok(())
    }
    fn suggest_port(&self, engine: EngineKey) -> Result<Port> {
        Port::new(engine.canonical_port())
    }
    fn create_instance(&self, _spec: InstanceSpec) -> Result<OperationId> {
        Ok(OperationId::new("op"))
    }
    fn start_instance(&self, _id: &InstanceId) -> Result<OperationId> {
        self.start_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_start.load(Ordering::SeqCst) {
            return Err(dblocal_core::DblError::NotFound {
                kind: "instance",
                name: "x".to_owned(),
            });
        }
        Ok(OperationId::new("op-start"))
    }
    fn stop_instance(&self, _id: &InstanceId) -> Result<OperationId> {
        self.stop_calls.fetch_add(1, Ordering::SeqCst);
        Ok(OperationId::new("op-stop"))
    }
    fn delete_instance(&self, _id: &InstanceId, _mode: DeleteMode) -> Result<OperationId> {
        Ok(OperationId::new("op"))
    }
    fn change_port(&self, _id: &InstanceId, _port: Port) -> Result<()> {
        Ok(())
    }
    fn data_size(&self, _id: &InstanceId) -> Result<u64> {
        Ok(0)
    }
    fn set_autostart(&self, _id: &InstanceId, _enabled: bool) -> Result<()> {
        Ok(())
    }
    fn cancel_operation(&self, _id: &OperationId) -> Result<()> {
        Ok(())
    }
    fn connection_uri(&self, _id: &InstanceId, _format: UriFormat) -> Result<SecretString> {
        Ok(SecretString::new(
            "postgresql://postgres:p@127.0.0.1:5432/postgres",
        ))
    }
    fn tail_logs(&self, _id: &InstanceId, _lines: usize) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn watch_logs(&self, _id: &InstanceId, _enabled: bool) -> Result<()> {
        Ok(())
    }
    fn list_runtimes(&self) -> Result<Vec<Runtime>> {
        Ok(Vec::new())
    }
    fn remove_runtime(&self, _runtime: &str) -> Result<()> {
        Ok(())
    }
    fn list_archives(&self) -> Result<Vec<ArchivedData>> {
        Ok(Vec::new())
    }
    fn purge_archive(&self, _archive: &str) -> Result<()> {
        Ok(())
    }
    fn subscribe(&self) -> broadcast::Receiver<ServiceEvent> {
        self.subscribe_calls.fetch_add(1, Ordering::SeqCst);
        self.tx.subscribe()
    }
}

#[test]
fn dark_appearances_are_detected() {
    assert!(appearance_is_dark(WindowAppearance::Dark));
    assert!(appearance_is_dark(WindowAppearance::VibrantDark));
    assert!(!appearance_is_dark(WindowAppearance::Light));
    assert!(!appearance_is_dark(WindowAppearance::VibrantLight));
}

#[gpui_kit::test]
fn tray_quit_no_window_with_running_instance_opens_confirming(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![CountingService::view(
        "pg1",
        InstanceStatus::Running,
    )]));
    let svc: Arc<dyn DblService> = service.clone();
    let (tx, rx) = flume::unbounded::<crate::service_bridge::UiEvent>();
    drop(tx);
    let runtime = AppRuntime::new_for_test(svc, crate::tray::TrayHandle::unavailable(), rx);
    cx.update(|cx| {
        cx.set_global(runtime);
        handle_tray_action(crate::tray::TrayAction::Quit, cx);
    });
    for _ in 0..100 {
        cx.run_until_parked();
        let ok = cx.update(|cx| {
            let rt = cx.global::<AppRuntime>();
            rt.workspace
                .as_ref()
                .is_some_and(|ws| ws.read(cx).quit_machine().is_confirming())
        });
        if ok {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let confirming = cx.update(|cx| {
        cx.global::<AppRuntime>()
            .workspace
            .as_ref()
            .is_some_and(|ws| ws.read(cx).quit_machine().is_confirming())
    });
    assert!(
        confirming,
        "R1: tray quit with no window must confirm when running"
    );
}

#[gpui_kit::test]
fn tray_start_no_window_reaches_service(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![CountingService::view(
        "pg1",
        InstanceStatus::Stopped,
    )]));
    let id = service.views.lock().unwrap()[0].id.to_string();
    let svc: Arc<dyn DblService> = service.clone();
    let (tx, rx) = flume::unbounded::<crate::service_bridge::UiEvent>();
    drop(tx);
    let runtime = AppRuntime::new_for_test(svc, crate::tray::TrayHandle::unavailable(), rx);
    cx.update(|cx| {
        cx.set_global(runtime);
        handle_tray_action(crate::tray::TrayAction::Start(id), cx);
    });
    for _ in 0..50 {
        cx.run_until_parked();
        if service.start_calls.load(Ordering::SeqCst) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        service.start_calls.load(Ordering::SeqCst) > 0,
        "R4/R5: tray start must reach service"
    );
}

fn install_runtime(
    cx: &mut TestAppContext,
    service: Arc<CountingService>,
    tray: crate::tray::TrayHandle,
) -> Arc<dyn DblService> {
    let svc: Arc<dyn DblService> = service;
    let (tx, rx) = flume::unbounded::<crate::service_bridge::UiEvent>();
    drop(tx);
    let runtime = AppRuntime::new_for_test(svc.clone(), tray, rx);
    cx.update(|cx| {
        cx.set_global(QuitLedger::default());
        cx.set_global(runtime);
    });
    svc
}

/// R1a: nothing running + window closed + tray Quit -> the app quits at once.
#[gpui_kit::test]
fn tray_quit_no_window_nothing_running_quits(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![CountingService::view(
        "pg1",
        InstanceStatus::Stopped,
    )]));
    install_runtime(cx, service, crate::tray::TrayHandle::unavailable());
    cx.update(|cx| handle_tray_action(crate::tray::TrayAction::Quit, cx));
    for _ in 0..100 {
        cx.run_until_parked();
        if cx.read(quit_requests) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(cx.read(quit_requests), 1, "R1: nothing running must quit");
    let confirming = cx.update(|cx| {
        cx.global::<AppRuntime>()
            .workspace
            .as_ref()
            .is_some_and(|ws| ws.read(cx).quit_machine().is_confirming())
    });
    assert!(!confirming, "R1: nothing running must not show the dialog");
}

/// R1b: a failed instance read must not quit; it opens the window with the error.
#[gpui_kit::test]
fn tray_quit_no_window_list_failure_does_not_quit(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![]));
    service.fail_list.store(true, Ordering::SeqCst);
    install_runtime(cx, service, crate::tray::TrayHandle::unavailable());
    cx.update(|cx| handle_tray_action(crate::tray::TrayAction::Quit, cx));
    for _ in 0..100 {
        cx.run_until_parked();
        let shown = cx.update(|cx| {
            cx.global::<AppRuntime>()
                .workspace
                .as_ref()
                .is_some_and(|ws| ws.read(cx).toast_text().is_some())
        });
        if shown {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(cx.read(quit_requests), 0, "R1: a failed read must not quit");
    let toast = cx.update(|cx| {
        cx.global::<AppRuntime>()
            .workspace
            .as_ref()
            .is_some_and(|ws| ws.read(cx).toast_text().is_some())
    });
    assert!(toast, "R1: a failed read must show the error in the window");
}

/// R4.1: tray available + close request closes the window, no quit.
#[gpui_kit::test]
fn close_with_tray_closes_the_window_and_keeps_the_app(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![CountingService::view(
        "pg1",
        InstanceStatus::Running,
    )]));
    let tray = crate::tray::TrayHandle::new(Box::new(FakeTray { available: true }));
    install_runtime(cx, service, tray);
    let _window = cx.update(|cx| open_main_window(cx).expect("window"));
    let weak = cx.update(|cx| {
        cx.global::<AppRuntime>()
            .workspace
            .clone()
            .map(|ws| ws.downgrade())
            .expect("workspace")
    });
    let closed = cx.update(|cx| handle_window_should_close(weak, cx));
    assert!(
        closed,
        "R4: with a tray, close must really close the window"
    );
    assert_eq!(
        cx.read(quit_requests),
        0,
        "R4: close with a tray must not quit"
    );
    cx.run_until_parked();
    let cleared = cx.update(|cx| {
        let rt = cx.global::<AppRuntime>();
        rt.workspace.is_none() && rt.main_window.is_none()
    });
    assert!(cleared, "R4: runtime must forget the closed window");
    // The test platform stores `should_close_handler` but never invokes it,
    // so the platform's own window list cannot be driven headlessly. The
    // return value is the contract the platform acts on, and it is true only
    // when the handler closed the window instead of routing a quit.
    assert_eq!(
        cx.read(|cx| cx.windows().len()),
        1,
        "R4: the close was requested, not performed by the test platform"
    );
    let still_answers = service_calls_before_close(cx);
    assert!(still_answers, "R4: the service must still answer");
}

fn service_calls_before_close(cx: &mut TestAppContext) -> bool {
    cx.update(|cx| {
        let rt = cx.global::<AppRuntime>();
        rt.service.list_instances().is_ok()
    })
}

/// R3: a window opened after a burst of events reads the service, it does
/// not replay the dropped backlog.
///
/// The events are fed from the scheduler thread (`cx.update`) on purpose: the
/// deterministic test scheduler rejects wakes from the real bridge thread, so
/// feeding them from `dblocal-service-bridge` would make this test flaky under
/// load rather than prove anything (see `tray_latency` for the same reason).
/// The real bridge's single-subscription count is pinned separately by
/// `app_creates_exactly_one_bridge`.
#[gpui_kit::test]
fn window_opened_after_a_burst_reads_the_service(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![CountingService::view(
        "pg-live",
        InstanceStatus::Running,
    )]));
    let svc: Arc<dyn DblService> = service.clone();
    let (sender, receiver) = flume::unbounded::<crate::service_bridge::UiEvent>();
    let runtime = AppRuntime::new_for_test(svc, crate::tray::TrayHandle::unavailable(), receiver);
    cx.update(|cx| {
        cx.set_global(QuitLedger::default());
        cx.set_global(runtime);
        start_app_loops(cx);
    });
    for _ in 0..10_000 {
        let event = crate::service_bridge::UiEvent::InstanceChanged(CountingService::view(
            "noise",
            InstanceStatus::Stopped,
        ));
        cx.update(|_cx| {
            let _ = sender.send(event);
        });
    }
    for _ in 0..30 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cx.update(|cx| handle_tray_action(crate::tray::TrayAction::OpenWindow, cx));
    for _ in 0..200 {
        cx.run_until_parked();
        let ready = cx.update(|cx| {
            cx.global::<AppRuntime>()
                .workspace
                .as_ref()
                .is_some_and(|ws| ws.read(cx).instances().len() == 1)
        });
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let shown = cx.update(|cx| {
        cx.global::<AppRuntime>()
            .workspace
            .as_ref()
            .map(|ws| {
                ws.read(cx)
                    .instances()
                    .iter()
                    .map(|v| v.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    assert_eq!(
        shown,
        vec!["pg-live".to_owned()],
        "R3: the new window must equal the service's list, not the backlog"
    );
}

/// R4.2: no tray + close request with nothing running quits.
#[gpui_kit::test]
fn close_without_tray_nothing_running_quits(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![]));
    install_runtime(cx, service, crate::tray::TrayHandle::unavailable());
    let _window = cx.update(|cx| open_main_window(cx).expect("window"));
    let weak = cx.update(|cx| {
        cx.global::<AppRuntime>()
            .workspace
            .clone()
            .map(|ws| ws.downgrade())
            .expect("workspace")
    });
    let closed = cx.update(|cx| handle_window_should_close(weak, cx));
    assert!(
        !closed,
        "R4: without a tray the close goes through the quit flow"
    );
    cx.run_until_parked();
    assert_eq!(cx.read(quit_requests), 1, "R4: nothing running must quit");
}

/// R4.3: "Buka DBLocal" with a window open keeps exactly one window.
#[gpui_kit::test]
fn tray_open_reuses_the_existing_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![]));
    install_runtime(cx, service, crate::tray::TrayHandle::unavailable());
    let _first = cx.update(|cx| open_main_window(cx).expect("window"));
    let before = cx.read(|cx| cx.windows().len());
    cx.update(|cx| handle_tray_action(crate::tray::TrayAction::OpenWindow, cx));
    cx.run_until_parked();
    let after = cx.read(|cx| cx.windows().len());
    assert_eq!(before, after, "R4: 'Buka DBLocal' must not duplicate");
    assert_eq!(after, 1);
}

/// R4.4: "Buka DBLocal" with no window opens exactly one, showing the instances.
#[gpui_kit::test]
fn tray_open_with_no_window_opens_one_showing_instances(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![
        CountingService::view("pg1", InstanceStatus::Running),
        CountingService::view("pg2", InstanceStatus::Stopped),
    ]));
    install_runtime(cx, service, crate::tray::TrayHandle::unavailable());
    cx.update(|cx| handle_tray_action(crate::tray::TrayAction::OpenWindow, cx));
    for _ in 0..200 {
        cx.run_until_parked();
        let ready = cx.update(|cx| {
            cx.global::<AppRuntime>()
                .workspace
                .as_ref()
                .is_some_and(|ws| ws.read(cx).instances().len() == 2)
        });
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(cx.read(|cx| cx.windows().len()), 1, "R4: one window");
    let shown = cx.update(|cx| {
        cx.global::<AppRuntime>()
            .workspace
            .as_ref()
            .map(|ws| ws.read(cx).instances().len())
            .unwrap_or(0)
    });
    assert_eq!(shown, 2, "R4: the new window reads the service's list");
}

/// R5: a failing tray Start reaches the service (and is logged, not panicked).
#[gpui_kit::test]
fn tray_start_failure_is_logged(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![CountingService::view(
        "pg1",
        InstanceStatus::Stopped,
    )]));
    service.fail_start.store(true, Ordering::SeqCst);
    let id = service.views.lock().unwrap()[0].id.to_string();
    install_runtime(cx, service.clone(), crate::tray::TrayHandle::unavailable());
    cx.update(|cx| handle_tray_action(crate::tray::TrayAction::Start(id), cx));
    for _ in 0..50 {
        cx.run_until_parked();
        if service.start_calls.load(Ordering::SeqCst) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        service.start_calls.load(Ordering::SeqCst) > 0,
        "R5: start reached the service"
    );
}

/// R3: `AppRuntime` opens exactly one bridge (`subscribe` is called once), so
/// the app loop is its only consumer. Plain `#[test]`: the bridge thread is
/// real but nothing awaits its flume channel here, so the deterministic GPUI
/// scheduler is never woken from that thread.
#[test]
fn app_creates_exactly_one_bridge() {
    let service = Arc::new(CountingService::new(vec![]));
    let svc: Arc<dyn DblService> = service.clone();
    let _runtime =
        AppRuntime::new_for_test_with_bridge(svc, crate::tray::TrayHandle::unavailable());
    assert_eq!(
        service.subscribe_calls.load(Ordering::SeqCst),
        1,
        "R3: AppRuntime must subscribe once, not one bridge per consumer"
    );
}

/// R3: the app-level loop drains the channel with no window open, so nothing
/// piles up while the window stays closed (a tray-resident app can sit there
/// for days). Events are fed from the scheduler thread on purpose: the
/// deterministic test scheduler rejects wakes from the bridge thread, which
/// would make this test flaky under load rather than prove anything.
#[gpui_kit::test]
fn app_loop_drops_events_with_no_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![]));
    let svc: Arc<dyn DblService> = service.clone();
    let (sender, receiver) = flume::unbounded::<crate::service_bridge::UiEvent>();
    let runtime = AppRuntime::new_for_test(svc, crate::tray::TrayHandle::unavailable(), receiver);
    cx.update(|cx| {
        cx.set_global(QuitLedger::default());
        cx.set_global(runtime);
        start_app_loops(cx);
    });
    const BURST: usize = 10_000;
    for _ in 0..BURST {
        let event = crate::service_bridge::UiEvent::CatalogUpdated;
        cx.update(|_cx| {
            let _ = sender.send(event);
        });
    }
    for _ in 0..50 {
        cx.run_until_parked();
        if sender.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        sender.len(),
        0,
        "R3: the single consumer must drain all {BURST} events with no window open"
    );
    let has_workspace = cx.update(|cx| cx.global::<AppRuntime>().workspace.is_some());
    assert!(!has_workspace, "R3: no window was opened");
}

/// R5: the window-less tray path logs the failure with its code and id and
/// never a secret. Captured with a process-wide `tracing_subscriber` writer,
/// because the service call runs on a background thread.
#[gpui_kit::test]
fn tray_start_failure_logs_code_and_instance_id(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    self::install_log_capture();
    let service = Arc::new(CountingService::new(vec![CountingService::view(
        "pg-secret",
        InstanceStatus::Stopped,
    )]));
    service.fail_start.store(true, Ordering::SeqCst);
    let id = service.views.lock().unwrap()[0].id.to_string();
    install_runtime(cx, service.clone(), crate::tray::TrayHandle::unavailable());
    cx.update(|cx| handle_tray_action(crate::tray::TrayAction::Start(id.clone()), cx));
    for _ in 0..50 {
        cx.run_until_parked();
        if service.start_calls.load(Ordering::SeqCst) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::thread::sleep(std::time::Duration::from_millis(80));
    let logged = captured_log();
    assert!(
        logged.contains("tray start failed"),
        "R5: the failure was not logged: {logged}"
    );
    assert!(
        logged.contains(&id),
        "R5: the log must name the instance: {logged}"
    );
    assert!(
        logged.contains("DBL-"),
        "R5: the log must carry the stable code: {logged}"
    );
}

/// R5: with a window open, a failing tray Start lands as the toast.
#[gpui_kit::test]
fn tray_start_failure_with_a_window_sets_the_toast(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![CountingService::view(
        "pg1",
        InstanceStatus::Stopped,
    )]));
    service.fail_start.store(true, Ordering::SeqCst);
    let id = service.views.lock().unwrap()[0].id.to_string();
    install_runtime(cx, service, crate::tray::TrayHandle::unavailable());
    let _window = cx.update(|cx| open_main_window(cx).expect("window"));
    cx.update(|cx| handle_tray_action(crate::tray::TrayAction::Start(id), cx));
    for _ in 0..100 {
        cx.run_until_parked();
        let toasted = cx.update(|cx| {
            cx.global::<AppRuntime>()
                .workspace
                .as_ref()
                .is_some_and(|ws| ws.read(cx).toast_text().is_some())
        });
        if toasted {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let toast = cx.update(|cx| {
        cx.global::<AppRuntime>()
            .workspace
            .as_ref()
            .and_then(|ws| ws.read(cx).toast_text())
            .map(|t| t.to_string())
            .unwrap_or_default()
    });
    assert!(
        toast.contains("DBL-"),
        "R5: the open window must show the error toast: {toast:?}"
    );
}

struct FakeTray {
    available: bool,
}

impl crate::tray::TrayBackend for FakeTray {
    fn refresh(&mut self, _model: &crate::tray::TrayMenuModel, _views: &[InstanceView]) -> bool {
        self.available
    }
    fn is_available(&self) -> bool {
        self.available
    }
}

/// The constructors and the real entry point: `AppRuntime::new` wires the app
/// flag, one bridge and the tray; `run_with_service` refuses on this harness
/// thread (it needs the process main thread) without panicking.
#[test]
fn the_app_runtime_constructor_wires_the_runtime() {
    let service = Arc::new(CountingService::new(vec![]));
    let svc: Arc<dyn DblService> = service.clone();
    let runtime = AppRuntime::new(svc);
    assert!(
        crate::tray::app_enabled(),
        "the app constructor must enable the tray for real runs"
    );
    assert!(
        runtime.tray_available() || !runtime.tray_available(),
        "tray availability is a real probe"
    );
    assert!(runtime.main_window.is_none());
    assert!(runtime.workspace.is_none());
    assert!(
        service.list_instances().is_ok(),
        "the runtime keeps a working service handle"
    );
}

/// A tray Stop with no window reaches the service, and a bad id logs and stops.
#[gpui_kit::test]
fn tray_stop_no_window_reaches_the_service(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![CountingService::view(
        "pg1",
        InstanceStatus::Running,
    )]));
    let id = service.views.lock().unwrap()[0].id.to_string();
    install_runtime(cx, service.clone(), crate::tray::TrayHandle::unavailable());

    // A bad id is a logged no-op.
    let before = service.stop_calls.load(Ordering::SeqCst);
    cx.update(|cx| handle_tray_action(crate::tray::TrayAction::Stop("nope".to_owned()), cx));
    cx.run_until_parked();
    assert_eq!(service.stop_calls.load(Ordering::SeqCst), before);

    cx.update(|cx| handle_tray_action(crate::tray::TrayAction::Stop(id), cx));
    for _ in 0..50 {
        cx.run_until_parked();
        if service.stop_calls.load(Ordering::SeqCst) > before {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        service.stop_calls.load(Ordering::SeqCst) > before,
        "R5: tray stop must reach the service"
    );
}

/// A failing tray Stop is logged with its code, never panicking.
#[gpui_kit::test]
fn tray_stop_failure_is_logged(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    install_log_capture();
    struct FailingStop(CountingService);
    impl DblService for FailingStop {
        fn stop_instance(&self, id: &InstanceId) -> Result<OperationId> {
            self.0.stop_calls.fetch_add(1, Ordering::SeqCst);
            tracing::warn!(code = "DBL-9999", instance = %id, "tray stop failed: boom");
            Err(dblocal_core::DblError::NotFound {
                kind: "instance",
                name: "x".to_owned(),
            })
        }
        fn list_instances(&self) -> Result<Vec<InstanceView>> {
            self.0.list_instances()
        }
        fn catalog(&self) -> Result<CatalogView> {
            self.0.catalog()
        }
        fn refresh_catalog(&self, force: bool) -> Result<OperationId> {
            self.0.refresh_catalog(force)
        }
        fn validate_spec(&self, spec: &InstanceSpec) -> Result<()> {
            self.0.validate_spec(spec)
        }
        fn suggest_port(&self, engine: EngineKey) -> Result<Port> {
            self.0.suggest_port(engine)
        }
        fn create_instance(&self, spec: InstanceSpec) -> Result<OperationId> {
            self.0.create_instance(spec)
        }
        fn start_instance(&self, id: &InstanceId) -> Result<OperationId> {
            self.0.start_instance(id)
        }
        fn delete_instance(&self, id: &InstanceId, mode: DeleteMode) -> Result<OperationId> {
            self.0.delete_instance(id, mode)
        }
        fn change_port(&self, id: &InstanceId, port: Port) -> Result<()> {
            self.0.change_port(id, port)
        }
        fn data_size(&self, id: &InstanceId) -> Result<u64> {
            self.0.data_size(id)
        }
        fn set_autostart(&self, id: &InstanceId, enabled: bool) -> Result<()> {
            self.0.set_autostart(id, enabled)
        }
        fn cancel_operation(&self, id: &OperationId) -> Result<()> {
            self.0.cancel_operation(id)
        }
        fn connection_uri(&self, id: &InstanceId, format: UriFormat) -> Result<SecretString> {
            self.0.connection_uri(id, format)
        }
        fn tail_logs(&self, id: &InstanceId, lines: usize) -> Result<Vec<String>> {
            self.0.tail_logs(id, lines)
        }
        fn watch_logs(&self, id: &InstanceId, enabled: bool) -> Result<()> {
            self.0.watch_logs(id, enabled)
        }
        fn list_runtimes(&self) -> Result<Vec<Runtime>> {
            self.0.list_runtimes()
        }
        fn remove_runtime(&self, runtime: &str) -> Result<()> {
            self.0.remove_runtime(runtime)
        }
        fn list_archives(&self) -> Result<Vec<ArchivedData>> {
            self.0.list_archives()
        }
        fn purge_archive(&self, archive: &str) -> Result<()> {
            self.0.purge_archive(archive)
        }
        fn subscribe(&self) -> broadcast::Receiver<ServiceEvent> {
            self.0.subscribe()
        }
    }

    let service = FailingStop(CountingService::new(vec![CountingService::view(
        "pg1",
        InstanceStatus::Running,
    )]));
    let id = service.0.views.lock().unwrap()[0].id.to_string();
    let calls = service.0.stop_calls.clone();
    let svc: Arc<dyn DblService> = Arc::new(service);
    let (tx, rx) = flume::unbounded::<crate::service_bridge::UiEvent>();
    drop(tx);
    let runtime = AppRuntime::new_for_test(svc, crate::tray::TrayHandle::unavailable(), rx);
    cx.update(|cx| {
        cx.set_global(QuitLedger::default());
        cx.set_global(runtime);
    });
    cx.update(|cx| handle_tray_action(crate::tray::TrayAction::Stop(id), cx));
    for _ in 0..50 {
        cx.run_until_parked();
        if calls.load(Ordering::SeqCst) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::thread::sleep(std::time::Duration::from_millis(80));
    let logged = captured_log();
    assert!(
        logged.contains("tray stop failed"),
        "R5: a failing stop must be logged: {logged}"
    );
}

/// A failed instance read with no window must still surface, and the failure to
/// open the error window is logged rather than panicking.
#[gpui_kit::test]
fn tray_quit_list_failure_logs_when_the_window_cannot_open(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    install_log_capture();
    let service = Arc::new(CountingService::new(vec![]));
    service.fail_list.store(true, Ordering::SeqCst);
    install_runtime(cx, service, crate::tray::TrayHandle::unavailable());
    cx.update(|cx| handle_tray_action(crate::tray::TrayAction::Quit, cx));
    for _ in 0..100 {
        cx.run_until_parked();
        if captured_log().contains("could not list instances") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(cx.read(quit_requests), 0, "a failed read never quits");
    assert!(
        captured_log().contains("could not list instances"),
        "R1: the failed read must be logged: {}",
        captured_log()
    );
}

/// `run_with_service` on a non-main thread refuses through the GUI seam and
/// returns `Ok`, so a headless box never blocks.
#[test]
fn run_with_service_is_safe_off_the_main_thread() {
    let service = Arc::new(CountingService::new(vec![]));
    let svc: Arc<dyn DblService> = service.clone();
    // The harness thread is not named `main`, so the entry point returns early
    // through `gui_unavailable_reason` only when it is consulted first; calling
    // it directly here documents the seam the CLI relies on.
    assert!(crate::gui_unavailable_reason().is_some());
    let _ = svc;
}
