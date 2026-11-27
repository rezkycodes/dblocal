//! Layout regression tests for the client-side title bar at the 900px minimum
//! window width, accounting for the Linux/Windows `WindowControls` sibling.
//!
//! `gpui-component`'s `TitleBar` renders:
//! `div("title-bar").ps(12px).child(h_flex().id("bar").flex_shrink_0().flex_1().child(our_content)).child(WindowControls)`.
//! On Linux (client-side decorations) and Windows, `WindowControls` draws three
//! `ControlIcon` buttons (minimize, maximize/restore, close), each given
//! `.w(TITLE_BAR_HEIGHT).flex_shrink_0()` (`gpui-component-0.10.0/src/title_bar.rs`).
//! Because `TestWindow::window_decorations()` returns `Decorations::Server`,
//! headless tests draw an empty `WindowControls` and never catch `bar` pushing
//! the window controls off the right edge.
//!
//! This harness reproduces `TitleBar`'s exact flex container at `900px` with a
//! `3 * TITLE_BAR_HEIGHT` non-shrinking `WindowControls` sibling, the longest
//! engine name (`PostgreSQL`), a max-length (31-char) instance name, the widest
//! catalog freshness label (`Catalog up to date`), and the optional update
//! button visible.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use dblocal_core::model::{
    AuthMode, CatalogStatus, EngineKey, Instance, InstanceId, InstanceName, InstanceSpec,
    InstanceStatus, Port,
};
use dblocal_core::service::{DblService, InstanceView, mock::MockService};
use gpui_kit::component::{Root, TITLE_BAR_HEIGHT, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    Entity, IntoElement, ParentElement, Pixels, Render, Styled, TestAppContext, VisualTestContext,
    Window, div, px, size,
};

use super::Workspace;

/// Minimum window width configured in `bootstrap.rs`.
const MIN_WINDOW_WIDTH: f32 = 900.0;
/// `TITLE_BAR_LEFT_PADDING` on non-macOS in `gpui-component-0.10.0/src/title_bar.rs`.
const TITLE_BAR_LEFT_PAD: f32 = 12.0;
/// Number of window control buttons on Linux/Windows (minimize, maximize, close).
const CONTROL_COUNT: f32 = 3.0;

/// Width of the three client-side window control buttons, derived from the kit's
/// `TITLE_BAR_HEIGHT` (`ControlIcon::render` sets `.w(TITLE_BAR_HEIGHT)`).
fn controls_width() -> Pixels {
    px(TITLE_BAR_HEIGHT.as_f32() * CONTROL_COUNT)
}

/// Wrapper reproducing `gpui_component::TitleBar`'s exact flex row with
/// client-side `WindowControls` present.
struct SimulatedClientTitleBar {
    workspace: Entity<Workspace>,
}

impl Render for SimulatedClientTitleBar {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl IntoElement {
        let controls_w = controls_width();
        let content = self.workspace.update(cx, |ws, cx| {
            super::title_bar::render_content_for_controls(ws, controls_w.as_f32(), window, cx)
                .into_any_element()
        });

        div()
            .id("sim-window")
            .w_full()
            .h_full()
            .overflow_hidden()
            .child(
                // Exact mirror of `gpui_component::TitleBar::render`:
                // `div().id("title-bar").flex().flex_row().items_center().justify_between().h(TITLE_BAR_HEIGHT).pl(12px)`
                h_flex()
                    .id("sim-title-bar")
                    .debug_selector(|| "sim-title-bar".to_owned())
                    .w_full()
                    .justify_between()
                    .h(TITLE_BAR_HEIGHT)
                    .pl(px(TITLE_BAR_LEFT_PAD))
                    .child(
                        // Exact mirror of `TitleBar`'s `"bar"` slot:
                        // `h_flex().id("bar").h_full().justify_between().flex_shrink_0().flex_1()`
                        h_flex()
                            .id("sim-bar")
                            .debug_selector(|| "sim-bar".to_owned())
                            .h_full()
                            .justify_between()
                            .flex_shrink_0()
                            .flex_1()
                            .child(content),
                    )
                    .child(
                        // Exact mirror of `WindowControls` with minimize + maximize + close.
                        h_flex()
                            .id("sim-window-controls")
                            .debug_selector(|| "sim-window-controls".to_owned())
                            .h_full()
                            .w(controls_w)
                            .flex_shrink_0(),
                    ),
            )
    }
}

