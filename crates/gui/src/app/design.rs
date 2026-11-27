//! Design primitives for the refreshed DBLocal look.
//!
//! The visual language follows DBDelve: calm grey planes, grouped cards whose
//! rows are split by hairlines, small uppercase section headings outside the
//! card, rounded icon tiles, and keyboard hints drawn as key caps. Every helper
//! paints from the active theme (or [`crate::theme::tokens`]), so no view needs a
//! colour literal and both themes keep the 4.5:1 contrast gate.
//!
//! A11y: every visible text goes through [`Label`] with an id derived from the
//! caller's key, so the same helper can be drawn many times in one frame
//! without tripping gpui's duplicate-node assertion.

use gpui_kit::assets::IconName;
use gpui_kit::component::label::Label;
use gpui_kit::component::{Theme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{Div, Hsla, IntoElement, ParentElement, SharedString, Stateful, Styled, div, px};

use crate::theme::tokens;

use super::components::icon;

/// Corner radius of a grouped card.
pub const GROUP_RADIUS: f32 = 10.0;
/// Horizontal padding inside a grouped row.
pub const ROW_PAD_X: f32 = 16.0;
/// Minimum height of a grouped row.
pub const ROW_MIN_H: f32 = 36.0;
/// Width of the label column in a grouped row.
pub const ROW_LABEL_W: f32 = 128.0;

/// Key caps for a shortcut, e.g. `["Ctrl", "K"]`.
pub fn kbd(key: &str, keys: &[&str], theme: &Theme) -> Div {
    let mut row = h_flex().flex_none().gap_0p5();
    for (index, cap) in keys.iter().enumerate() {
        row = row.child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .min_w(px(18.))
                .h(px(18.))
                .px(px(5.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme.input)
                .bg(tokens::hover(theme))
                .child(
                    Label::new((*cap).to_owned())
                        .id(SharedString::from(format!("kbd-{key}-{index}")))
                        .text_size(px(10.5))
                        .text_color(theme.muted_foreground),
                ),
        );
    }
    row
}

/// A rounded square holding an icon: the leading mark of launcher rows,
/// dialog headers and stat tiles.
pub fn icon_tile(name: IconName, size: f32, colour: Hsla, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px((size * 0.26).max(5.0)))
        .border_1()
        .border_color(theme.border)
        .bg(tokens::hover(theme))
        .child(icon(name, (size * 0.5).round(), colour))
}

/// The small uppercase heading that sits *above* a grouped card.
pub fn group_heading(text: &str, theme: &Theme) -> Label {
    Label::new(text.to_uppercase())
        .id(SharedString::from(format!("group-heading-{text}")))
        .text_size(px(11.))
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(theme.muted_foreground)
        .pl(px(4.))
}

/// A grouped card: card plane, one hairline border, rows split by hairlines.
///
/// Children are expected to be [`group_row`]s (or anything that draws its own
/// top border through [`row_divider`]).
pub fn group_card(key: &str, theme: &Theme) -> Stateful<Div> {
    v_flex()
        .id(SharedString::from(format!("group-card-{key}")))
        .rounded(px(GROUP_RADIUS))
        .border_1()
        .border_color(theme.border)
        .bg(tokens::card(theme))
        .overflow_hidden()
}

/// A labelled row inside a [`group_card`]: muted label on the left, the value
/// in the text colour on the right of it. `first` drops the top hairline.
pub fn group_row(
    key: &str,
    label: &str,
    value: impl IntoElement,
    first: bool,
    theme: &Theme,
) -> Stateful<Div> {
    h_flex()
        .id(SharedString::from(format!("group-row-{key}")))
        .min_h(px(ROW_MIN_H))
        .px(px(ROW_PAD_X))
        .py(px(7.))
        .gap_3()
        .items_center()
        .text_sm()
        .when(!first, |row| {
            row.border_t_1().border_color(tokens::hairline(theme))
        })
        .child(
            Label::new(label.to_owned())
                .id(SharedString::from(format!("group-row-label-{key}")))
                .w(px(ROW_LABEL_W))
                .flex_none()
                .text_color(theme.muted_foreground),
        )
        .child(div().flex_1().min_w_0().child(value))
}

