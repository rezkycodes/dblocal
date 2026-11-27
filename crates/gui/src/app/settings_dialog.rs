//! The Settings dialog (`F-PKG-05`, `F-AUT-01`): a title-bar gear and
//! `Ctrl/Cmd+,` open it.
//!
//! Two sections, both reading only confirmed state:
//!
//! - **Updates**: the running version, how the app was installed, the automatic
//!   check switch, a manual "Check for updates", and the result — the newer
//!   version with a one-press update, or the package-manager reason why this
//!   install cannot replace itself (`F-PKG-05` AC2). It renders the same
//!   [`UpdateState`](super::update_state::UpdateState) the title-bar icon and
//!   the update dialog use.
//! - **Run at login**: the W4-D login-item state read on open, a switch,
//!   the location of the item, and a typed error when a write fails — the
//!   switch never shows enabled after a failed enable.

use dblocal_core::strings;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::label::Label;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Disableable, FocusableExt as _, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, Window, div, px,
};

use super::Workspace;
use super::create_dialog::overlay;
use super::settings_state::{LoginState, install_kind_label};
use super::update_state::UpdateState;
use crate::app::components::icon;
use crate::app::design;

/// Renders the Settings dialog.
pub(super) fn render(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let update = workspace.update_state();

    let mut version_value = h_flex().gap_2().items_center().child(
        Label::new(workspace.version())
            .id("settings-version-value")
            .font_family(theme.mono_font_family.clone()),
    );
    if let Some(kind) = update.install_kind.clone() {
        version_value = version_value.child(design::chip(
            "install-kind",
            install_kind_label(&kind),
            theme.info,
        ));
    }

    let updates = v_flex()
        .id("settings-updates")
        .gap_2()
        .child(design::group_heading(strings::UI_SETTINGS_UPDATES, &theme))
        .child(
            design::group_card("settings-updates", &theme)
                .child(design::group_row(
                    "settings-version",
                    strings::UI_SETTINGS_CURRENT_VERSION,
                    version_value,
                    true,
                    &theme,
                ))
                .child(render_auto_check(
                    update,
                    &theme,
                    window.last_input_was_keyboard(),
                    cx,
                ))
                .child(render_update_result(update, workspace, &theme, cx)),
        );

    let login = v_flex()
        .id("settings-login")
        .gap_2()
        .child(design::group_heading(
            strings::UI_SETTINGS_AUTOSTART,
            &theme,
        ))
        .child(
            design::group_card("settings-login", &theme).child(render_login_section(
                workspace,
                &theme,
                window.last_input_was_keyboard(),
                cx,
            )),
        );

    overlay(
        "settings-overlay",
        "settings-panel",
        div()
            .w(px(620.))
            .max_h(px(760.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(px(12.))
            .shadow_lg()
            .overflow_hidden()
            .child(
                design::dialog_header(
                    "settings",
                    gpui_kit::assets::IconName::Settings,
                    strings::UI_SETTINGS_TITLE,
                    Some(strings::UI_SETTINGS_SUBTITLE),
                    &theme,
                )
                .border_b_1()
                .border_color(theme.border)
                .child(
                    Button::new("settings-close")
                        .ghost()
                        .small()
                        .compact()
                        .icon(gpui_kit::assets::IconName::Close)
                        .tooltip(strings::UI_SETTINGS_CLOSE)
                        .accessibility_label(strings::UI_SETTINGS_CLOSE)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_settings(cx);
                        })),
                ),
            )
            .child(
                v_flex()
                    .id("settings-body")
                    .gap_5()
                    .p(px(20.))
                    .child(updates)
                    .child(login),
            ),
        workspace,
        window,
        cx,
    )
}

/// The automatic-check switch as a setting row, with its typed error.
fn render_auto_check(
    update: &UpdateState,
    theme: &gpui_kit::component::Theme,
    keyboard: bool,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let row = design::setting_row(
        "settings-auto-check",
        strings::UI_SETTINGS_AUTO_CHECK,
        Some(strings::UI_SETTINGS_AUTO_CHECK_BODY),
        Switch::new("settings-auto-check-switch")
            // A click focuses the switch; its ring is for keyboard focus only.
            .focus_ring(keyboard)
            .accessibility_label(strings::UI_SETTINGS_AUTO_CHECK)
            .checked(update.auto_check.unwrap_or(true))
            .disabled(update.auto_check_busy)
            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                this.set_auto_update_check(*checked, cx);
            })),
        false,
        theme,
    );
    match &update.auto_check_error {
        Some(error) => v_flex()
            .child(row)
            .child(
                Label::new(error.clone())
                    .id(SharedString::from("settings-auto-check-error"))
                    .px(px(design::ROW_PAD_X))
                    .pb(px(10.))
                    .text_xs()
                    .text_color(theme.danger),
            )
            .into_any_element(),
        None => row.into_any_element(),
    }
}

