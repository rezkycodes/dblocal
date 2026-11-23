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
use gpui_kit::{Context, IntoElement, ParentElement, Styled, div, px};

use crate::format;

use super::Workspace;
use super::actions;
use super::components::icon;

/// Renders the client-side title bar.
pub(super) fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
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
    let hint = actions::palette_hint();
    let primary = theme.primary;

    TitleBar::new().child(
        h_flex()
            .id("title-bar-content")
            .w_full()
            .gap_3()
            .pr_2()
            .items_center()
            .child(app_mark(primary, theme.primary_foreground))
            .child(
                Label::new(strings::APP_NAME)
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD),
            )
            .when_some(selected, |this, view| {
                let engine = view.engine.display_name();
                this.child(breadcrumb(
                    engine,
                    &view.name,
                    theme.muted_foreground,
                    theme.foreground,
                ))
            })
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(icon(IconName::CircleCheck, 13.0, freshness_colour))
                    .child(Label::new(freshness.to_owned())),
            )
            .child(
                Label::new(hint)
                    .text_xs()
                    .text_color(theme.muted_foreground),
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
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_settings(cx);
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

/// `Engine › instance` for the selected tab.
fn breadcrumb(
    engine: &str,
    name: &str,
    muted: gpui_kit::Hsla,
    bright: gpui_kit::Hsla,
) -> impl IntoElement {
    h_flex()
        .id("breadcrumb")
        .gap_1p5()
        .text_xs()
        .child(Label::new(engine.to_owned()).text_color(muted))
        .child(Label::new(strings::UI_BREADCRUMB_SEPARATOR.to_owned()).text_color(muted))
        .child(Label::new(name.to_owned()).text_color(bright))
}
