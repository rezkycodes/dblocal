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
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, Window, div, px,
};

use super::Workspace;
use super::create_dialog::overlay;
use super::settings_state::{LoginState, install_kind_label};
use super::update_state::UpdateState;
use crate::app::components::{icon, section_label};

/// Renders the Settings dialog.
pub(super) fn render(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let update = workspace.update_state();

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
        .when_some(update.install_kind.clone(), |this, kind| {
            this.child(
                h_flex()
                    .gap_2()
                    .child(
                        Label::new(strings::UI_SETTINGS_INSTALL_KIND)
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    )
                    .child(Label::new(install_kind_label(&kind)).text_xs()),
            )
        })
        .child(render_update_result(update, workspace, &theme, cx));

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
                                this.close_settings(cx);
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
        workspace,
        window,
        cx,
    )
}

/// The update check's answer, its automatic switch, and the actions it allows.
fn render_update_result(
    update: &UpdateState,
    workspace: &Workspace,
    theme: &gpui_kit::component::Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let mut column = v_flex().id("settings-check").gap_2();

    // The automatic-check switch, with its own explanation.
    column = column.child(
        Switch::new("settings-auto-check-switch")
            .label(strings::UI_SETTINGS_AUTO_CHECK)
            .checked(update.auto_check.unwrap_or(true))
            .disabled(update.auto_check_busy)
            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                this.set_auto_update_check(*checked, cx);
            })),
    );
    column = column.child(
        Label::new(strings::UI_SETTINGS_AUTO_CHECK_BODY)
            .id(SharedString::from("settings-auto-check-body"))
            .text_xs()
            .text_color(theme.muted_foreground),
    );
    if let Some(error) = &update.auto_check_error {
        column = column.child(
            Label::new(error.clone())
                .id(SharedString::from("settings-auto-check-error"))
                .text_xs()
                .text_color(theme.danger),
        );
    }

    if update.icon_visible() {
        column = column.child(
            Label::new(update.headline())
                .id(SharedString::from("settings-update-headline"))
                .text_sm(),
        );
    } else if update.checked_once {
        column = column.child(
            Label::new(strings::UI_SETTINGS_UP_TO_DATE)
                .id(SharedString::from("settings-up-to-date"))
                .text_sm(),
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
            .when(update.icon_visible() && !running, |this| {
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
                        .ghost()
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
