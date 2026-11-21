//! Process bootstrap: open the window and hand it a `Workspace`.
//!
//! This is the only place that calls into `gpui_kit::platform`; everything else
//! is a view or a view-model, which keeps the process entry easy to audit.

use std::sync::Arc;

use dblocal_core::service::DblService;
use gpui_kit::component::{Root, TitleBar};
use gpui_kit::{
    App, AppContext as _, WindowAppearance, WindowBounds, WindowKind, WindowOptions, px, size,
};

use super::Workspace;

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
            crate::tray::enable_for_app();
            super::actions::bind_keys(cx);

            let bounds = gpui_kit::Bounds::centered(None, size(px(1280.), px(820.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(900.), px(600.))),
                app_id: Some("dev.dblocal.DBLocal".to_owned()),
                ..TitleBar::window_options()
            };
            let _ = WindowKind::Normal;

            let service_for_window = service.clone();
            let result = cx.open_window(options, move |window, cx| {
                let workspace = cx.new(|cx| Workspace::new(service_for_window.clone(), window, cx));
                let weak = workspace.downgrade();
                window.on_window_should_close(cx, move |_window, cx| {
                    if let Some(workspace) = weak.upgrade() {
                        workspace.update(cx, |this, cx| {
                            this.handle_window_close_request(cx);
                        });
                        false
                    } else {
                        true
                    }
                });
                cx.new(|cx| Root::new(workspace, window, cx))
            });

            match result {
                Ok(_handle) => {
                    cx.activate(true);
                }
                Err(err) => {
                    tracing::error!("could not open the main window: {err:#}");
                    cx.quit();
                }
            }
        });
    Ok(())
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
