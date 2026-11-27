//! The title bar: app mark, name, breadcrumb, catalog freshness, new-instance
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
use gpui_kit::component::{ActiveTheme, Sizable, TitleBar, h_flex};
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
    let primary = theme.primary;

    TitleBar::new()
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
        .child(
            h_flex()
                .id("title-bar-content")
                .w_full()
                .gap_2()
                .pr_2()
                .items_center()
                .child(app_mark(primary, theme.primary_foreground))
                .child(
                    Label::new(strings::APP_NAME)
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                )
                .when_some(selected, |this, view| this.child(breadcrumb(view, &theme)))
                .child(div().flex_1().min_w(px(12.)))
                .child(search_button(&theme, window, cx))
                .child(div().flex_1().min_w(px(12.)))
                .child(
                    design::chip("catalog", freshness.to_owned(), freshness_colour).child(icon(
                        IconName::CircleCheck,
                        12.0,
                        freshness_colour,
                    )),
                )
                .when_some(
                    super::update_dialog::title_bar_button(workspace, cx),
                    |this, button| this.child(button),
                )
                .child(
                    // The Settings dialog's title-bar entry point (`F-PKG-05`,
                    // `F-AUT-01`); `Ctrl/Cmd+,` fires the same action.
                    Button::new("title-settings")
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
                )
                .child(
                    Button::new("title-new")
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
        .flex_none()
        .w(px(340.))
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
                .truncate()
                .text_xs()
                .text_color(theme.muted_foreground),
        )
        .child(design::kbd("title-search", &keys, theme));
    super::components::apply_focus_ring(button, &focus, window, cx)
}

/// The rounded app mark: a database glyph on the accent fill.
fn app_mark(fill: gpui_kit::Hsla, on_fill: gpui_kit::Hsla) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(22.))
        .rounded(px(5.))
        .bg(fill)
        .child(icon(IconName::Database, 14.0, on_fill))
}

/// `/ [icon] Engine / ● instance` for the selected tab, DBDelve style.
fn breadcrumb(
    view: &dblocal_core::service::InstanceView,
    theme: &gpui_kit::component::Theme,
) -> impl IntoElement {
    let muted = theme.muted_foreground;
    h_flex()
        .id("breadcrumb")
        .gap_2()
        .pl_1()
        .text_xs()
        .child(
            Label::new(strings::UI_BREADCRUMB_SEPARATOR)
                .id("crumb-sep-0")
                .text_color(muted),
        )
        .child(engine_icon(view.engine, 14.0, theme))
        .child(
            Label::new(view.engine.display_name().to_owned())
                .id("crumb-engine")
                .text_color(muted),
        )
        .child(
            Label::new(strings::UI_BREADCRUMB_SEPARATOR)
                .id("crumb-sep-1")
                .text_color(muted),
        )
        .child(status_glyph(view.status, 7.0, theme))
        .child(
            Label::new(view.name.clone())
                .id("crumb-instance")
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(theme.foreground),
        )
}
