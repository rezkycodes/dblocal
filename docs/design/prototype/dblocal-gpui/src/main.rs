//! DBLocal — prototipe UI desktop (GPUI Kit 0.10).
//!
//! Struktur:
//! - `theme`  : tema #15161d dari `themes/dblocal.json` + token warna
//! - `assets` : ikon gpui-kit + ikon Lucide tambahan
//! - `model`  : tipe domain + data contoh (nanti diganti `dblocal-core`)
//! - `ui`     : workspace (dock, tab, overview/logs/databases/config), dialog

mod assets;
mod model;
mod theme;
mod ui;

use gpui_kit::component::TitleBar;
use gpui_kit::{App, AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};

use crate::ui::workspace::{self, Workspace};

fn main() {
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            theme::init(cx);
            workspace::bind_keys(cx);

            let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(900.), px(600.))),
                app_id: Some("dev.dblocal.DBLocal".into()),
                ..TitleBar::window_options()
            };

            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| Workspace::new(window, cx))
            })
            .expect("gagal membuka jendela DBLocal");

            cx.activate(true);
        });
}
