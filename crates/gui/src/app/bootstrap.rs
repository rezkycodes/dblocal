//! Process bootstrap: open the window and hand it a `Workspace`.
//!
//! The app-level runtime (service, tray, event bridge, quit state) outlives any
//! single window so F-TRY-02 works on Linux where `cx.hide()` is a no-op: closing
//! the window with a tray present closes the window but keeps the process alive,
//! and "Buka DBLocal" opens a fresh window attached to the same service.

use std::sync::Arc;

use dblocal_core::service::DblService;
use gpui_kit::component::{Root, TitleBar};
use gpui_kit::{
    App, AppContext as _, BorrowAppContext, Global, QuitMode, WindowAppearance, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, px, size,
};

use super::Workspace;

/// App-level runtime shared by all windows.
pub struct AppRuntime {
    pub service: Arc<dyn DblService>,
    pub tray: crate::tray::TrayHandle,
    pub events: flume::Receiver<crate::service_bridge::UiEvent>,
    pub main_window: Option<WindowHandle<Root>>,
    pub workspace: Option<gpui_kit::Entity<Workspace>>,
}

impl Global for AppRuntime {}

/// Counts quit requests for headless assertions (the test platform's `quit()`
/// is a no-op, so "the app asked to quit" must be observable).
#[derive(Default)]
pub struct QuitLedger {
    requests: std::sync::atomic::AtomicUsize,
}

impl Global for QuitLedger {}

/// The single quit path: record the request, then ask the platform to quit.
pub(crate) fn request_app_quit(cx: &mut App) {
    if cx.has_global::<QuitLedger>() {
        cx.update_global::<QuitLedger, _>(|ledger, _| {
            ledger
                .requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
    }
    cx.quit();
}

/// How many times this app asked to quit (tests only).
#[cfg(test)]
pub fn quit_requests(cx: &App) -> usize {
    cx.try_global::<QuitLedger>()
        .map(|ledger| ledger.requests.load(std::sync::atomic::Ordering::SeqCst))
        .unwrap_or(0)
}

impl AppRuntime {
    pub fn new(service: Arc<dyn DblService>) -> Self {
        crate::tray::enable_for_app();
        let events = crate::service_bridge::spawn_bridge(service.clone());
        let tray_start = crate::tray::start_tray();
        Self {
            service,
            tray: tray_start.handle,
            events,
            main_window: None,
            workspace: None,
        }
    }

    #[cfg(test)]
    pub fn new_for_test(
        service: Arc<dyn DblService>,
        tray: crate::tray::TrayHandle,
        events: flume::Receiver<crate::service_bridge::UiEvent>,
    ) -> Self {
        Self {
            service,
            tray,
            events,
            main_window: None,
            workspace: None,
        }
    }

    pub fn tray_available(&self) -> bool {
        self.tray.is_available()
    }

    pub fn refresh_tray(&mut self, views: &[dblocal_core::service::InstanceView]) -> bool {
        self.tray.refresh(views)
    }
}

/// Opens the GUI with the in-memory demo service.
#[cfg(feature = "demo")]
pub(crate) fn run_with_mock() -> dblocal_core::Result<()> {
    let service: Arc<dyn DblService> = Arc::new(dblocal_core::service::mock::MockService::demo());
    run_with_service(service)
}

/// Opens the GUI against any [`DblService`] implementation.
pub fn run_with_service(service: Arc<dyn DblService>) -> dblocal_core::Result<()> {
    gpui_kit::platform::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            super::actions::bind_keys(cx);

            let runtime = AppRuntime::new(service);
            let tray_available = runtime.tray_available();
            cx.set_global(runtime);
            cx.set_global(QuitLedger::default());
            cx.set_quit_mode(if tray_available {
                QuitMode::Explicit
            } else {
                QuitMode::LastWindowClosed
            });

            start_app_event_loop(cx);
            start_tray_action_pump(cx);

            if let Err(err) = open_main_window(cx) {
                tracing::error!("could not open the main window: {err:#}");
                request_app_quit(cx);
            }
        });
    Ok(())
}

/// Single app-level event loop: consumes the one bridge, refreshes tray, and
/// forwards events to the live workspace when present. Drops events when no
/// window is open so the channel stays bounded (R3).
pub fn start_app_event_loop(cx: &mut App) {
    let (events, service) = {
        let runtime = cx.global::<AppRuntime>();
        (runtime.events.clone(), runtime.service.clone())
    };
    cx.spawn(async move |cx: &mut gpui_kit::AsyncApp| {
        while let Ok(event) = events.recv_async().await {
            let closed = matches!(event, crate::service_bridge::UiEvent::Closed);
            let should_refresh_tray = matches!(
                event,
                crate::service_bridge::UiEvent::InstanceChanged(_)
                    | crate::service_bridge::UiEvent::InstanceRemoved(_)
                    | crate::service_bridge::UiEvent::Resync
                    | crate::service_bridge::UiEvent::CatalogUpdated
            );
            if should_refresh_tray {
                refresh_tray_from_service(service.clone(), cx).await;
            }
            cx.update(|cx| {
                let ws = cx
                    .try_global::<AppRuntime>()
                    .and_then(|r| r.workspace.clone());
                if let Some(ws) = ws {
                    ws.update(cx, |this, cx| this.apply_event_with_cx(event, cx));
                }
            });
            if closed {
                break;
            }
        }
    })
    .detach();
}

