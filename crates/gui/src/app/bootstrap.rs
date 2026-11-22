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
    tray_actions: Option<flume::Receiver<crate::tray::TrayAction>>,
    pub events: flume::Receiver<crate::service_bridge::UiEvent>,
    pub main_window: Option<WindowHandle<Root>>,
    pub workspace: Option<gpui_kit::Entity<Workspace>>,
}

impl Global for AppRuntime {}

impl AppRuntime {
    pub fn new(service: Arc<dyn DblService>) -> Self {
        crate::tray::enable_for_app();
        let events = crate::service_bridge::spawn_bridge(service.clone());
        let tray_start = crate::tray::start_tray();
        Self {
            service,
            tray: tray_start.handle,
            tray_actions: Some(tray_start.actions),
            events,
            main_window: None,
            workspace: None,
        }
    }

    pub fn tray_available(&self) -> bool {
        self.tray.is_available()
    }

    pub fn take_tray_actions(&mut self) -> flume::Receiver<crate::tray::TrayAction> {
        self.tray_actions
            .take()
            .unwrap_or_else(|| flume::unbounded().1)
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
            let service_for_tray = runtime.service.clone();
            cx.set_global(runtime);
            cx.set_quit_mode(if tray_available {
                QuitMode::Explicit
            } else {
                QuitMode::LastWindowClosed
            });

            // App-level tray refresh: keeps tray menu current even with no window.
            let tray_rx = crate::service_bridge::spawn_bridge(service_for_tray.clone());
            let service_for_actions = service_for_tray.clone();
            cx.spawn(async move |cx: &mut gpui_kit::AsyncApp| {
                while let Ok(event) = tray_rx.recv_async().await {
                    let should_refresh = matches!(
                        event,
                        crate::service_bridge::UiEvent::InstanceChanged(_)
                            | crate::service_bridge::UiEvent::InstanceRemoved(_)
                            | crate::service_bridge::UiEvent::Resync
                            | crate::service_bridge::UiEvent::CatalogUpdated
                    );
                    if should_refresh {
                        let svc = service_for_tray.clone();
                        let views = cx
                            .background_spawn(
                                async move { svc.list_instances().unwrap_or_default() },
                            )
                            .await;
                        cx.update(|cx| {
                            if cx.has_global::<AppRuntime>() {
                                cx.update_global::<AppRuntime, _>(|runtime, _| {
                                    runtime.refresh_tray(&views);
                                });
                            }
                        });
                    }
                    if matches!(event, crate::service_bridge::UiEvent::Closed) {
                        break;
                    }
                }
            })
            .detach();

            // App-level tray action pump: forward to workspace when open, else handle.
            let actions_rx = {
                let runtime = cx.global_mut::<AppRuntime>();
                runtime.take_tray_actions()
            };
            cx.spawn(async move |cx: &mut gpui_kit::AsyncApp| {
                while let Ok(action) = actions_rx.recv_async().await {
                    cx.update(|cx| {
                        let ws = cx
                            .try_global::<AppRuntime>()
                            .and_then(|r| r.workspace.clone());
                        if let Some(ws) = ws {
                            ws.update(cx, |this, cx| this.apply_tray_action(action, cx));
                        } else {
                            handle_app_tray_action(action, &service_for_actions, cx);
                        }
                    });
                }
            })
            .detach();

            if let Err(err) = open_main_window(cx) {
                tracing::error!("could not open the main window: {err:#}");
                cx.quit();
            }
        });
    Ok(())
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
    // Focus existing window if one is already open.
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
        });
        cx.new(|cx| Root::new(workspace, window, cx))
    })?;

    cx.update_global::<AppRuntime, _>(|runtime, _| {
        runtime.main_window = Some(handle);
    });
    cx.activate(true);
    Ok(handle)
}

fn handle_app_tray_action(
    action: crate::tray::TrayAction,
    service: &Arc<dyn DblService>,
    cx: &mut App,
) {
    match action {
        crate::tray::TrayAction::OpenWindow => {
            let _ = open_main_window(cx);
        }
        crate::tray::TrayAction::Quit => {
            let ws = cx
                .try_global::<AppRuntime>()
                .and_then(|r| r.workspace.clone());
            if let Some(ws) = ws {
                ws.update(cx, |this, cx| this.request_quit(cx));
            } else {
                if let Ok(_handle) = open_main_window(cx) {
                    if let Some(ws) = cx
                        .try_global::<AppRuntime>()
                        .and_then(|r| r.workspace.clone())
                    {
                        ws.update(cx, |this, cx| this.request_quit(cx));
                    }
                } else {
                    cx.quit();
                }
            }
        }
        crate::tray::TrayAction::Start(id) => {
            if let Ok(id) = id.parse::<dblocal_core::model::InstanceId>() {
                let svc = service.clone();
                cx.background_spawn(async move {
                    let _ = svc.start_instance(&id);
                })
                .detach();
            }
        }
        crate::tray::TrayAction::Stop(id) => {
            if let Ok(id) = id.parse::<dblocal_core::model::InstanceId>() {
                let svc = service.clone();
                cx.background_spawn(async move {
                    let _ = svc.stop_instance(&id);
                })
                .detach();
            }
        }
    }
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
    use super::*;

    #[test]
    fn dark_appearances_are_detected() {
        assert!(appearance_is_dark(WindowAppearance::Dark));
        assert!(appearance_is_dark(WindowAppearance::VibrantDark));
        assert!(!appearance_is_dark(WindowAppearance::Light));
        assert!(!appearance_is_dark(WindowAppearance::VibrantLight));
    }

    #[test]
    fn window_close_decision_is_tray_aware() {
        assert!(window_close_should_close(true));
        assert!(!window_close_should_close(false));
    }

    #[test]
    fn existing_window_is_focused_not_duplicated() {
        assert!(should_focus_existing_main_window(true));
        assert!(!should_focus_existing_main_window(false));
    }
}