/// The update check's answer and the actions it allows.
fn render_update_result(
    update: &UpdateState,
    workspace: &Workspace,
    theme: &gpui_kit::component::Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let mut column = v_flex()
        .id("settings-check")
        .gap_2()
        .px(px(design::ROW_PAD_X))
        .py(px(12.))
        .border_t_1()
        .border_color(crate::theme::tokens::hairline(theme));

    if update.icon_visible() {
        column = column.child(
            h_flex()
                .gap_2()
                .child(icon(
                    gpui_kit::assets::IconName::CircleArrowUp,
                    14.0,
                    theme.primary,
                ))
                .child(
                    Label::new(update.headline())
                        .id(SharedString::from("settings-update-headline"))
                        .text_sm(),
                ),
        );
    } else if update.checked_once {
        column = column.child(
            h_flex()
                .gap_2()
                .child(icon(
                    gpui_kit::assets::IconName::CircleCheck,
                    14.0,
                    theme.success,
                ))
                .child(
                    Label::new(strings::UI_SETTINGS_UP_TO_DATE)
                        .id(SharedString::from("settings-up-to-date"))
                        .text_sm(),
                ),
        );
    }

    if let Some(err) = &update.check_error {
        column = column.child(error_line(&err.code, &err.message, theme));
    }

    if let Some(reason) = update.cannot_update_reason() {
        column = column.child(
            Label::new(reason.to_owned())
                .id(SharedString::from("settings-package-hint"))
                .text_xs()
                .text_color(theme.warning),
        );
    }

    let checking = update.checking;
    let running = update.running();
    column = column.child(
        h_flex()
            .gap_2()
            .items_center()
            .child(
                Button::new("settings-check-now")
                    .outline()
                    .icon(gpui_kit::assets::IconName::RefreshCw)
                    .label(if checking {
                        strings::UI_SETTINGS_CHECKING
                    } else {
                        strings::UI_SETTINGS_CHECK_UPDATES
                    })
                    .small()
                    .loading(checking)
                    .disabled(checking || running)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.check_update_now(cx);
                    })),
            )
            // Only an install that may replace itself gets the update button:
            // a managed install or a release without a signing key is pointed
            // at its release page instead (`F-PKG-05` AC2).
            .when(update.can_self_update() && !running, |this| {
                this.child(
                    Button::new("settings-update")
                        .label(strings::UI_SETTINGS_INSTALL)
                        .small()
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.press_update_button(window, cx);
                        })),
                )
            })
            .when_some(update.release_url().map(str::to_owned), |this, _url| {
                this.child(
                    Button::new("settings-release-page")
                        .secondary()
                        .small()
                        .label(strings::UI_UPDATE_RELEASE_PAGE)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_update_release_page(cx);
                        })),
                )
            }),
    );

    let _ = workspace;
    column.into_any_element()
}

/// The login-item switch, its confirmed state and its typed error.
fn render_login_section(
    workspace: &Workspace,
    theme: &gpui_kit::component::Theme,
    keyboard: bool,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let state = workspace.settings_state();
    let busy = matches!(state.login, LoginState::Busy(_));
    let enabled = state.login_enabled();
    let mut column = v_flex().id("settings-login-body");

    column = column.child(design::setting_row(
        "settings-login",
        strings::UI_SETTINGS_AUTOSTART_SWITCH,
        Some(strings::UI_SETTINGS_AUTOSTART_BODY),
        Switch::new("settings-login-switch")
            // A click focuses the switch; its ring is for keyboard focus only.
            .focus_ring(keyboard)
            .accessibility_label(strings::UI_SETTINGS_AUTOSTART_SWITCH)
            .checked(enabled)
            .disabled(busy)
            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                this.set_login_item(*checked, cx);
            })),
        true,
        theme,
    ));

    // The confirmed state renders whenever one was read, whatever the section's
    // status: a failed write keeps the switch on the machine's real state and
    // shows the typed error under it (F2).
    let mut detail = v_flex()
        .gap_1p5()
        .px(px(design::ROW_PAD_X))
        .py(px(12.))
        .border_t_1()
        .border_color(crate::theme::tokens::hairline(theme));
    match state.login_known() {
        Some(status) => {
            detail = detail.child(
                h_flex()
                    .gap_2()
                    .child(crate::app::components::status_glyph(
                        if status.enabled {
                            dblocal_core::model::InstanceStatus::Running
                        } else {
                            dblocal_core::model::InstanceStatus::Stopped
                        },
                        7.0,
                        theme,
                    ))
                    .child(
                        Label::new(if status.enabled {
                            strings::UI_SETTINGS_AUTOSTART_ON
                        } else {
                            strings::UI_SETTINGS_AUTOSTART_OFF
                        })
                        .id(SharedString::from("settings-login-state"))
                        .text_xs()
                        .text_color(if status.enabled {
                            theme.success
                        } else {
                            theme.muted_foreground
                        }),
                    ),
            );
            detail = detail.child(
                h_flex()
                    .gap_2()
                    .child(
                        Label::new(strings::UI_SETTINGS_AUTOSTART_LOCATION)
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        Label::new(status.location.display().to_string())
                            .id(SharedString::from("settings-login-location"))
                            .text_xs()
                            .truncate()
                            .font_family(theme.mono_font_family.clone()),
                    ),
            );
        }
        None => {
            detail = detail.child(
                Label::new(strings::UI_SETTINGS_AUTOSTART_UNKNOWN)
                    .id(SharedString::from("settings-login-unknown"))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            );
        }
    }

    if let Some(err) = state.login_error() {
        detail = detail.child(error_line(&err.code, &err.message, theme));
    }
    if busy {
        detail = detail.child(
            Label::new(strings::UI_PENDING)
                .id(SharedString::from("settings-login-busy"))
                .text_xs()
                .text_color(theme.muted_foreground),
        );
    }

    column.child(detail).into_any_element()
}

/// One typed failure line: code plus the message.
fn error_line(code: &str, message: &str, theme: &gpui_kit::component::Theme) -> AnyElement {
    h_flex()
        .id(SharedString::from(format!("settings-error-{code}")))
        .gap_2()
        .items_start()
        .child(icon(
            gpui_kit::assets::IconName::TriangleAlert,
            14.0,
            theme.danger,
        ))
        .child(
            Label::new(format!("{code} — {message}"))
                .id(SharedString::from(format!("settings-error-text-{code}")))
                .text_xs()
                .text_color(theme.danger),
        )
        .into_any_element()
}
