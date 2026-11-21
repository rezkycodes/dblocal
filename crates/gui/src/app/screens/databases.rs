//! The Databases tab (`F-DBU`, Phase 2) and the Config tab (`F-CFG`, Phase 3).
//!
//! Neither has a backend in the frozen `DblService`: there is no
//! `list_databases`/`create_database`/`config.read` call. In the product build
//! both tabs render the prototype structure with every control disabled and the
//! "belum tersedia di versi ini (Fase N)" state, and never show sample rows.
//! The demo build may render the prototype layout with sample data so the design
//! stays reviewable (`D-25`).
//!
//! Every placeholder here is listed in `docs/architecture/gui.md` with the phase
//! that fills it.

use dblocal_core::model::EngineKey;
use dblocal_core::strings;
use gpui_kit::assets::IconName;
use gpui_kit::component::Disableable;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, Theme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, Styled, div, px};

use super::Workspace;
use super::page;
use crate::app::components::{card, icon, section_label};
use crate::format::{self, PlaceholderCard, PlaceholderTab};

impl Workspace {
    pub(crate) fn render_databases(
        &self,
        view: &dblocal_core::service::InstanceView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        if !self.demo_content() {
            return placeholder_page(
                PlaceholderTab::Databases,
                IconName::Database,
                strings::UI_SUBTAB_DATABASES,
                &theme,
            );
        }
        // Demo build: the prototype layout with sample rows so it is reviewable.
        if view.engine == EngineKey::Valkey {
            return page()
                .child(
                    card(None, &theme).child(
                        Label::new(strings::UI_DATABASES_VALKEY_NOTE)
                            .text_color(theme.muted_foreground),
                    ),
                )
                .into_any_element();
        }
        page()
            .child(demo_databases_table(&theme))
            .child(demo_users_card(&theme))
            .into_any_element()
    }

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
    // The prototype's Databases create card puts title + field + button on one
    // header row. Detect that shape so we don't stack them and crush padding.
    let header_row = spec.title.is_some()
        && spec.controls.len() == 2
        && spec.controls[0].kind == format::PlaceholderControlKind::Field
        && spec.controls[1].kind == format::PlaceholderControlKind::Button;

    let mut card_box = if header_row {
        let mut hdr = h_flex().gap_2().items_center();
        if let Some(title) = spec.title {
            hdr = hdr.child(
                div().flex_1().child(
                    Label::new(title.to_owned())
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                ),
            );
        }
        for (control_index, control) in spec.controls.iter().enumerate() {
            hdr = hdr.child(placeholder_control(
                tab,
                index,
                control_index,
                control,
                theme,
            ));
        }
        let mut c = card(None, theme);
        c = c.child(hdr);
        c
    } else {
        let mut c = card(spec.title, theme);
        if !spec.controls.is_empty() {
            let mut row = h_flex().gap_2().items_center().flex_wrap();
            for (control_index, control) in spec.controls.iter().enumerate() {
                row = row.child(placeholder_control(
                    tab,
                    index,
                    control_index,
                    control,
                    theme,
                ));
            }
            c = c.child(row);
        }
        c
    };

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

/// One disabled control: a button, or a text field drawn with its placeholder.
fn placeholder_control(
    tab: PlaceholderTab,
    card_index: usize,
    control_index: usize,
    control: &format::PlaceholderControl,
    theme: &Theme,
) -> AnyElement {
    let id = format!(
        "placeholder-{tab:?}-{card_index}-{control_index}",
        tab = tab
    );
    match control.kind {
        format::PlaceholderControlKind::Button => Button::new(id)
            .label(control.label)
            .disabled(control.disabled)
            .into_any_element(),
        format::PlaceholderControlKind::Field => div()
            .h(px(32.))
            .w(px(200.))
            .px(px(10.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.table_row_border)
            .bg(theme.background)
            .child(
                Label::new(control.label)
                    .text_sm()
                    .text_color(theme.muted_foreground),
            )
            .into_any_element(),
    }
}

/// The prototype's Databases layout, drawn only in the demo build.
fn demo_databases_table(theme: &Theme) -> impl IntoElement {
    let mono = theme.mono_font_family.clone();
    let rows = [
        ("postgres", "7,5 MB"),
        ("app_dev", "31,4 MB"),
        ("app_test", "9,3 MB"),
    ];
    let mut table = v_flex()
        .rounded(px(8.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .child(
            h_flex()
                .p(px(14.))
                .gap_2()
                .items_center()
                .child(
                    Label::new(format!("{} · 3", strings::UI_DATABASES_HEADING))
                        .flex_1()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                )
                .child(
                    Button::new("demo-create-db")
                        .primary()
                        .small()
                        .label(strings::UI_DATABASES_CREATE)
                        .disabled(true),
                ),
        )
        .child(
            h_flex()
                .h(px(30.))
                .px(px(14.))
                .text_xs()
                .text_color(theme.muted_foreground)
                .border_t_1()
                .border_color(theme.border)
                .child(Label::new(strings::UI_DATABASES_COLUMN_NAME).flex_1())
                .child(Label::new(strings::UI_DATABASES_COLUMN_ENCODING).w(px(160.)))
                .child(Label::new(strings::UI_DATABASES_COLUMN_SIZE).w(px(100.))),
        );
    for (name, size) in rows {
        table = table.child(
            h_flex()
                .h(px(42.))
                .px(px(14.))
                .border_t_1()
                .border_color(theme.table_row_border)
                .child(Label::new(name).flex_1().font_family(mono.clone()))
                .child(
                    Label::new(strings::UI_DATABASES_ENCODING)
                        .w(px(160.))
                        .text_color(theme.muted_foreground),
                )
                .child(Label::new(size).w(px(100.))),
        );
    }
    table
}

/// The demo user list and the superuser password card.
fn demo_users_card(theme: &Theme) -> impl IntoElement {
    let mono = theme.mono_font_family.clone();
    card(Some(strings::UI_DATABASES_USERS), theme).children(
        [
            ("postgres", "semua", "superuser"),
            ("app_rw", "app_dev", "read-write"),
            ("report_ro", "app_dev", "read-only"),
        ]
        .into_iter()
        .map(|(user, db, role)| {
            h_flex()
                .gap_3()
                .h(px(34.))
                .child(Label::new(user).flex_1().font_family(mono.clone()))
                .child(
                    Label::new(db)
                        .w(px(100.))
                        .text_color(theme.muted_foreground),
                )
                .child(
                    Label::new(role)
                        .px_2()
                        .rounded_full()
                        .bg(theme.muted)
                        .text_xs(),
                )
        }),
    )
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
    for (name, value, default, hint) in params {
        let changed = value != default && name != "port";
        table = table.child(
            h_flex()
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
                                .child(Label::new(name).font_family(mono.clone()))
                                .when(changed, |this| {
                                    this.child(
                                        Label::new(strings::UI_CONFIG_CHANGED)
                                            .text_size(px(11.))
                                            .text_color(theme.primary),
                                    )
                                }),
                        )
                        .child(
                            Label::new(hint)
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
                        .child(Label::new(value).font_family(mono.clone())),
                )
                .child(
                    Label::new(default)
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
