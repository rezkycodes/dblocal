//! The Config tab (`F-CFG`, Phase 3): an honest placeholder in the product
//! build, the prototype layout with sample rows only in the demo build (`D-25`).
//!
//! Nothing here has a backend: there is no `config.read` in the frozen
//! `DblService`. Every control is disabled and the header names the phase, so
//! the tab can never look like it saved something.

use dblocal_core::model::EngineKey;
use dblocal_core::strings;
use gpui_kit::assets::IconName;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, Theme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, div, px};

use super::Workspace;
use super::page;
use crate::app::components::{card, icon, section_label};
use crate::format::{self, PlaceholderCard, PlaceholderTab};

impl Workspace {
    pub(crate) fn render_config(
        &self,
        view: &dblocal_core::service::InstanceView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        if !self.demo_content() {
            return placeholder_page(
                PlaceholderTab::Config,
                IconName::SlidersHorizontal,
                strings::UI_SUBTAB_CONFIG,
                &theme,
            );
        }
        page()
            .child(demo_config_table(view, &theme))
            .into_any_element()
    }
}

/// The honest "no backend yet" state: the prototype's card structure, every
/// control disabled, and the "(Fase N)" note in the header. No sample rows.
pub(crate) fn placeholder_page(
    tab: PlaceholderTab,
    glyph: IconName,
    title: &str,
    theme: &Theme,
) -> AnyElement {
    let mut content = page();
    content = content.child(placeholder_header(
        glyph,
        title,
        format::placeholder_phase_label(tab),
        theme,
    ));
    for (index, card_spec) in format::placeholder_layout(tab).into_iter().enumerate() {
        content = content.child(placeholder_card(tab, index, &card_spec, theme));
    }
    content.into_any_element()
}

/// The banner the placeholder page starts with: icon, tab name, phase.
fn placeholder_header(
    glyph: IconName,
    title: &str,
    phase: &str,
    theme: &Theme,
) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(icon(glyph, 20.0, theme.muted_foreground))
        .child(
            Label::new(title.to_owned())
                .text_sm()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD),
        )
        .child(
            Label::new(strings::placeholder_phase(phase))
                .text_xs()
                .text_color(theme.muted_foreground),
        )
}

/// One placeholder card, rendered from the pure descriptor so the view and the
/// view-model test cannot drift.
fn placeholder_card(
    tab: PlaceholderTab,
    index: usize,
    spec: &PlaceholderCard,
    theme: &Theme,
) -> impl IntoElement {
    let mut card_box = card(spec.title, Some(&format!("placeholder-{index}")), theme);
    if !spec.controls.is_empty() {
        let mut row = h_flex().gap_2().items_center().flex_wrap();
        for (control_index, control) in spec.controls.iter().enumerate() {
            row = row.child(placeholder_control(tab, index, control_index, control));
        }
        card_box = card_box.child(row);
    }

    if let Some(columns) = spec.columns {
        card_box = card_box.child(
            Label::new(columns.to_owned())
                .text_xs()
                .text_color(theme.muted_foreground),
        );
    }
    if let Some(body) = spec.body {
        card_box = card_box.child(
            Label::new(body.to_owned())
                .text_sm()
                .text_color(theme.muted_foreground),
        );
    }
    if let Some(empty) = spec.empty {
        card_box = card_box.child(
            div()
                .p(px(16.))
                .rounded(px(6.))
                .bg(theme.background)
                .border_1()
                .border_color(theme.table_row_border)
                .child(
                    Label::new(empty.to_owned())
                        .text_xs()
                        .text_color(theme.muted_foreground),
                ),
        );
    }
    card_box
}

/// One disabled control: the placeholder only ever draws buttons now that
/// Config has no field (`W4-B`).
fn placeholder_control(
    tab: PlaceholderTab,
    card_index: usize,
    control_index: usize,
    control: &format::PlaceholderControl,
) -> AnyElement {
    let id = format!("placeholder-{tab:?}-{card_index}-{control_index}");
    Button::new(id)
        .label(control.label)
        .disabled(control.disabled)
        .into_any_element()
}

