//! Small view helpers shared by every screen.
//!
//! The prototype's `ui/components.rs` is the reference: a status glyph that
//! encodes its state by shape (never colour alone), an engine monogram badge, a
//! status pill, a card, a key/value row, a mono span and a coloured banner.
//!
//! Every text node goes through [`Label`], which is what exposes it to assistive
//! technology (Kit design rule A1); a bare `div().child("...")` does not.

use gpui_kit::assets::IconName;
use gpui_kit::component::label::Label;
use gpui_kit::component::{Theme, h_flex, v_flex};
use gpui_kit::{Div, Hsla, IntoElement, ParentElement, SharedString, Styled, div, px, svg};

use dblocal_core::model::{EngineKey, InstanceStatus};

use crate::format::{self, StatusGlyph};
use crate::theme::tokens;

/// The colour a status paints with, from the active theme.
pub fn status_colour(status: InstanceStatus, theme: &Theme) -> Hsla {
    match format::status_glyph(status) {
        StatusGlyph::Running => theme.success,
        StatusGlyph::Failed => theme.danger,
        StatusGlyph::Busy => theme.warning,
        StatusGlyph::Stopped => theme.muted_foreground,
    }
}

/// The colour an engine's monogram paints with.
pub fn engine_colour(engine: EngineKey, theme: &Theme) -> Hsla {
    match engine {
        EngineKey::Postgresql => theme.primary,
        EngineKey::Mysql => theme.chart_3,
        EngineKey::Mariadb => theme.info,
        EngineKey::Valkey => theme.danger,
        _ => theme.muted_foreground,
    }
}

/// An icon at an explicit size and colour.
pub fn icon(name: IconName, size: f32, colour: Hsla) -> Div {
    div()
        .flex_none()
        .child(svg().path(name.path()).size(px(size)).text_color(colour))
}

/// A status glyph whose *shape* also differs per state (`F-PLH-02` AC2).
///
/// running `●`, stopped `○`, failed `▲`, busy `◐`. The render tests sample the
/// colour and the shape separately, and every place that draws one also draws
/// the status label next to it, so colour is never the only signal.
pub fn status_glyph(status: InstanceStatus, size: f32, theme: &Theme) -> Div {
    let colour = status_colour(status, theme);
    let side = px(size);
    match format::status_glyph(status) {
        StatusGlyph::Running => div()
            .flex_none()
            .flex()
            .items_center()
            .child(div().size(side).rounded_full().bg(colour)),
        StatusGlyph::Stopped => div().flex_none().flex().items_center().child(
            div()
                .size(side)
                .rounded_full()
                .border_1()
                .border_color(colour),
        ),
        StatusGlyph::Failed => icon(IconName::TriangleAlert, size + 3.0, colour),
        StatusGlyph::Busy => div().flex_none().flex().items_center().child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .size(side)
                .rounded_full()
                .border_1()
                .border_color(colour)
                .child(div().w(px(size / 2.0)).h_full().rounded_full().bg(colour)),
        ),
    }
}

/// Monogram badge for an engine (`PG`, `MY`, `MA`, `VK`).
pub fn engine_badge(engine: EngineKey, size: f32, theme: &Theme) -> Div {
    let colour = engine_colour(engine, theme);
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(size * 0.2))
        .bg(tokens::tint(colour, 0.16))
        .child(
            Label::new(format::engine_monogram(engine))
                .text_color(colour)
                .text_size(px((size * 0.42).max(9.0)))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD),
        )
}

/// A status chip: the glyph plus the Indonesian word.
pub fn status_pill(status: InstanceStatus, theme: &Theme) -> Div {
    let colour = status_colour(status, theme);
    h_flex()
        .flex_none()
        .gap_1p5()
        .h(px(22.))
        .px(px(9.))
        .rounded_full()
        .bg(tokens::tint(colour, 0.14))
        .text_color(colour)
        .text_xs()
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .child(status_glyph(status, 8.0, theme))
        .child(Label::new(format::status_label(status)))
}

/// Small capitalised section label, e.g. `INSTANCES`.
pub fn section_label(text: impl Into<SharedString>) -> Label {
    Label::new(text.into().to_uppercase())
        .text_size(px(11.))
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
}

/// A bordered card with an optional heading.
pub fn card(title: Option<&str>, theme: &Theme) -> Div {
    let mut base = v_flex()
        .gap_3()
        .p(px(18.))
        .rounded(px(8.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover);
    if let Some(title) = title {
        base = base.child(
            Label::new(title.to_owned())
                .text_sm()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD),
        );
    }
    base
}

/// A `label — value` row for the detail cards.
pub fn kv(label: &str, value: impl IntoElement, theme: &Theme) -> Div {
    h_flex()
        .gap_3()
        .items_start()
        .child(
            Label::new(label.to_owned())
                .w(px(130.))
                .flex_none()
                .text_color(theme.muted_foreground),
        )
        .child(div().flex_1().min_w_0().child(value))
}

/// Monospace text in the theme's mono family, used for URIs, ports and logs.
pub fn mono_themed(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .font_family(theme.mono_font_family.clone())
        .child(Label::new(text.into()))
}

/// A tinted banner: fill, border and whatever the caller adds inside.
pub fn banner(colour: Hsla) -> Div {
    h_flex()
        .flex_wrap()
        .gap_3()
        .items_start()
        .p(px(14.))
        .rounded(px(8.))
        .border_1()
        .border_color(tokens::banner_border(colour))
        .bg(tokens::banner_fill(colour))
}
