//! The in-app update UI: the title-bar download button and its dialog
//! (`F-PKG-05`).
//!
//! The button sits beside the Settings gear and is drawn only while a newer
//! release is known. Pressing it opens the dialog, which is the one place an
//! update is started, cancelled, retried, restarted, or explained. The same
//! read model drives both, so neither can claim a state the other is not in.

use dblocal_core::strings;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::label::Label;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{ActiveTheme, Sizable, Theme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, Window, div, px,
};

use super::Workspace;
use super::create_dialog::overlay;
use super::update_state::UpdatePhase;
use crate::app::components::icon;

/// The title-bar update button, when a newer release is known.
pub(super) fn title_bar_button(
    workspace: &Workspace,
    cx: &mut Context<Workspace>,
) -> Option<AnyElement> {
    let state = workspace.update_state();
    if !state.icon_visible() {
        return None;
    }
    let label = state.headline();
    Some(
        Button::new("title-update")
            .ghost()
            .small()
            .compact()
            .icon(IconName::Download)
            .tooltip(SharedString::from(label.clone()))
            .accessibility_label(SharedString::from(label))
            .on_click(cx.listener(|this, _, window, cx| {
                this.press_update_button(window, cx);
            }))
            .into_any_element(),
    )
}

/// The update dialog.
pub(super) fn render(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let state = workspace.update_state();

    let mut body = v_flex().id("update-body").gap_3();
    body = body.child(
        Label::new(state.headline())
            .id(SharedString::from("update-headline"))
            .text_sm(),
    );

    if matches!(state.phase, UpdatePhase::CannotSelfUpdate) {
        if let Some(reason) = state.cannot_update_reason() {
            body = body.child(
                Label::new(reason.to_owned())
                    .id(SharedString::from("update-cannot-reason"))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            );
        }
        body = body.child(
            Label::new(strings::UI_UPDATE_CANNOT_BODY)
                .id(SharedString::from("update-cannot-body"))
                .text_xs()
                .text_color(theme.muted_foreground),
        );
    }

    if let Some(detail) = state.download_detail() {
        body = body.child(
            Label::new(detail)
                .id(SharedString::from("update-download-detail"))
                .text_xs()
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground),
        );
        body = body.child(
            Progress::new(SharedString::from("update-progress"))
                .value(state.percent().map(f32::from).unwrap_or(0.0))
                .color(theme.progress_bar)
                .h(px(8.))
                .accessibility_label(strings::UI_UPDATE_DOWNLOADING),
        );
    }

    if matches!(
        state.phase,
        UpdatePhase::Verifying | UpdatePhase::Installing
    ) {
        body = body.child(
            Progress::new(SharedString::from("update-progress-busy"))
                .value(100.0)
                .color(theme.progress_bar)
                .h(px(8.))
                .accessibility_label(strings::UI_UPDATE_INSTALLING),
        );
    }

    if let UpdatePhase::Failed { code, message } = &state.phase {
        body = body.child(error_line(code, message, &theme));
    }
    if let Some(err) = &state.restart_error {
        body = body.child(error_line(&err.code, &err.message, &theme));
    }

    body = body.child(actions(workspace, &theme, cx));

    let panel = div()
        .w(px(520.))
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
                .items_center()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(icon(IconName::Download, 16.0, theme.primary))
                        .child(
                            Label::new(strings::UI_UPDATE_DIALOG_TITLE)
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                        ),
                )
                .child(
                    Button::new("update-close")
                        .ghost()
                        .small()
                        .compact()
                        .label(strings::UI_UPDATE_CLOSE)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_update_dialog(cx);
                        })),
                ),
        )
        .child(v_flex().gap_4().p(px(20.)).child(body));

    overlay(
        "update-overlay",
        "update-panel",
        panel,
        workspace,
        window,
        cx,
    )
}

/// The dialog's button row, one set per phase.
fn actions(workspace: &Workspace, theme: &Theme, cx: &mut Context<Workspace>) -> AnyElement {
    let state = workspace.update_state();
    let running = state.running();
    let mut row = h_flex().gap_2().items_center().justify_end();

    if state.release_url().is_some()
        && matches!(
            state.phase,
            UpdatePhase::CannotSelfUpdate | UpdatePhase::Failed { .. }
        )
    {
        row = row.child(
            Button::new("update-release-page")
                .ghost()
                .small()
                .label(strings::UI_UPDATE_RELEASE_PAGE)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.open_update_release_page(cx);
                })),
        );
    }

    if state.can_cancel() {
        row = row.child(
            Button::new("update-cancel")
                .ghost()
                .small()
                .label(strings::UI_UPDATE_CANCEL)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.cancel_update(cx);
                })),
        );
    }

    if state.can_restart() {
        row = row.child(
            Button::new("update-restart")
                .primary()
                .small()
                .label(strings::UI_UPDATE_READY_RESTART)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.retry_restart(cx);
                })),
        );
    } else if state.can_retry() && !running {
        row = row.child(
            Button::new("update-retry")
                .primary()
                .small()
                .label(strings::UI_UPDATE_RETRY)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.retry_update(cx);
                })),
        );
    }

    if running {
        row = row.child(
            Label::new(strings::UI_UPDATE_IN_PROGRESS)
                .id(SharedString::from("update-busy"))
                .text_xs()
                .text_color(theme.muted_foreground),
        );
    }

    row.into_any_element()
}

/// One typed failure line: code plus the message.
fn error_line(code: &str, message: &str, theme: &Theme) -> AnyElement {
    h_flex()
        .id(SharedString::from(format!("update-error-{code}")))
        .gap_2()
        .items_start()
        .child(icon(IconName::TriangleAlert, 14.0, theme.danger))
        .child(
            Label::new(format!("{code} — {message}"))
                .id(SharedString::from(format!("update-error-text-{code}")))
                .text_xs()
                .text_color(theme.danger),
        )
        .into_any_element()
}
