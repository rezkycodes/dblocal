//! Small elements reused across the workspace.

use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Div, Rgba, SharedString, div, px, svg};

use crate::model::{EngineKind, Status};
use crate::theme::tokens;

/// A status dot. The shape also differs (not just the colour) per F-PLH-02:
/// ● running · ○ stopped · ▲ failed · ◐ sibuk.
pub fn status_dot(status: Status, size: f32) -> AnyElement {
    let color = status.color();
    let s = px(size);
    match status {
        Status::Running => div().size(s).rounded_full().bg(color).into_any_element(),
        Status::Stopped => div()
            .size(s)
            .rounded_full()
            .border_1()
            .border_color(color)
            .into_any_element(),
        Status::Failed => svg()
            .path("icons/triangle-alert.svg")
            .size(px(size + 3.))
            .text_color(color)
            .into_any_element(),
        _ => div()
            .size(s)
            .rounded_full()
            .border_1()
            .border_color(color)
            .child(div().w(px(size / 2.)).h_full().rounded_l_full().bg(color))
            .into_any_element(),
    }
}

/// Monogram engine berwarna (PG, VK, MY, MA).
pub fn engine_badge(engine: EngineKind, size: f32) -> Div {
    let color = engine.color();
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(size * 0.2))
        .bg(tokens::tint(color, 0.16))
        .text_color(color)
        .text_size(px((size * 0.42).max(9.)))
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .child(engine.mono())
}

/// A status pill with a text label.
pub fn status_pill(status: Status) -> Div {
    let color = status.color();
    h_flex()
        .gap_1p5()
        .h(px(22.))
        .px(px(9.))
        .rounded_full()
        .bg(tokens::tint(color, 0.14))
        .text_color(color)
        .text_xs()
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .child(status_dot(status, 8.))
        .child(status.label())
}

/// Label seksi huruf kapital kecil.
pub fn section_label(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(tokens::text_muted())
        .child(text.into().to_uppercase())
}

/// A bordered card with an optional title.
pub fn card(title: Option<&'static str>) -> Div {
    v_flex()
        .gap_3()
        .p(px(18.))
        .rounded(px(8.))
        .border_1()
        .border_color(tokens::border())
        .bg(tokens::bg_panel())
        .when_some(title, |this, title| {
            this.child(
                div()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(title),
            )
        })
}

/// A "label — value" row for detail cards.
pub fn kv(label: &'static str, value: impl IntoElement) -> Div {
    h_flex()
        .gap_3()
        .items_start()
        .child(
            div()
                .w(px(130.))
                .flex_none()
                .text_color(tokens::text_muted())
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(value))
}

/// Monospace text using the theme mono font.
pub fn mono(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .font_family(cx.theme().mono_font_family.clone())
        .child(text.into())
}

/// Banner berwarna (peringatan / error / info).
pub fn banner(color: Rgba) -> Div {
    h_flex()
        .flex_wrap()
        .gap_3()
        .items_start()
        .p(px(14.))
        .rounded(px(8.))
        .border_1()
        .border_color(tokens::tint(color, 0.45))
        .bg(tokens::tint(color, 0.08))
}
