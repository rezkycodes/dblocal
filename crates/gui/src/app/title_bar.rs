//! The title bar: name, breadcrumb, catalog freshness, new-instance
//! button (D-25 prototype).
//!
//! Window controls are drawn by `gpui_kit::component::TitleBar`'s client-side
//! decorations per platform: Linux and Windows draw their own, macOS keeps the
//! native traffic lights. The tray seam is here too: `tray_available` stays
//! `false` until Wave 2 wires it, and the status bar says so.

use dblocal_core::strings;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, Sizable, TITLE_BAR_HEIGHT, TitleBar, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{Context, IntoElement, ParentElement, SharedString, Styled, Window, div, px};

use crate::format;

use super::Workspace;
use super::actions;
use super::components::{engine_icon, icon, status_glyph};
use super::design;
use crate::theme::tokens;

/// Renders the client-side title bar.
pub(super) fn render(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    TitleBar::new()
        .min_w_0()
        .overflow_hidden()
        .on_close_window(|_, window, cx| {
            let ws = cx
                .try_global::<super::bootstrap::AppRuntime>()
                .and_then(|r| r.workspace.clone());
            match ws {
                Some(workspace) => {
                    if super::bootstrap::handle_window_should_close(workspace.downgrade(), cx) {
                        window.remove_window();
                    }
                }
                None => window.remove_window(),
            }
        })
        .child(render_content(workspace, window, cx))
}

/// Maximum width of the centred command search field at wide window widths.
const SEARCH_MAX_W: f32 = 340.0;
/// Minimum width of the command search field before the breadcrumb truncates.
const SEARCH_MIN_W: f32 = 120.0;
/// Minimum width of each flexible spacer beside the command search field.
const SPACER_MIN_W: f32 = 12.0;
/// `TITLE_BAR_LEFT_PADDING` in `gpui-component`'s `TitleBar` (`80px` on macOS,
/// `12px` on Linux/Windows).
#[cfg(target_os = "macos")]
const KIT_TITLE_BAR_LEFT_PAD: f32 = 80.0;
#[cfg(not(target_os = "macos"))]
const KIT_TITLE_BAR_LEFT_PAD: f32 = 12.0;

/// Width `gpui-component`'s `WindowControls` sibling occupies on the right of
/// `TitleBar` (`ControlIcon` sets `.w(TITLE_BAR_HEIGHT)` per visible button).
pub(super) fn client_window_controls_width(window: &Window) -> f32 {
    if cfg!(target_os = "macos") || cfg!(target_family = "wasm") {
        return 0.0;
    }
    #[cfg(target_os = "linux")]
    if !matches!(
        window.window_decorations(),
        gpui_kit::Decorations::Client { .. }
    ) {
        return 0.0;
    }
    let supported = window.window_controls();
    let count = 1usize + usize::from(supported.minimize) + usize::from(supported.maximize);
    TITLE_BAR_HEIGHT.as_f32() * count as f32
}

/// Renders the DBLocal title-bar content row inside `TitleBar`'s `bar` slot.
///
/// `gpui-component`'s `TitleBar` places our child inside an internal `bar` flex
/// item (`flex_1` + `flex_shrink_0` + `flex_basis: 0`) before `WindowControls`
/// (`flex_shrink_0`, up to `3 * TITLE_BAR_HEIGHT` on Linux/Windows). Because
/// `bar` has `flex_shrink_0`, Taffy clamps its base size to its automatic
/// `min-content` width, so a `w_full()` child (`width: auto` during intrinsic
/// sizing) reports the unshrunk sum of all children (`~1049px`) and pushes
/// `WindowControls` off the right edge! Setting an explicit `max_w` from the
/// viewport minus `KIT_TITLE_BAR_LEFT_PAD` and `WindowControls` bounds `bar`'s
/// intrinsic width while letting inner flex items shrink cleanly.
pub(super) fn render_content(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    render_content_for_controls(workspace, client_window_controls_width(window), window, cx)
}

