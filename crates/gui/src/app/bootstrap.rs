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
use super::tray_paths::handle_tray_action;

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

    /// Test constructor that spawns the real bridge (so `subscribe` is counted)
    /// with a fake tray, and no display.
    #[cfg(test)]
    pub fn new_for_test_with_bridge(
        service: Arc<dyn DblService>,
        tray: crate::tray::TrayHandle,
    ) -> Self {
        let events = crate::service_bridge::spawn_bridge(service.clone());
        Self::new_for_test(service, tray, events)
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

            start_app_loops(cx);

            if let Err(err) = open_main_window(cx) {
                tracing::error!("could not open the main window: {err:#}");
                request_app_quit(cx);
            }
        });
    Ok(())
}

/// Starts the app-level loops. This is the only place the app wires its event
/// consumers: one bridge, one tray-action pump (R3).
pub fn start_app_loops(cx: &mut App) {
    start_app_event_loop(cx);
    start_tray_action_pump(cx);
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
