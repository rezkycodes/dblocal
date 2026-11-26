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
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeStyled as _, h_flex, v_flex};
use gpui_kit::{
    Div, FocusHandle, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString, Styled,
    Window, div, px, svg,
};

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
        EngineKey::Redis => theme.danger,
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
///
/// A11y: a page can show two badges of one engine (the dock row and the
/// instance header), each as its own subtree, so the label id depends on the
/// size — the only thing telling those badges apart — to stay unique.
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
                .id(SharedString::from(format!(
                    "engine-badge-{}-{size}",
                    engine.as_str()
                )))
                .text_color(colour)
                .text_size(px((size * 0.42).max(9.0)))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD),
        )
}

/// A status chip: the glyph plus the status word.
///
/// A11y: the same status can be drawn more than once in a frame (an instance
/// row, a header pill, a filter result), so the label id carries the status.
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
        .child(
            Label::new(format::status_label(status)).id(SharedString::from(format!(
                "status-pill-{}",
                status_word(status)
            ))),
        )
}

/// Stable id fragment for a status, used where two nodes share a parent.
fn status_word(status: InstanceStatus) -> &'static str {
    match status {
        InstanceStatus::Running => "running",
        InstanceStatus::Stopped => "stopped",
        InstanceStatus::Failed => "failed",
        _ => "busy",
    }
}

/// Small capitalised section label, e.g. `INSTANCES`.
///
/// A11y: a page draws several section headings, all from this one call site.
/// `Label::new` would give them one element id and gpui's duplicate-node
/// assertion would fire, so the id comes from the heading text.
pub fn section_label(text: impl Into<SharedString>) -> Label {
    let text = text.into();
    Label::new(text.clone().to_uppercase())
        .id(SharedString::from(format!("section-{text}")))
        .text_size(px(11.))
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
}

/// A bordered card with an optional heading.
///
/// A11y: callers that draw several cards pass a `key` that tells them apart (a
/// loop index); two cards with the same key, or two title-less cards, would
/// otherwise share one element path, because `Label::new`'s id is the call site.
/// `key` falls back to the heading, then to a fixed name.
pub fn card(title: Option<&str>, key: Option<&str>, theme: &Theme) -> gpui_kit::Stateful<Div> {
    let card_key = key
        .map(str::to_owned)
        .or_else(|| title.map(str::to_owned))
        .unwrap_or_else(|| "card".to_owned());
    let mut base = v_flex()
        .id(SharedString::from(format!("card-{card_key}")))
        .gap_3()
        .p(px(18.))
        .pb(px(18.))
        .rounded(px(8.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover);
    if let Some(title) = title {
        base = base.child(
            Label::new(title.to_owned())
                .id(SharedString::from(format!("card-title-{card_key}")))
                .text_sm()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD),
        );
    }
    base
}

/// A `label — value` row for the detail cards.
///
/// A11y: the row root and its label carry the field name, so the same call
/// site can serve every field of every card without colliding.
pub fn kv(label: &str, value: impl IntoElement, theme: &Theme) -> gpui_kit::Stateful<Div> {
    h_flex()
        .id(SharedString::from(format!("kv-{label}")))
        .gap_3()
        .items_start()
        .child(
            Label::new(label.to_owned())
                .id(SharedString::from(format!("kv-label-{label}")))
                .w(px(130.))
                .flex_none()
                .text_color(theme.muted_foreground),
        )
        .child(div().flex_1().min_w_0().child(value))
}

/// Monospace text in the theme's mono family, used for URIs, ports and logs.
///
/// A11y: the wrapper takes an id from the text so repeated uses stay distinct.
pub fn mono_themed(text: impl Into<SharedString>, theme: &Theme) -> gpui_kit::Stateful<Div> {
    let text = text.into();
    div()
        .id(SharedString::from(format!("mono-{text}")))
        .font_family(theme.mono_font_family.clone())
        .child(Label::new(text))
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

/// Makes a clickable element a keyboard tab stop with a visible focus ring.
///
/// gpui synthesises an `on_click` from Enter/Space on a *focused* element, so a
/// row that already has `on_click` becomes fully keyboard-operable once it holds
/// focus — the missing pieces were a focus handle, `track_focus`, a tab stop,
/// and a ring that is more than a background tint. Applied to a card or row, this
/// turns it into a focusable list option (`F-PLH-01` AC1: reach every choice by
/// Tab, confirm with Enter/Space, and always see where focus is).
///
/// The caller passes a stable `focus` handle (taken once per row from
/// `window.use_keyed_state`) so focus survives a re-render — choosing an engine
/// must not drop the ring back to the first row.
pub fn apply_focus_ring<E: InteractiveElement + Styled + ParentElement + 'static>(
    row: E,
    focus: &FocusHandle,
    window: &Window,
    cx: &gpui_kit::App,
) -> E {
    let focused = focus.contains_focused(window, cx);
    let stop = focus.clone().tab_stop(true);
    let row = row.track_focus(&stop);
    if focused {
        // The theme's own focus appearance: a border in `theme.ring` plus the
        // outside ring. This is the visible focus indicator `F-PLH-01` AC1
        // requires, and it is a ring/outline, not only a background tint.
        row.focus_ring_style(window, cx)
    } else {
        row
    }
}

/// A stable, keyed focus handle for a page row that must survive re-renders.
///
/// Used for the dock's engine-group headers and instance rows and the instance
/// tabs, so Tab reaches them in visual order (`F4`) and the ring persists.
pub fn keyed_focus(
    key: impl Into<gpui_kit::ElementId>,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> FocusHandle {
    window
        .use_keyed_state(key, cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone()
}

/// A focus ring drawn **inside** the element's own bounds, on all four edges.
///
/// `apply_focus_ring` uses gpui-component's `focus_ring_style`, whose ring sits
/// *outside* the border. In a clipped container — the version list has
/// `overflow_hidden`, so a row's border is the only visible edge — that outside
/// ring is cut off and the user sees at most a thin line on one edge (`G2`). This
/// variant paints a 2px `theme.ring` border on every side, inset so it stays
/// within the row and cannot be clipped, so the focused row reads as a full
/// outline regardless of its parent. It replaces the tint-only look for the
/// version rows and engine-group headers.
pub fn apply_inset_focus_ring<E: InteractiveElement + Styled + ParentElement + 'static>(
    row: E,
    focus: &FocusHandle,
    window: &Window,
    cx: &gpui_kit::App,
) -> E {
    let focused = focus.contains_focused(window, cx);
    let stop = focus.clone().tab_stop(true);
    // `relative` so the absolute inset ring below is positioned against this row,
    // not the window.
    let row = row.track_focus(&stop).relative();
    if focused {
        let ring = cx.theme().ring;
        row.child(
            div()
                .absolute()
                .inset_0()
                .border_2()
                .border_color(ring)
                .rounded(px(4.)),
        )
    } else {
        row
    }
}
