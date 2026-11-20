//! Delete-instance modal (`F-INS-05`, decision D-18).
//!
//! Two explicit actions: purge the data (requires typing the instance name) or
//! archive it. The dialog shows the current data-directory size.

use dblocal_core::model::DeleteMode;
use dblocal_core::strings;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, SharedString, Styled,
    div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, Disableable as _, Sizable,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Input,
    v_flex,
};

use crate::format;

use super::Workspace;
use super::create_dialog::overlay;

/// Renders the delete modal when a target is selected.
pub(super) fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let theme = cx.theme().clone();
    let Some(name) = workspace.delete_target_name(cx) else {
        return div().into_any_element();
    };
    let mode = workspace.delete_mode();
    let typed_matches = workspace.delete_confirmation_ok(cx);
    let size = workspace.delete_size();
    let can_purge = mode == DeleteMode::PurgeData && typed_matches;

    let mut modes = v_flex().gap_2();
    for (candidate, label, detail) in [
        (
            DeleteMode::ArchiveData,
            strings::DELETE_ARCHIVE,
            format!("~{}", format::format_bytes(size)),
        ),
        (
            DeleteMode::PurgeData,
            strings::DELETE_PURGE,
            format!("~{}", format::format_bytes(size)),
        ),
    ] {
        let selected = candidate == mode;
        modes = modes.child(
            div()
                .id(SharedString::from(format!(
                    "delete-mode-{}",
                    candidate.as_str()
                )))
                .p_2()
                .rounded(px(4.))
                .cursor_pointer()
                .border_1()
                .border_color(if selected { theme.accent } else { theme.border })
                .when(selected, |this| this.bg(theme.muted))
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.set_delete_mode(candidate);
                    cx.notify();
                }))
                .child(
                    v_flex().gap_1().child(div().text_sm().child(label)).child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(detail),
                    ),
                ),
        );
    }

    let mut body = v_flex()
        .gap_3()
        .p_4()
        .child(
            div()
                .text_sm()
                .child(format!("{} — {}", strings::UI_DELETE_TITLE, name)),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(strings::UI_DELETE_PROMPT),
        )
        .child(modes);

    if mode == DeleteMode::PurgeData {
        body = body
            .child(
                div()
                    .text_xs()
                    .text_color(theme.danger)
                    .child(strings::UI_DELETE_TYPE_NAME),
            )
            .child(match workspace.delete_name_input() {
                Some(input) => Input::new(input).small().into_any_element(),
                None => div().into_any_element(),
            });
    }

    body = body.child(
        h_flex()
            .justify_end()
            .gap_2()
            .child(
                Button::new("delete-cancel")
                    .label(strings::UI_CANCEL)
                    .compact()
                    .ghost()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.close_modals_action();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("delete-confirm")
                    .label(strings::UI_DELETE)
                    .compact()
                    .danger()
                    .disabled(!can_purge && mode == DeleteMode::PurgeData)
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.confirm_delete_action(cx);
                    })),
            ),
    );

    overlay(
        "delete-overlay",
        "delete-panel",
        div()
            .w(px(480.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .overflow_hidden()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(strings::UI_DELETE_TITLE),
            )
            .child(body),
        cx,
    )
}