fn setup_worst_case_title_bar(
    cx: &mut TestAppContext,
    window_width: f32,
) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service: Arc<dyn DblService> = Arc::new(MockService::demo());
    let holder: std::rc::Rc<std::cell::RefCell<Option<Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let holder_in = holder.clone();

    let (_view, cx) = cx.add_window_view(move |window, cx| {
        let workspace = cx.new(|cx| {
            let mut ws = Workspace::preview_update_available(service, window, cx);
            // Widest catalog label: "Catalog up to date" (18 chars vs 16/15).
            ws.catalog.status = CatalogStatus::Fresh;
            // Longest engine name (`PostgreSQL`, 10 chars) + max-length (31-char)
            // wide instance name (`w`s and hyphens).
            let long_name = "wwwwwwwwwwwwwww-wwwwwwwwwwwwwww";
            assert_eq!(long_name.len(), 31);
            let spec = InstanceSpec::new(
                InstanceName::new(long_name).expect("valid 31-char name"),
                EngineKey::Postgresql,
                "17.4",
                Some(Port::new(5432).expect("port")),
                AuthMode::Password,
            )
            .expect("spec");
            let id = InstanceId::new();
            let inst = Instance::new(
                &spec,
                id.clone(),
                InstanceStatus::Running,
                std::time::SystemTime::UNIX_EPOCH,
            );
            ws.state.upsert_instance(InstanceView::from_instance(&inst));
            ws.open_tab_sync(id);
            ws
        });
        *holder_in.borrow_mut() = Some(workspace.clone());
        let sim = cx.new(|_| SimulatedClientTitleBar { workspace });
        Root::new(sim, window, cx)
    });

    cx.simulate_resize(size(px(window_width), px(600.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    let ws = holder.borrow().clone().expect("workspace built");
    (ws, cx)
}

#[gpui_kit::test]
fn window_controls_and_title_bar_buttons_fit_at_900px_minimum_width(cx: &mut TestAppContext) {
    let (_ws, cx) = setup_worst_case_title_bar(cx, MIN_WINDOW_WIDTH);

    let wrapper_max_right = MIN_WINDOW_WIDTH - controls_width().as_f32();
    let bar = cx.debug_bounds("sim-bar").expect("sim-bar bounds");
    let content = cx
        .debug_bounds("title-bar-content")
        .expect("title-bar-content bounds");
    let controls = cx
        .debug_bounds("sim-window-controls")
        .expect("sim-window-controls bounds");
    let search = cx
        .debug_bounds("title-search")
        .expect("title-search bounds");
    let chip = cx
        .debug_bounds("title-catalog-chip")
        .expect("title-catalog-chip bounds");
    let update = cx
        .debug_bounds("title-update")
        .expect("title-update bounds");
    let settings = cx
        .debug_bounds("title-settings")
        .expect("title-settings bounds");
    let new_btn = cx.debug_bounds("title-new").expect("title-new bounds");

    eprintln!(
        "TITLEBAR-900 wrapper_max_right={wrapper_max_right:.1} \
         bar=({:.1}..{:.1}, w={:.1}) content=({:.1}..{:.1}, w={:.1}) \
         search=({:.1}..{:.1}, w={:.1}) chip=({:.1}..{:.1}) \
         update=({:.1}..{:.1}) settings=({:.1}..{:.1}) new=({:.1}..{:.1}) \
         controls=({:.1}..{:.1}, w={:.1})",
        bar.left().as_f32(),
        bar.right().as_f32(),
        bar.size.width.as_f32(),
        content.left().as_f32(),
        content.right().as_f32(),
        content.size.width.as_f32(),
        search.left().as_f32(),
        search.right().as_f32(),
        search.size.width.as_f32(),
        chip.left().as_f32(),
        chip.right().as_f32(),
        update.left().as_f32(),
        update.right().as_f32(),
        settings.left().as_f32(),
        settings.right().as_f32(),
        new_btn.left().as_f32(),
        new_btn.right().as_f32(),
        controls.left().as_f32(),
        controls.right().as_f32(),
        controls.size.width.as_f32(),
    );

    // Every interactive element and the content row itself must stay within the
    // budget left after the 3 window controls (`900 - 3 * TITLE_BAR_HEIGHT`).
    for (name, b) in [
        ("title-bar-content", content),
        ("sim-bar", bar),
        ("title-search", search),
        ("title-catalog-chip", chip),
        ("title-update", update),
        ("title-settings", settings),
        ("title-new", new_btn),
    ] {
        assert!(
            b.right().as_f32() <= wrapper_max_right + 0.5,
            "{name} right edge {:.1} exceeds available bar right {:.1} (pushes window controls off-screen)",
            b.right().as_f32(),
            wrapper_max_right,
        );
    }

    // And the window controls themselves must sit completely inside the 900px window!
    assert!(
        controls.right().as_f32() <= MIN_WINDOW_WIDTH + 0.5,
        "window controls pushed off-screen: right={:.1} > {MIN_WINDOW_WIDTH}",
        controls.right().as_f32()
    );
    assert!(
        search.size.width.as_f32() >= 120.0 - 0.5,
        "search button must retain at least its 120px minimum width (got {:.1})",
        search.size.width.as_f32()
    );
}

#[gpui_kit::test]
fn search_field_keeps_full_340px_width_and_centres_at_1280px(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service: Arc<dyn DblService> = Arc::new(MockService::demo());
    let (_view, cx) = cx.add_window_view(move |window, cx| {
        let workspace = cx.new(|cx| Workspace::preview(service, window, cx));
        let sim = cx.new(|_| SimulatedClientTitleBar { workspace });
        Root::new(sim, window, cx)
    });

    cx.simulate_resize(size(px(1280.0), px(820.0)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });

    let search = cx
        .debug_bounds("title-search")
        .expect("title-search bounds");
    let controls = cx
        .debug_bounds("sim-window-controls")
        .expect("sim-window-controls bounds");
    eprintln!(
        "TITLEBAR-1280 search=({:.1}..{:.1}, w={:.1}) controls=({:.1}..{:.1})",
        search.left().as_f32(),
        search.right().as_f32(),
        search.size.width.as_f32(),
        controls.left().as_f32(),
        controls.right().as_f32(),
    );
    assert!(
        (search.size.width.as_f32() - 340.0).abs() < 1.0,
        "at 1280px the search field must keep its full 340px width (got {:.1})",
        search.size.width.as_f32()
    );
    assert!(
        (controls.right().as_f32() - 1280.0).abs() < 1.0,
        "at 1280px window controls sit flush at 1280px (got {:.1})",
        controls.right().as_f32()
    );
}
