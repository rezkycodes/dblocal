//! The Config tab (`F-CFG-01`, `F-CFG-02`), live over `DblService`.
//!
//! One card "Pengaturan" with a row per setting from `config_view`: the key, the
//! current value in a field (or plain text when read-only), the default next to
//! it, the unit and the one-line description, and a "perlu restart" marker in
//! text. Edits are local until "Terapkan"; the footer keeps "Buka file
//! konfigurasi", "Pulihkan konfigurasi sebelumnya", "Batalkan perubahan" and the
//! apply. A read failure is the service's typed message with a retry.
//!
//! Nothing per-engine is hard-coded here: every row, its default and its unit
//! come from the view. The one deliberate deviation — `port` and PostgreSQL's
//! authentication method are read-only — is rendered as plain text with the
//! reason `docs/architecture/config.md` records; no control is added for them.
//!
//! Identifiers are derived from the instance id and the key, so two instances
//! with the same keys cannot collide in the a11y tree (the W2-B2 trap).

use dblocal_core::config::ConfigSetting;
use dblocal_core::service::InstanceView;
use dblocal_core::strings;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, Theme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement, SharedString, div, px,
};

use super::Workspace;
use super::page;
use crate::app::components::{banner, card, icon};
use crate::app::config_state::{InstanceConfig, config_action_id, config_row_id};

impl Workspace {
    pub(crate) fn render_config(&self, view: &InstanceView, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(config) = self.config_state_for(&view.id) else {
            // The tab was never entered; render the empty read state so the
            // first frame before the read lands is honest, not blank.
            return page()
                .child(self.render_config_loading(view, &theme))
                .into_any_element();
        };
        let mut content = page();
        if let Some(error) = self.render_config_error(view, config, &theme, cx) {
            content = content.child(error);
        }
        if let Some(applied) = self.render_config_applied(view, config, &theme, cx) {
            content = content.child(applied);
        }
        content = content.child(self.render_config_card(view, config, &theme, cx));
        content.into_any_element()
    }

    fn render_config_loading(&self, view: &InstanceView, theme: &Theme) -> AnyElement {
        card(
            Some(strings::UI_CONFIG_CARD_TITLE),
            Some("config-shell"),
            theme,
        )
        .child(
            Label::new(strings::UI_CONFIG_LOADING)
                .id(SharedString::from(config_action_id(
                    "config-loading",
                    &view.id,
                )))
                .text_xs()
                .text_color(theme.muted_foreground),
        )
        .into_any_element()
    }