/// Renders the title-bar content row, reserving `controls_w` logical pixels on
/// the right for `gpui-component`'s `WindowControls` sibling.
pub(super) fn render_content_for_controls(
    workspace: &Workspace,
    controls_w: f32,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let max_bar_w =
        (window.viewport_size().width.as_f32() - KIT_TITLE_BAR_LEFT_PAD - controls_w).max(0.0);
    let theme = cx.theme().clone();
    let selected = workspace
        .tabs
        .selected_instance()
        .and_then(|id| workspace.instance(id));
    let freshness = format::catalog_freshness_word(workspace.catalog().status);
    let freshness_colour = match workspace.catalog().status {
        dblocal_core::model::CatalogStatus::Fresh => theme.success,
        dblocal_core::model::CatalogStatus::Stale => theme.warning,
        _ => theme.danger,
    };

    h_flex()
        .id("title-bar-content")
        .debug_selector(|| "title-bar-content".to_owned())
        .flex_1()
        .w_full()
        .max_w(px(max_bar_w))
        .min_w_0()
        .overflow_hidden()
        .gap_2()
        .pr_2()
        .items_center()
        .child(
            Label::new(strings::APP_NAME)
                .flex_shrink_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD),
        )
        .when_some(selected, |this, view| this.child(breadcrumb(view, &theme)))
        .child(div().flex_1().min_w(px(SPACER_MIN_W)))
        .child(search_button(&theme, window, cx))
        .child(div().flex_1().min_w(px(SPACER_MIN_W)))
        .child(
            design::chip("catalog", freshness.to_owned(), freshness_colour)
                .debug_selector(|| "title-catalog-chip".to_owned())
                .child(icon(IconName::CircleCheck, 12.0, freshness_colour)),
        )
        .when_some(
            super::update_dialog::title_bar_button(workspace, cx),
            |this, button| this.child(div().flex_none().child(button)),
        )
        .child(
            div().flex_none().child(
                // The Settings dialog's title-bar entry point (`F-PKG-05`,
                // `F-AUT-01`); `Ctrl/Cmd+,` fires the same action.
                Button::new("title-settings")
                    .debug_selector(|| "title-settings".to_owned())
                    .ghost()
                    .small()
                    .compact()
                    .icon(IconName::Settings)
                    .tooltip(strings::UI_SETTINGS)
                    .accessibility_label(strings::UI_SETTINGS)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings(window, cx);
                        cx.notify();
                    })),
            ),
        )
        .child(
            div().flex_none().child(
                Button::new("title-new")
                    .debug_selector(|| "title-new".to_owned())
                    .ghost()
                    .small()
                    .compact()
                    .icon(IconName::Plus)
                    .tooltip(strings::UI_NEW_INSTANCE)
                    .accessibility_label(strings::UI_NEW_INSTANCE)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_new_instance(window, cx);
                    })),
            ),
        )
}

/// The command search field in the middle of the title bar. It is a button:
/// a click (or Enter/Space when focused) opens the command palette, the same
/// as `Ctrl/Cmd+K`.
///
/// Sized with `w(340)`, `max_w(340)`, `min_w(120)` and a higher shrink factor so
/// it stays 340px and centred between the `flex_1` spacers at wide widths, and
/// shrinks first down to 120px before the breadcrumb truncates on narrow windows.
fn search_button(
    theme: &gpui_kit::component::Theme,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let keys = [actions::modifier_label(), "K"];
    let focus =
        super::components::keyed_focus(SharedString::from("title-search-focus"), window, cx);
    let button = h_flex()
        .id("title-search")
        .debug_selector(|| "title-search".to_owned())
        .w(px(SEARCH_MAX_W))
        .max_w(px(SEARCH_MAX_W))
        .min_w(px(SEARCH_MIN_W))
        .flex_shrink(4.0)
        .h(px(24.))
        .px(px(8.))
        .gap_2()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(theme.border)
        .bg(tokens::hover(theme))
        .cursor_pointer()
        .aria_label(strings::UI_TITLE_SEARCH)
        .hover(|s| s.bg(tokens::active(theme)))
        .on_click(cx.listener(|this, _, window, cx| {
            this.toggle_palette_in(window, cx);
            cx.notify();
        }))
        .child(icon(IconName::Search, 13.0, theme.muted_foreground))
        .child(
            Label::new(strings::UI_TITLE_SEARCH)
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(theme.muted_foreground),
        )
        .child(design::kbd("title-search", &keys, theme));
    super::components::apply_focus_ring(button, &focus, window, cx)
}

/// `/ [icon] Engine / ● instance` for the selected tab, DBDelve style.
fn breadcrumb(
    view: &dblocal_core::service::InstanceView,
    theme: &gpui_kit::component::Theme,
) -> impl IntoElement {
    let muted = theme.muted_foreground;
    h_flex()
        .id("breadcrumb")
        .debug_selector(|| "title-breadcrumb".to_owned())
        .flex_shrink_1()
        .min_w_0()
        .overflow_hidden()
        .gap_2()
        .pl_1()
        .text_xs()
        .child(
            Label::new(strings::UI_BREADCRUMB_SEPARATOR)
                .id("crumb-sep-0")
                .flex_none()
                .text_color(muted),
        )
        .child(engine_icon(view.engine, 14.0, theme))
        .child(
            Label::new(view.engine.display_name().to_owned())
                .id("crumb-engine")
                .flex_shrink_1()
                .min_w_0()
                .truncate()
                .text_color(muted),
        )
        .child(
            Label::new(strings::UI_BREADCRUMB_SEPARATOR)
                .id("crumb-sep-1")
                .flex_none()
                .text_color(muted),
        )
        .child(status_glyph(view.status, 7.0, theme))
        .child(
            Label::new(view.name.clone())
                .id("crumb-instance")
                .flex_shrink(2.0)
                .min_w_0()
                .truncate()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(theme.foreground),
        )
}
