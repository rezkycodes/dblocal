//! Shared PATH panel and PHP settings card for the Toolchains pages (v0.3.0).

use dblocal_core::strings;
use dblocal_core::toolchain::php_ini::SettingKey;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::label::Label;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, FocusableExt as _, Sizable, Theme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement, SharedString, Window, div, px,
};

use crate::app::Workspace;
use crate::app::components::icon;
use crate::app::design;
use crate::app::toolchain_state::{PHP_SETTINGS_META, PathHealth};
use crate::theme::tokens;

/// Fixed width of the PHP settings text field column so all fields line up.
const PHP_FIELD_W: f32 = 180.0;

impl Workspace {
    /// Shared PATH status and consent card, rendered on every toolchain page.
    pub(super) fn render_path_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let path_state = &self.toolchain.path;
        let mono = theme.mono_font_family.clone();

        let status_chip = match path_state.health {
            PathHealth::AppliedAndFirst => design::chip(
                "tc-path-status",
                strings::UI_TOOLCHAIN_PATH_ON_FIRST,
                theme.success,
            ),
            PathHealth::NotApplied => design::chip(
                "tc-path-status",
                strings::UI_TOOLCHAIN_PATH_NOT_ON,
                theme.warning,
            ),
            PathHealth::NeedsAttention => design::chip(
                "tc-path-status",
                strings::UI_TOOLCHAIN_PATH_ATTENTION,
                theme.danger,
            ),
        };

        let shims_display = crate::format::abbreviate_home(&path_state.shims_dir, self.home_dir());

        let mut card =
            design::group_card("tc-path-card", theme).debug_selector(|| "tc-path-card".to_owned());