    /// The typed failure of the last read/apply/restore/open, with a retry.
    fn render_config_error(
        &self,
        view: &InstanceView,
        config: &InstanceConfig,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let err = config
            .error
            .as_ref()
            .or_else(|| config.data.error())?
            .clone();
        let retry_id = view.id.clone();
        let path = config
            .view()
            .map(|v| v.raw_path.display().to_string())
            .unwrap_or_default();
        let mut row = banner(theme.danger)
            .items_center()
            .child(icon(IconName::TriangleAlert, 16.0, theme.danger))
            .child(
                Label::new(format!("{} — {}", err.code, err.message))
                    .id(SharedString::from(config_action_id(
                        "config-error",
                        &view.id,
                    )))
                    .flex_1()
                    .text_xs()
                    .text_color(theme.danger),
            );
        // A launcher failure still shows the path so the user can open it by hand
        // (`DBL-8608`); it is never a panic.
        if !path.is_empty() && err.code == dblocal_core::config::error::CODE_LAUNCHER {
            row = row.child(
                Label::new(path)
                    .id(SharedString::from(config_action_id(
                        "config-error-path",
                        &view.id,
                    )))
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground),
            );
        }
        Some(
            row.child(
                Button::new(SharedString::from(config_action_id(
                    "config-retry",
                    &view.id,
                )))
                .label(strings::UI_CONFIG_RETRY)
                .small()
                .compact()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.retry_config(&retry_id, cx);
                })),
            )
            .into_any_element(),
        )
    }

    /// The "Tersimpan. Berlaku setelah restart." banner with the backup time and
    /// the restart offer (`F-CFG-01` AC2).
    fn render_config_applied(
        &self,
        view: &InstanceView,
        config: &InstanceConfig,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let applied = config.applied.as_ref()?;
        let now = now_unix_secs();
        let backup_time = strings::config_backup_age(applied.backup.taken_at_secs, now);
        let id_later = view.id.clone();
        let id_restart = view.id.clone();
        let running = crate::format::is_running(view.status);
        let busy = config.any_busy();

        let mut row = banner(theme.warning)
            .items_center()
            .child(icon(IconName::RefreshCw, 16.0, theme.warning))
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(240.))
                    .gap_1()
                    .flex_wrap()
                    .child(
                        Label::new(strings::UI_CONFIG_APPLIED)
                            .id(SharedString::from(config_action_id(
                                "config-applied",
                                &view.id,
                            )))
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        Label::new(format!(
                            "{} {backup_time}",
                            dblocal_core::config::messages::MSG_BACKUP_AT
                        ))
                        .id(SharedString::from(config_action_id(
                            "config-applied-backup",
                            &view.id,
                        )))
                        .text_xs()
                        .text_color(theme.muted_foreground),
                    ),
            )
            .child(
                Button::new(SharedString::from(config_action_id(
                    "config-later",
                    &view.id,
                )))
                .debug_selector(|| config_action_id("config-later", &view.id))
                .label(strings::UI_CONFIG_LATER)
                .small()
                .compact()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.dismiss_config_applied(&id_later, cx);
                })),
            );
        // "Restart sekarang" only when the instance runs (`F-CFG-01` AC2).
        if running {
            let restart_selector = config_action_id("config-restart", &view.id);
            row = row.child(
                Button::new(SharedString::from(config_action_id(
                    "config-restart",
                    &view.id,
                )))
                .debug_selector(|| restart_selector)
                .label(strings::UI_CONFIG_RESTART_NOW)
                .small()
                .compact()
                .primary()
                .loading(busy)
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.restart_after_apply(&id_restart, cx);
                })),
            );
        }
        Some(row.into_any_element())
    }

    /// The one card: header, rows, footer actions.
    fn render_config_card(
        &self,
        view: &InstanceView,
        config: &InstanceConfig,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card_selector = config_action_id("config-card", &view.id);
        let mut body = v_flex()
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .debug_selector(|| card_selector)
            .child(
                h_flex()
                    .px(px(18.))
                    .py(px(12.))
                    .gap_2()
                    .child(
                        Label::new(strings::UI_CONFIG_CARD_TITLE)
                            .id(SharedString::from(config_action_id(
                                "config-title",
                                &view.id,
                            )))
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        Label::new(strings::UI_CONFIG_COLUMNS)
                            .id(SharedString::from(config_action_id(
                                "config-columns",
                                &view.id,
                            )))
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    ),
            );

        let Some(config_view) = config.view() else {
            body = body.child(
                Label::new(strings::UI_CONFIG_LOADING)
                    .id(SharedString::from(config_action_id(
                        "config-body-loading",
                        &view.id,
                    )))
                    .p(px(18.))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            );
            return body.into_any_element();
        };

        if config_view.settings.is_empty() {
            body = body.child(
                Label::new(strings::UI_CONFIG_EMPTY)
                    .id(SharedString::from(config_action_id(
                        "config-empty",
                        &view.id,
                    )))
                    .p(px(18.))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            );
        }

        if config.touched_outside {
            body = body.child(
                h_flex()
                    .px(px(18.))
                    .py(px(6.))
                    .gap_1p5()
                    .items_center()
                    .child(icon(IconName::Info, 12.0, theme.primary))
                    .child(
                        Label::new(strings::UI_CONFIG_CHANGED_OUTSIDE)
                            .id(SharedString::from(config_action_id(
                                "config-outside",
                                &view.id,
                            )))
                            .text_xs()
                            .text_color(theme.primary),
                    ),
            );
        }

        for setting in config.editable().into_iter().chain(config.readonly()) {
            body = body.child(self.render_config_row(view, config, setting, theme, cx));
        }

        body.child(self.render_config_footer(view, config, theme, cx))
            .into_any_element()
    }

    /// One row: key, description, restart marker on the left; field, default,
    /// unit on the right. Read-only rows draw plain text with their reason.
    fn render_config_row(
        &self,
        view: &InstanceView,
        config: &InstanceConfig,
        setting: &ConfigSetting,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mono = theme.mono_font_family.clone();
        let changed = config.edits.contains_key(&setting.key);
        // `min_w(px(0.))` is what lets this column actually flex: without it the
        // text's intrinsic width wins and every row pushes its field to a
        // different x (`F3`).
        let left = v_flex()
            .flex_1()
            .min_w(px(0.))
            .gap_0p5()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Label::new(setting.key.clone())
                            .id(SharedString::from(config_row_id(
                                "config-key",
                                &view.id,
                                &setting.key,
                            )))
                            .font_family(mono.clone())
                            .when(changed, |this| this.text_color(theme.primary)),
                    )
                    .when(!setting.readonly && setting.restart_required, |this| {
                        this.child(
                            Label::new(strings::UI_CONFIG_RESTART_MARKER)
                                .id(SharedString::from(config_row_id(
                                    "config-restart-marker",
                                    &view.id,
                                    &setting.key,
                                )))
                                .text_size(px(11.))
                                .text_color(theme.warning),
                        )
                    }),
            )
            .child(
                Label::new(setting.description.clone())
                    .id(SharedString::from(config_row_id(
                        "config-desc",
                        &view.id,
                        &setting.key,
                    )))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            );

        // A fixed grid shared by every row (`F3`): the field is always the same
        // width and the same distance from the card's left edge, so the whole
        // card lines up instead of every row starting its field where its own
        // text happens to end. The default/unit cell has a fixed width too and
        // wraps (or truncates) rather than pushing past the card border.
        let mut right = h_flex().flex_none().gap_3().items_center();
        if setting.readonly {
            right = right
                .child(
                    div()
                        .w(px(FIELD_W))
                        .flex_none()
                        .debug_selector(|| field_selector(view, &setting.key))
                        .child(
                            Label::new(setting.current.clone().unwrap_or_default())
                                .id(SharedString::from(config_row_id(
                                    "config-readonly-value",
                                    &view.id,
                                    &setting.key,
                                )))
                                .font_family(mono.clone()),
                        ),
                )
                .child(
                    div()
                        .w(px(SIDE_W))
                        .flex_none()
                        .debug_selector(|| side_selector(view, &setting.key))
                        .child(
                            Label::new(readonly_reason(&setting.key))
                                .id(SharedString::from(config_row_id(
                                    "config-readonly-reason",
                                    &view.id,
                                    &setting.key,
                                )))
                                .text_xs()
                                .text_color(theme.muted_foreground),
                        ),
                );
        } else {
            let field = self.render_config_field(view, setting, theme, cx);
            let mut default_note = v_flex()
                .w(px(SIDE_W))
                .flex_none()
                .min_w(px(0.))
                .gap_0p5()
                .debug_selector(|| side_selector(view, &setting.key))
                .child(
                    Label::new(strings::config_default(&setting.default))
                        .id(SharedString::from(config_row_id(
                            "config-default",
                            &view.id,
                            &setting.key,
                        )))
                        .font_family(mono.clone())
                        .text_sm()
                        .text_color(theme.muted_foreground),
                );
            // The unit lives under the default, inside the same fixed cell.
            if let Some(unit) = strings::config_unit(&setting.unit) {
                default_note = default_note.child(
                    Label::new(unit)
                        .id(SharedString::from(config_row_id(
                            "config-unit",
                            &view.id,
                            &setting.key,
                        )))
                        .text_xs()
                        .text_color(theme.muted_foreground),
                );
            }
            // An unset key shows "bawaan" so the default is clearly in effect.
            if setting.current.is_none() {
                default_note = default_note.child(
                    Label::new(strings::UI_CONFIG_UNSET_NOTE)
                        .id(SharedString::from(config_row_id(
                            "config-unset",
                            &view.id,
                            &setting.key,
                        )))
                        .text_size(px(11.))
                        .text_color(theme.muted_foreground),
                );
            }
            right = right.child(field).child(default_note);
        }

        // The row's content must fill the row: a `v_flex` child with no flex
        // sizes to its content, which lands the right group wherever the left
        // text happens to end (`F3`).
        let mut content = v_flex()
            .flex_1()
            .min_w(px(0.))
            .gap_1()
            .child(h_flex().flex_1().min_w(px(0.)).gap_4().items_center().child(left).child(right));
        // The service's own validation sentence, under the field (`DBL-8601`).
        if let Some(err) = config.errors.get(&setting.key) {
            content = content.child(
                Label::new(format!("{} — {}", err.code, err.message))
                    .id(SharedString::from(config_row_id(
                        "config-field-error",
                        &view.id,
                        &setting.key,
                    )))
                    .text_xs()
                    .text_color(theme.danger),
            );
        }

        h_flex()
            .id(SharedString::from(config_row_id(
                "config-row",
                &view.id,
                &setting.key,
            )))
            .gap_4()
            .px(px(18.))
            .py(px(10.))
            .border_t_1()
            .border_color(theme.table_row_border)
            .when(changed, |this| this.bg(theme.muted))
            .child(content)
            .into_any_element()
    }

    /// The editable field of one row, or plain text when the tab is in flight.
    fn render_config_field(
        &self,
        view: &InstanceView,
        setting: &ConfigSetting,
        theme: &Theme,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        // The field entity is built by `Render::render` before the frame, so it
        // exists by the time a row draws; the fallback keeps the frame honest on
        // the very first paint before the read lands.
        let key = setting.key.clone();
        let selector = field_selector(view, &setting.key);
        match self.config_field_handle(&view.id, &setting.key) {
            Some(entity) => div()
                .w(px(FIELD_W))
                .flex_none()
                .debug_selector(|| selector)
                .child(Input::new(&entity).small())
                .into_any_element(),
            None => {
                let text = setting.current.clone().unwrap_or_default();
                div()
                    .w(px(FIELD_W))
                    .flex_none()
                    .debug_selector(|| selector)
                    .child(
                        Label::new(text)
                            .id(SharedString::from(config_row_id(
                                "config-value-pending",
                                &view.id,
                                &key,
                            )))
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground),
                    )
                    .into_any_element()
            }
        }
    }

    /// The card footer: open file, restore, reset, apply.
    fn render_config_footer(
        &self,
        view: &InstanceView,
        config: &InstanceConfig,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = config.any_busy();
        let can_apply = config.can_apply();
        let can_reset = config.can_reset();
        let can_restore = config.can_restore();
        let id_open = view.id.clone();
        let id_copy = view.id.clone();
        let id_restore = view.id.clone();
        let id_reset = view.id.clone();
        let id_apply = view.id.clone();
        let path = config
            .view()
            .map(|v| v.raw_path.display().to_string())
            .unwrap_or_default();

        let mut footer = h_flex()
            .gap_2()
            .px(px(18.))
            .py(px(12.))
            .border_t_1()
            .border_color(theme.border)
            .flex_wrap()
            .child(
                Button::new(SharedString::from(config_action_id(
                    "config-open",
                    &view.id,
                )))
                .icon(IconName::ExternalLink)
                .label(strings::UI_CONFIG_OPEN_FILE)
                .small()
                .loading(config.is_busy(crate::app::config_state::ConfigAction::OpenRaw))
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_config_raw(&id_open, cx);
                })),
            )
            .child(
                Button::new(SharedString::from(config_action_id(
                    "config-copy-path",
                    &view.id,
                )))
                .icon(IconName::Copy)
                .label(strings::UI_CONFIG_COPY_PATH)
                .small()
                .compact()
                .disabled(path.is_empty())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.copy_config_path(&id_copy, cx);
                })),
            )
            .child(
                Button::new(SharedString::from(config_action_id(
                    "config-restore-backup",
                    &view.id,
                )))
                .label(strings::UI_CONFIG_RESTORE)
                .small()
                .disabled(!can_restore)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_config_restore(&id_restore, cx);
                })),
            )
            .child(div().flex_1())
            .child(
                Button::new(SharedString::from(config_action_id(
                    "config-cancel-changes",
                    &view.id,
                )))
                .label(strings::UI_CONFIG_CANCEL_CHANGES)
                .small()
                .disabled(!can_reset)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.reset_config_edits(&id_reset, cx);
                })),
            )
            .child(
                Button::new(SharedString::from(config_action_id(
                    "config-apply",
                    &view.id,
                )))
                .label(strings::UI_CONFIG_APPLY)
                .small()
                .primary()
                .loading(config.is_busy(crate::app::config_state::ConfigAction::Apply))
                .disabled(!can_apply)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.apply_config(&id_apply, cx);
                })),
            );

        // The raw path is always shown with its Copy button: the launcher can
        // fail, and the user must still be able to find the file (`F-CFG-02`).
        if !path.is_empty() {
            footer = footer.child(
                Label::new(path)
                    .id(SharedString::from(config_action_id(
                        "config-path",
                        &view.id,
                    )))
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground),
            );
        }
        footer.into_any_element()
    }
}

/// The field column's width: fixed for every editable and read-only row, so all
/// fields share one x and one width (`F3`).
const FIELD_W: f32 = 220.0;

/// The default/unit column's width: fixed, and it wraps instead of growing, so
/// nothing (the Valkey `save` default, a `"satuan: ..."` label) runs past the
/// card border (`F3`).
const SIDE_W: f32 = 150.0;

/// A stable debug selector for one row's field cell, readable through
/// `VisualTestContext::debug_bounds` (the bounds evidence `F3` wants).
fn field_selector(view: &InstanceView, key: &str) -> String {
    config_row_id("config-field-cell", &view.id, key)
}

/// A stable debug selector for one row's default/unit (or read-only reason) cell.
fn side_selector(view: &InstanceView, key: &str) -> String {
    config_row_id("config-side-cell", &view.id, key)
}

/// Why a read-only row cannot be edited here (`docs/architecture/config.md` §4).
fn readonly_reason(key: &str) -> &'static str {
    if key == "port" {
        strings::UI_CONFIG_READONLY_PORT
    } else {
        strings::UI_CONFIG_READONLY_MANAGED
    }
}

/// Current Unix time, for the backup age. Saturating so a clock before 1970
/// reads 0 rather than panicking.
fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
