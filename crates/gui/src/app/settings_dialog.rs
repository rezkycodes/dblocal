//! The Settings dialog (`F-PKG-05`, `F-AUT-01`): a title-bar gear and
//! `Ctrl/Cmd+,` open it.
//!
//! Two sections, both reading only confirmed state:
//!
//! - **Pembaruan**: the running version, how the app was installed, a manual
//!   "Periksa sekarang", the result, and "Pasang" only when this install may
//!   self-update — otherwise the package-manager hint the service returned
//!   (`F-PKG-05` AC2). No timer, no check at startup.
//! - **Jalankan saat login**: the W4-D login-item state read on open, a switch,
//!   the location of the item, and a typed error when a write fails — the
//!   switch never shows enabled after a failed enable.

use dblocal_core::strings;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::label::Label;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, div, px};

use super::Workspace;
use super::create_dialog::overlay;
use super::settings_state::{
    InstallState, LoginState, can_install, check_headline, install_kind_label, package_manager_hint,
};
use crate::app::components::{icon, section_label};

/// Renders the Settings dialog.
pub(super) fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let theme = cx.theme().clone();

    let updates = v_flex()
        .id("settings-updates")
        .gap_2()
        .child(section_label(strings::UI_SETTINGS_UPDATES).text_color(theme.muted_foreground))
        .child(
            h_flex()
                .gap_2()
                .child(
                    Label::new(strings::UI_SETTINGS_CURRENT_VERSION)
                        .text_xs()
                        .text_color(theme.muted_foreground),
                )
                .child(Label::new(workspace.version()).text_xs()),
        )
        .child(render_check_result(workspace, &theme, cx));

    let login = v_flex()
        .id("settings-login")
        .gap_2()
        .child(section_label(strings::UI_SETTINGS_AUTOSTART).text_color(theme.muted_foreground))
        .child(
            Label::new(strings::UI_SETTINGS_AUTOSTART_BODY)
                .text_xs()
                .text_color(theme.muted_foreground),
        )
        .child(render_login_section(workspace, &theme, cx));

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
            .rounded(px(10.))
            .overflow_hidden()
            .child(
                h_flex()
                    .px(px(20.))
                    .py(px(14.))
                    .border_b_1()
                    .border_color(theme.border)
                    .justify_between()
                    .child(
                        Label::new(strings::UI_SETTINGS_TITLE)
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                    )
                    .child(
                        Button::new("settings-close")
                            .ghost()
                            .small()
                            .compact()
                            .label(strings::UI_SETTINGS_CLOSE)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_settings();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("settings-body")
                    .gap_6()
                    .p(px(20.))
                    .child(updates)
                    .child(login),
            ),
        cx,
    )
}

/// The update check's current answer and the actions it allows.
fn render_check_result(
    workspace: &Workspace,
    theme: &gpui_kit::component::Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let state = workspace.settings_state();
    let mut column = v_flex().id("settings-check").gap_2();

    if let Some(status) = state.checked() {
        column = column.child(
            h_flex()
                .gap_2()
                .child(
                    Label::new(strings::UI_SETTINGS_INSTALL_KIND)
                        .text_xs()
                        .text_color(theme.muted_foreground),
                )
                .child(Label::new(install_kind_label(&status.install_kind)).text_xs()),
        );
        if let Some(headline) = check_headline(state) {
            column = column.child(Label::new(headline).text_sm());
        }
    }

    if let Some(err) = state.check_error() {
        column = column.child(error_line(&err.code, &err.message, theme));
    }

    let checking = state.is_checking();
    let installable = can_install(state);
    let installing = state.is_installing();
    column = column.child(
        h_flex()
            .gap_2()
            .items_center()
            .child(
                Button::new("settings-check-now")
                    .label(if checking {
                        strings::UI_SETTINGS_CHECKING
                    } else {
                        strings::UI_SETTINGS_CHECK_NOW
                    })
                    .small()
                    .loading(checking)
                    .disabled(checking || installing)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.check_updates(cx);
                    })),
            )
            .when(installable || installing, |this| {
                this.child(
                    Button::new("settings-install")
                        .label(if installing {
                            strings::UI_SETTINGS_INSTALLING
                        } else {
                            strings::UI_SETTINGS_INSTALL
                        })
                        .small()
                        .primary()
                        .loading(installing)
                        .disabled(installing)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.install_update(cx);
                        })),
                )
            }),
    );

    if let Some(hint) = package_manager_hint(state) {
        // F-PKG-05 AC2: a package-manager install is pointed at its manager
        // instead of being offered a self-update.
        column = column.child(
            Label::new(hint.to_owned())
                .id(SharedString::from("settings-package-hint"))
                .text_xs()
                .text_color(theme.warning),
        );
    }

    if let Some(err) = state.install_error() {
        column = column.child(error_line(&err.code, &err.message, theme));
    } else if state.install == InstallState::Installed {
        column = column.child(
            Label::new(strings::UI_SETTINGS_INSTALLED)
                .id(SharedString::from("settings-installed"))
                .text_xs()
                .text_color(theme.success),
        );
    }

    column.into_any_element()
}

/// The login-item switch, its confirmed state and its typed error.
fn render_login_section(
    workspace: &Workspace,
    theme: &gpui_kit::component::Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let state = workspace.settings_state();
    let busy = matches!(state.login, LoginState::Busy(_));
    let enabled = state.login_enabled();
    let mut column = v_flex().id("settings-login-body").gap_2();

    column = column.child(
        Switch::new("settings-login-switch")
            .label(strings::UI_SETTINGS_AUTOSTART_SWITCH)
            .checked(enabled)
            .disabled(busy)
            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                this.set_login_item(*checked, cx);
            })),
    );

    // The confirmed state renders whenever one was read, whatever the section's
    // status: a failed write keeps the switch on the machine's real state and
    // shows the typed error under it (F2).
    match state.login_known() {
        Some(status) => {
            column = column.child(
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
            );
            column = column.child(
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
                            .font_family(theme.mono_font_family.clone()),
                    ),
            );
        }
        None => {
            column = column.child(
                Label::new(strings::UI_SETTINGS_AUTOSTART_UNKNOWN)
                    .id(SharedString::from("settings-login-unknown"))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            );
        }
    }

    if let Some(err) = state.login_error() {
        column = column.child(error_line(&err.code, &err.message, theme));
    }
    if busy {
        column = column.child(
            Label::new(strings::UI_PENDING)
                .id(SharedString::from("settings-login-busy"))
                .text_xs()
                .text_color(theme.muted_foreground),
        );
    }

    column.into_any_element()
}

/// One typed failure line: code plus the Indonesian message.
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