        // Top summary row with status chip and actions.
        card = card.child(
            h_flex()
                .flex_wrap()
                .px(px(design::ROW_PAD_X))
                .py(px(12.))
                .gap_3()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(200.))
                        .gap_1()
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_2()
                                .items_center()
                                .child(
                                    Label::new(strings::UI_TOOLCHAIN_SHIMS_DIR)
                                        .id("tc-path-shims-title")
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(status_chip),
                        )
                        .child(
                            Label::new(shims_display)
                                .id("tc-path-shims-dir")
                                .text_xs()
                                .font_family(mono.clone())
                                .text_color(theme.muted_foreground),
                        ),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap_2()
                        .when(
                            path_state.health != PathHealth::AppliedAndFirst || !path_state.applied,
                            |bar| {
                                bar.child(
                                    Button::new("tc-path-add")
                                        .when(!path_state.applied, |b| b.primary())
                                        .when(path_state.applied, |b| b.outline())
                                        .small()
                                        .icon(IconName::Terminal)
                                        .label(strings::UI_TOOLCHAIN_PATH_ADD)
                                        .loading(path_state.busy)
                                        .disabled(path_state.busy)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_path_consent_dialog(window, cx);
                                        })),
                                )
                            },
                        )
                        .when(path_state.applied, |bar| {
                            bar.child(
                                Button::new("tc-path-remove")
                                    .outline()
                                    .small()
                                    .label(strings::UI_TOOLCHAIN_PATH_REMOVE)
                                    .disabled(path_state.busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.remove_toolchain_path(cx);
                                    })),
                            )
                        }),
                ),
        );

        // Plain-English findings list from `doctor`.
        if !path_state.findings.is_empty() {
            let mut list = v_flex()
                .px(px(design::ROW_PAD_X))
                .py(px(10.))
                .gap_1p5()
                .border_t_1()
                .border_color(tokens::hairline(theme));
            for (i, finding) in path_state.findings.iter().enumerate() {
                let colour = if finding.severe {
                    theme.danger
                } else {
                    theme.warning
                };
                list = list.child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .child(icon(IconName::TriangleAlert, 14.0, colour))
                        .child(
                            Label::new(finding.message.clone())
                                .id(SharedString::from(format!("tc-path-finding-{i}")))
                                .flex_1()
                                .min_w(px(180.))
                                .text_xs()
                                .text_color(colour),
                        ),
                );
            }
            card = card.child(list);
        }

        // Last edit report (per-file failures or summary).
        if let Some(report) = &path_state.last_report
            && (!report.failures.is_empty() || !report.changed.is_empty())
        {
            let mut rep = v_flex()
                .px(px(design::ROW_PAD_X))
                .py(px(10.))
                .gap_1()
                .border_t_1()
                .border_color(tokens::hairline(theme));
            for (i, path) in report.changed.iter().enumerate() {
                rep = rep.child(
                    Label::new(format!("Updated {}", path.display()))
                        .id(SharedString::from(format!("tc-path-changed-{i}")))
                        .text_xs()
                        .font_family(mono.clone())
                        .text_color(theme.success),
                );
            }
            for (i, (path, reason)) in report.failures.iter().enumerate() {
                rep = rep.child(
                    Label::new(format!("Failed {}: {reason}", path.display()))
                        .id(SharedString::from(format!("tc-path-fail-{i}")))
                        .text_xs()
                        .font_family(mono.clone())
                        .text_color(theme.danger),
                );
            }
            card = card.child(rep);
        }

        if let Some(err) = &path_state.last_error {
            card = card.child(
                div()
                    .px(px(design::ROW_PAD_X))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(tokens::hairline(theme))
                    .child(
                        Label::new(err.clone())
                            .id("tc-path-last-error")
                            .text_xs()
                            .text_color(theme.danger),
                    ),
            );
        }

        v_flex()
            .gap_1p5()
            .child(design::group_heading(
                strings::UI_TOOLCHAIN_SHELL_PATH,
                theme,
            ))
            .child(card)
            .into_any_element()
    }

    /// Global PHP settings card (only on the PHP page, enabled only when a PHP
    /// version is active).
    pub(super) fn render_php_settings_card(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let php_active = self.toolchain.php_settings_enabled();
        let form = &self.toolchain.php_settings;
        let mono = theme.mono_font_family.clone();
        let keyboard = window.last_input_was_keyboard();

        let mut card = design::group_card("tc-php-settings-card", theme)
            .debug_selector(|| "tc-php-settings-card".to_owned());

        if !php_active {
            card = card.child(
                h_flex()
                    .px(px(design::ROW_PAD_X))
                    .py(px(10.))
                    .gap_2()
                    .items_center()
                    .child(icon(IconName::Info, 14.0, theme.muted_foreground))
                    .child(
                        Label::new(strings::UI_TOOLCHAIN_PHP_SETTINGS_INACTIVE)
                            .id("tc-php-settings-disabled-note")
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    ),
            );
        }

        if let Some(notice) = &form.notice {
            card = card.child(
                h_flex()
                    .px(px(design::ROW_PAD_X))
                    .py(px(8.))
                    .gap_2()
                    .items_center()
                    .border_t_1()
                    .border_color(tokens::hairline(theme))
                    .child(icon(IconName::CircleCheck, 14.0, theme.success))
                    .child(
                        Label::new(notice.clone())
                            .id("tc-php-settings-notice")
                            .text_xs()
                            .text_color(theme.success),
                    ),
            );
        }

        if let Some(err) = &form.error {
            card = card.child(
                h_flex()
                    .px(px(design::ROW_PAD_X))
                    .py(px(8.))
                    .gap_2()
                    .items_center()
                    .border_t_1()
                    .border_color(tokens::hairline(theme))
                    .child(icon(IconName::TriangleAlert, 14.0, theme.danger))
                    .child(
                        Label::new(err.clone())
                            .id("tc-php-settings-error")
                            .text_xs()
                            .text_color(theme.danger),
                    ),
            );
        }

        for (idx, meta) in PHP_SETTINGS_META.iter().enumerate() {
            let first = idx == 0 && php_active && form.notice.is_none() && form.error.is_none();
            let field_error = form.errors.get(&meta.key);

            let control = if meta.key == SettingKey::DisplayErrors {
                div()
                    .flex_none()
                    .child(
                        Switch::new("tc-php-display-errors")
                            .focus_ring(keyboard)
                            .accessibility_label("display_errors")
                            .checked(form.display_errors_on())
                            .disabled(!php_active || form.busy)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.toggle_php_display_errors(*checked, cx);
                            })),
                    )
                    .into_any_element()
            } else if let Some(entity) = self.php_setting_fields.get(&meta.key) {
                div()
                    .w(px(PHP_FIELD_W))
                    .flex_none()
                    .child(
                        Input::new(entity)
                            .small()
                            .disabled(!php_active || form.busy),
                    )
                    .into_any_element()
            } else {
                div()
                    .w(px(PHP_FIELD_W))
                    .flex_none()
                    .child(
                        Label::new(form.field_text(meta.key))
                            .id(SharedString::from(format!("tc-php-val-{idx}")))
                            .font_family(mono.clone())
                            .text_color(theme.muted_foreground),
                    )
                    .into_any_element()
            };

            let mut row = v_flex()
                .id(SharedString::from(format!("tc-php-row-{idx}")))
                .px(px(design::ROW_PAD_X))
                .py(px(9.))
                .gap_1()
                .when(!first, |d| {
                    d.border_t_1().border_color(tokens::hairline(theme))
                })
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_3()
                        .items_center()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w(px(180.))
                                .gap_0p5()
                                .child(
                                    Label::new(meta.label)
                                        .id(SharedString::from(format!("tc-php-key-{idx}")))
                                        .font_family(mono.clone())
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM),
                                )
                                .child(
                                    Label::new(meta.description)
                                        .id(SharedString::from(format!("tc-php-desc-{idx}")))
                                        .text_xs()
                                        .text_color(theme.muted_foreground),
                                ),
                        )
                        .child(control),
                );

            if let Some(err) = field_error {
                row = row.child(
                    Label::new(err.clone())
                        .id(SharedString::from(format!("tc-php-err-{idx}")))
                        .text_xs()
                        .text_color(theme.danger),
                );
            }

            card = card.child(row);
        }

        // Footer: action row (Open config folder left; Reset + Save together on
        // the right) with the full config folder path on its own line below so
        // Save never wraps alone and the path stays readable at 900px.
        let dir_label = crate::format::abbreviate_home(&form.config_dir, self.home_dir());
        card = card.child(
            v_flex()
                .px(px(design::ROW_PAD_X))
                .py(px(12.))
                .gap_1p5()
                .border_t_1()
                .border_color(tokens::hairline(theme))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("tc-php-open-folder")
                                .debug_selector(|| "tc-php-open-folder".to_owned())
                                .outline()
                                .small()
                                .icon(IconName::FolderOpen)
                                .label(strings::UI_TOOLCHAIN_PHP_OPEN_FOLDER)
                                .disabled(form.config_dir.as_os_str().is_empty())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_php_config_folder(cx);
                                })),
                        )
                        .child(div().flex_1())
                        .child(
                            h_flex()
                                .flex_none()
                                .gap_2()
                                .items_center()
                                .child(
                                    Button::new("tc-php-reset")
                                        .debug_selector(|| "tc-php-reset".to_owned())
                                        .outline()
                                        .small()
                                        .label(strings::UI_TOOLCHAIN_PHP_RESET)
                                        .disabled(!form.can_reset(php_active))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.reset_php_settings(cx);
                                        })),
                                )
                                .child(
                                    Button::new("tc-php-save")
                                        .debug_selector(|| "tc-php-save".to_owned())
                                        .primary()
                                        .small()
                                        .label(strings::UI_TOOLCHAIN_PHP_SAVE)
                                        .loading(form.busy)
                                        .disabled(!form.can_save(php_active))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.save_php_settings(cx);
                                        })),
                                ),
                        ),
                )
                .when(!dir_label.is_empty(), |col| {
                    col.child(
                        div()
                            .min_w_0()
                            .debug_selector(|| "tc-php-conf-dir".to_owned())
                            .child(
                                Label::new(dir_label)
                                    .id("tc-php-conf-dir")
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .font_family(mono)
                                    .text_color(theme.muted_foreground),
                            ),
                    )
                }),
        );

        v_flex()
            .gap_1p5()
            .child(design::group_heading(
                strings::UI_TOOLCHAIN_PHP_SETTINGS,
                theme,
            ))
            .child(card)
            .into_any_element()
    }
}