async fn refresh_tray_from_service(service: Arc<dyn DblService>, cx: &mut gpui_kit::AsyncApp) {
    let views = cx
        .background_spawn(async move { service.list_instances().unwrap_or_default() })
        .await;
    cx.update(|cx| {
        if cx.has_global::<AppRuntime>() {
            cx.update_global::<AppRuntime, _>(|runtime, _| {
                runtime.refresh_tray(&views);
            });
        }
    });
}

/// App-level tray action pump: forward to workspace when open, else handle.
pub fn start_tray_action_pump(cx: &mut App) {
    let actions_rx = cx.global::<AppRuntime>().tray.subscribe_actions();
    cx.spawn(async move |cx: &mut gpui_kit::AsyncApp| {
        while let Ok(action) = actions_rx.recv_async().await {
            cx.update(|cx| {
                handle_tray_action(action, cx);
            });
        }
    })
    .detach();
}

/// Decision: with a tray present, closing the window should close it and keep
/// the process alive; without a tray, the quit flow handles the close.
pub fn window_close_should_close(has_tray: bool) -> bool {
    has_tray
}

/// Decision: if a main window already exists, focus it instead of opening a
/// second one.
pub fn should_focus_existing_main_window(has_main_window: bool) -> bool {
    has_main_window
}

pub(crate) fn open_main_window(cx: &mut App) -> anyhow::Result<WindowHandle<Root>> {
    if let Some(handle) = cx.try_global::<AppRuntime>().and_then(|r| r.main_window)
        && should_focus_existing_main_window(true)
    {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
        return Ok(handle);
    }

    let bounds = gpui_kit::Bounds::centered(None, size(px(1280.), px(820.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(900.), px(600.))),
        app_id: Some("dev.dblocal.DBLocal".to_owned()),
        ..TitleBar::window_options()
    };
    let _ = WindowKind::Normal;

    let handle = cx.open_window(options, move |window, cx| {
        let workspace = cx.new(|cx| Workspace::new(window, cx));
        cx.update_global::<AppRuntime, _>(|runtime, _| {
            runtime.workspace = Some(workspace.clone());
        });
        let weak = workspace.downgrade();
        window.on_window_should_close(cx, move |_window, cx| {
            handle_window_should_close(weak.clone(), cx)
        });
        cx.new(|cx| Root::new(workspace, window, cx))
    })?;

    cx.update_global::<AppRuntime, _>(|runtime, _| {
        runtime.main_window = Some(handle);
    });
    cx.activate(true);
    Ok(handle)
}

/// Window-close decision, extracted for headless tests (R4).
pub fn handle_window_should_close(weak: gpui_kit::WeakEntity<Workspace>, cx: &mut App) -> bool {
    if let Some(workspace) = weak.upgrade() {
        workspace.update(cx, |this, cx| {
            this.handle_window_close_request(cx);
        });
        let has_tray = cx
            .try_global::<AppRuntime>()
            .is_some_and(|r| r.tray_available());
        if window_close_should_close(has_tray) {
            clear_main_window(cx);
            return true;
        }
        return false;
    }
    true
}

/// Route a tray action. With a workspace open, forward to it; otherwise handle
/// at app level (R1/R4/R5).
pub fn handle_tray_action(action: crate::tray::TrayAction, cx: &mut App) {
    let ws = cx
        .try_global::<AppRuntime>()
        .and_then(|r| r.workspace.clone());
    if let Some(ws) = ws {
        ws.update(cx, |this, cx| this.apply_tray_action(action, cx));
        return;
    }
    handle_app_tray_action(action, cx);
}

/// App-level tray handling when no window is open.
pub fn handle_app_tray_action(action: crate::tray::TrayAction, cx: &mut App) {
    let Some(service) = cx.try_global::<AppRuntime>().map(|r| r.service.clone()) else {
        return;
    };
    match action {
        crate::tray::TrayAction::OpenWindow => {
            if let Err(err) = open_main_window(cx) {
                tracing::warn!("tray open window failed: {err:#}");
            }
        }
        crate::tray::TrayAction::Quit => handle_tray_quit_no_window(service, cx),
        crate::tray::TrayAction::Start(id) => handle_tray_start_no_window(service, id, cx),
        crate::tray::TrayAction::Stop(id) => handle_tray_stop_no_window(service, id, cx),
    }
}