/// The demo Config layout: current value beside the default.
fn demo_config_table(
    view: &dblocal_core::service::InstanceView,
    theme: &Theme,
) -> impl IntoElement {
    let mono = theme.mono_font_family.clone();
    let port = view.port.to_string();
    let params: Vec<(&'static str, String, &'static str, &'static str)> = match view.engine {
        EngineKey::Postgresql => vec![
            ("port", port, "5432", "Diuji bind sebelum disimpan"),
            ("max_connections", "200".into(), "100", "Butuh restart"),
            ("shared_buffers", "512MB".into(), "128MB", "Butuh restart"),
            ("work_mem", "4MB".into(), "4MB", "Berlaku setelah reload"),
        ],
        EngineKey::Valkey => vec![
            ("port", port, "6379", "Diuji bind sebelum disimpan"),
            ("maxmemory", "256mb".into(), "0", "0 = tanpa batas"),
            ("appendonly", "no".into(), "no", "Persistensi AOF"),
        ],
        _ => vec![
            ("port", port, "3306", "Diuji bind sebelum disimpan"),
            ("max_connections", "151".into(), "151", "Butuh restart"),
            (
                "innodb_buffer_pool_size",
                "256M".into(),
                "128M",
                "Butuh restart",
            ),
        ],
    };
    let mut table = v_flex()
        .rounded(px(8.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .child(
            h_flex()
                .px(px(18.))
                .py(px(12.))
                .child(
                    Label::new(strings::UI_CONFIG_HEADING)
                        .flex_1()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                )
                .child(
                    Label::new(strings::UI_CONFIG_COLUMNS)
                        .text_xs()
                        .text_color(theme.muted_foreground),
                ),
        );
    for (index, (name, value, default, hint)) in params.into_iter().enumerate() {
        let changed = value != default && name != "port";
        table = table.child(
            h_flex()
                .id(SharedString::from(format!("config-row-{index}")))
                .gap_4()
                .px(px(18.))
                .py(px(10.))
                .border_t_1()
                .border_color(theme.table_row_border)
                .child(
                    v_flex()
                        .flex_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Label::new(name)
                                        .id(SharedString::from(format!("config-name-{index}")))
                                        .font_family(mono.clone()),
                                )
                                .when(changed, |this| {
                                    this.child(
                                        Label::new(strings::UI_CONFIG_CHANGED)
                                            .id(SharedString::from(format!(
                                                "config-changed-{index}"
                                            )))
                                            .text_size(px(11.))
                                            .text_color(theme.primary),
                                    )
                                }),
                        )
                        .child(
                            Label::new(hint)
                                .id(SharedString::from(format!("config-hint-{index}")))
                                .text_xs()
                                .text_color(theme.muted_foreground),
                        ),
                )
                .child(
                    div()
                        .w(px(220.))
                        .h(px(32.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(if changed { theme.primary } else { theme.border })
                        .bg(theme.background)
                        .child(
                            Label::new(value)
                                .id(SharedString::from(format!("config-value-{index}")))
                                .font_family(mono.clone()),
                        ),
                )
                .child(
                    Label::new(default)
                        .id(SharedString::from(format!("config-default-{index}")))
                        .w(px(120.))
                        .font_family(mono.clone())
                        .text_color(theme.muted_foreground),
                ),
        );
    }
    table.child(
        h_flex()
            .gap_2()
            .px(px(18.))
            .py(px(12.))
            .border_t_1()
            .border_color(theme.border)
            .child(
                Button::new("demo-save")
                    .primary()
                    .label(strings::UI_CONFIG_SAVE)
                    .disabled(true),
            )
            .child(section_label(strings::UI_PLACEHOLDER_PREVIEW).text_size(px(11.))),
    )
}