/// A row with a title and a description on the left and a control on the
/// right — the DBDelve "setting" row used by the Settings dialog.
pub fn setting_row(
    key: &str,
    title: &str,
    description: Option<&str>,
    control: impl IntoElement,
    first: bool,
    theme: &Theme,
) -> Stateful<Div> {
    h_flex()
        .id(SharedString::from(format!("setting-row-{key}")))
        .min_h(px(52.))
        .px(px(ROW_PAD_X))
        .py(px(10.))
        .gap_4()
        .items_center()
        .when(!first, |row| {
            row.border_t_1().border_color(tokens::hairline(theme))
        })
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    Label::new(title.to_owned())
                        .id(SharedString::from(format!("setting-title-{key}")))
                        .font_weight(gpui_kit::FontWeight::MEDIUM),
                )
                .when_some(description, |col, text| {
                    col.child(
                        Label::new(text.to_owned())
                            .id(SharedString::from(format!("setting-body-{key}")))
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    )
                }),
        )
        .child(div().flex_none().child(control))
}

/// One tile of the Overview stat strip: an icon tile, a muted caption and the
/// value underneath.
pub fn stat_tile(
    key: &str,
    glyph: IconName,
    caption: &str,
    value: impl IntoElement,
    accent: Hsla,
    theme: &Theme,
) -> Stateful<Div> {
    h_flex()
        .id(SharedString::from(format!("stat-{key}")))
        .flex_1()
        .min_w(px(170.))
        .gap_3()
        .p(px(12.))
        .rounded(px(GROUP_RADIUS))
        .border_1()
        .border_color(theme.border)
        .bg(tokens::card(theme))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(32.))
                .rounded(px(8.))
                .bg(tokens::tint(accent, 0.14))
                .child(icon(glyph, 16.0, accent)),
        )
        .child(
            v_flex()
                .min_w_0()
                .gap_0p5()
                .child(
                    Label::new(caption.to_owned())
                        .id(SharedString::from(format!("stat-caption-{key}")))
                        .text_xs()
                        .text_color(theme.muted_foreground),
                )
                .child(div().min_w_0().truncate().child(value)),
        )
}

/// A tinted pill with a word in a semantic colour.
pub fn chip(key: &str, text: impl Into<SharedString>, colour: Hsla) -> Stateful<Div> {
    h_flex()
        .id(SharedString::from(format!("chip-{key}")))
        .flex_none()
        .h(px(20.))
        .px(px(8.))
        .gap_1()
        .rounded_full()
        .bg(tokens::badge_fill(colour))
        .text_xs()
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .text_color(colour)
        .child(Label::new(text.into()).id(SharedString::from(format!("chip-label-{key}"))))
}

/// A dialog header: icon tile, title, a muted subtitle, then whatever the
/// caller appends on the right (usually a close button).
pub fn dialog_header(
    key: &str,
    glyph: IconName,
    title: &str,
    subtitle: Option<&str>,
    theme: &Theme,
) -> Div {
    h_flex()
        .gap_3()
        .items_center()
        .px(px(20.))
        .pt(px(18.))
        .pb(px(14.))
        .child(icon_tile(glyph, 36.0, theme.foreground, theme))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    Label::new(title.to_owned())
                        .id(SharedString::from(format!("dialog-title-{key}")))
                        .text_size(px(15.))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                )
                .when_some(subtitle, |col, text| {
                    col.child(
                        Label::new(text.to_owned())
                            .id(SharedString::from(format!("dialog-subtitle-{key}")))
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    )
                }),
        )
}

/// A thin vertical rule between status-bar groups.
pub fn vrule(theme: &Theme) -> Div {
    div().flex_none().w(px(1.)).h(px(12.)).bg(theme.border)
}

// The scale the design direction asks for, checked at compile time.
const _: () = {
    assert!(ROW_MIN_H >= 36.0, "rows must stay comfortably clickable");
    assert!(
        ROW_LABEL_W >= 120.0,
        "label column is at least the old kv width"
    );
    assert!(GROUP_RADIUS > 8.0, "group cards are rounder than controls");
};
