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

    pub fn restore_tray_actions(&mut self, rx: flume::Receiver<crate::tray::TrayAction>) {
        self.tray_actions = Some(rx);
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
            // Keep the process alive when the last window closes if a tray is
            // present; on Linux cx.hide() is not implemented so closing the window
            // must not quit the app.
            cx.set_quit_mode(if tray_available {
                QuitMode::Explicit
            } else {
                QuitMode::LastWindowClosed
            });

            if let Err(err) = open_main_window(cx) {
                tracing::error!("could not open the main window: {err:#}");
                cx.quit();
            }
        });
    Ok(())
}

pub(crate) fn open_main_window(cx: &mut App) -> anyhow::Result<WindowHandle<Root>> {
    // Focus existing window if one is already open.
    if let Some(handle) = cx.try_global::<AppRuntime>().and_then(|r| r.main_window) {
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
        let weak = workspace.downgrade();
        window.on_window_should_close(cx, move |_window, cx| {
            if let Some(workspace) = weak.upgrade() {
                workspace.update(cx, |this, cx| {
                    this.handle_window_close_request(cx);
                });
                let has_tray = cx.try_global::<AppRuntime>().is_some_and(|r| r.tray_available());
                if has_tray {
                    return true; // close window; process stays alive (QuitMode::Explicit)
                }
                return false; // quit flow decides
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

pub(crate) fn clear_main_window(cx: &mut App) {
    if cx.has_global::<AppRuntime>() {
        cx.update_global::<AppRuntime, _>(|runtime, _| {
            runtime.main_window = None;
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
}