fn handle_tray_quit_no_window(service: Arc<dyn DblService>, cx: &mut App) {
    cx.spawn(async move |cx: &mut gpui_kit::AsyncApp| {
        let svc = service.clone();
        let listed = cx
            .background_spawn(async move { svc.list_instances() })
            .await;
        cx.update(|cx| match listed {
            Ok(views) => {
                let running = crate::quit::running_instance_ids(&views);
                if running.is_empty() {
                    request_app_quit(cx);
                    return;
                }
                match open_main_window(cx) {
                    Ok(_) => {
                        if let Some(ws) = cx
                            .try_global::<AppRuntime>()
                            .and_then(|r| r.workspace.clone())
                        {
                            ws.update(cx, |this, cx| this.request_quit_with(running, cx));
                        }
                    }
                    Err(err) => {
                        tracing::warn!("tray quit could not open window: {err:#}");
                    }
                }
            }
            Err(err) => {
                tracing::warn!("tray quit could not list instances: {err}");
                if let Err(open_err) = open_main_window(cx) {
                    tracing::warn!("tray quit could not open error window: {open_err:#}");
                    return;
                }
                let facing = dblocal_core::service::UserFacingError::from(&err);
                if let Some(ws) = cx
                    .try_global::<AppRuntime>()
                    .and_then(|r| r.workspace.clone())
                {
                    ws.update(cx, |this, cx| {
                        this.report_user_facing_error(&facing);
                        cx.notify();
                    });
                }
            }
        });
    })
    .detach();
}

fn handle_tray_start_no_window(service: Arc<dyn DblService>, id: String, cx: &mut App) {
    let Ok(id) = id.parse::<dblocal_core::model::InstanceId>() else {
        tracing::warn!("tray start: invalid instance id");
        return;
    };
    cx.background_spawn(async move {
        if let Err(err) = service.start_instance(&id) {
            tracing::warn!("tray start failed for {id}: {err}");
        }
    })
    .detach();
}

fn handle_tray_stop_no_window(service: Arc<dyn DblService>, id: String, cx: &mut App) {
    let Ok(id) = id.parse::<dblocal_core::model::InstanceId>() else {
        tracing::warn!("tray stop: invalid instance id");
        return;
    };
    cx.background_spawn(async move {
        if let Err(err) = service.stop_instance(&id) {
            tracing::warn!("tray stop failed for {id}: {err}");
        }
    })
    .detach();
}

pub(crate) fn clear_main_window(cx: &mut App) {
    if cx.has_global::<AppRuntime>() {
        cx.update_global::<AppRuntime, _>(|runtime, _| {
            runtime.main_window = None;
            runtime.workspace = None;
        });
    }
}

/// Returns whether a system appearance is dark.
pub fn appearance_is_dark(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
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
    use gpui_kit::TestAppContext;
    use tokio::sync::broadcast;

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

    /// R3: the app-level loop is the only consumer, and one bridge is made.
    #[gpui_kit::test]
    fn app_uses_exactly_one_bridge_and_drops_events_with_no_window(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(crate::theme::init);
        let service = Arc::new(CountingService::new(vec![]));
        let svc: Arc<dyn DblService> = service.clone();
        let events = crate::service_bridge::spawn_bridge(svc.clone());
        let observed = events.clone();
        assert_eq!(
            service.subscribe_calls.load(Ordering::SeqCst),
            1,
            "R3: spawn_bridge subscribes once"
        );
        let runtime = AppRuntime::new_for_test(svc, crate::tray::TrayHandle::unavailable(), events);
        cx.update(|cx| {
            cx.set_global(QuitLedger::default());
            cx.set_global(runtime);
            start_app_event_loop(cx);
        });
        for _ in 0..10_000 {
            let _ = service.tx.send(ServiceEvent::CatalogUpdated {
                status: dblocal_core::model::CatalogStatus::Fresh,
            });
        }
        for _ in 0..50 {
            cx.run_until_parked();
            if observed.len() <= 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            service.subscribe_calls.load(Ordering::SeqCst),
            1,
            "R3: the app-level loop must not create a second bridge"
        );
        assert!(
            observed.len() <= 4,
            "R3: {} events accumulated with no window open",
            observed.len()
        );
        let has_workspace = cx.update(|cx| cx.global::<AppRuntime>().workspace.is_some());
        assert!(!has_workspace, "R3: no window was opened");
    }

    struct FakeTray {
        available: bool,
    }

    impl crate::tray::TrayBackend for FakeTray {
        fn refresh(
            &mut self,
            _model: &crate::tray::TrayMenuModel,
            _views: &[InstanceView],
        ) -> bool {
            self.available
        }
        fn is_available(&self) -> bool {
            self.available
        }
    }
}
