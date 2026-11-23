//! The delete-instance modal (`F-INS-05`, decision D-18).
//!
//! Two explicit actions: archive the data (the default) or purge it (requires
//! typing the instance name). The dialog shows the current data-directory size
//! and explains what each mode does.

use dblocal_core::model::DeleteMode;
use dblocal_core::strings;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, input::Input, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    Styled, Window, div, prelude::*, px,
};

use crate::format;

use super::Workspace;
use super::create_dialog::overlay;

/// Renders the delete modal when a target is selected.
pub(super) fn render(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let Some(name) = workspace.delete_target_name(cx) else {
        return div().into_any_element();
    };
    let mode = workspace.delete_mode();
    let typed_matches = workspace.delete_confirmation_ok(cx);
    let size = workspace.delete_size();
    let can_purge = mode == DeleteMode::PurgeData && typed_matches;

    let modes = v_flex()
        .gap_2()
        .child(mode_card(
            DeleteMode::ArchiveData,
            strings::DELETE_ARCHIVE,
            strings::UI_DELETE_ARCHIVE_HINT,
            format!("~{}", format::format_bytes_id(size)),
            mode,
            &theme,
            cx,
        ))
        .child(mode_card(
            DeleteMode::PurgeData,
            strings::DELETE_PURGE,
            strings::UI_DELETE_PURGE_HINT,
            format!("~{}", format::format_bytes_id(size)),
            mode,
            &theme,
            cx,
        ));

    let mut body = v_flex()
        .gap_3()
        .p(px(20.))
        .child(
            Label::new(format!("{} — {name}", strings::UI_DELETE_TITLE))
                .text_sm()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD),
        )
        .child(
            Label::new(strings::UI_DELETE_PROMPT)
                .text_xs()
                .text_color(theme.muted_foreground),
        )
        .child(modes);

    if mode == DeleteMode::PurgeData {
        body = body
            .child(
                Label::new(strings::UI_DELETE_TYPE_NAME)
                    .text_xs()
                    .text_color(theme.danger),
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
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.close_modals_action();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("delete-confirm")
                    .label(strings::UI_DELETE)
                    .compact()
                    .danger()
                    .disabled(mode == DeleteMode::PurgeData && !can_purge)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.confirm_delete_action(cx);
                    })),
            ),
    );

    overlay(
        "delete-overlay",
        "delete-panel",
        div()
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
                    .child(
                        Label::new(strings::UI_DELETE_TITLE)
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                    ),
            )
            .child(body),
        workspace,
        window,
        cx,
    )
}

/// One selectable delete mode: title, hint, size, and the selected border.
#[allow(clippy::too_many_arguments)]
fn mode_card(
    candidate: DeleteMode,
    label: &str,
    hint: &str,
    size: String,
    mode: DeleteMode,
    theme: &gpui_kit::component::Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let selected = candidate == mode;
    div()
        .id(SharedString::from(format!(
            "delete-mode-{}",
            candidate.as_str()
        )))
        .p_3()
        .rounded(px(6.))
        .cursor_pointer()
        .border_1()
        .border_color(if selected {
            theme.primary
        } else {
            theme.border
        })
        .when(selected, |this| this.bg(theme.muted))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.set_delete_mode(candidate);
                cx.notify();
            }),
        )
        .child(
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .justify_between()
                        .child(Label::new(label.to_owned()).text_sm())
                        .child(
                            Label::new(size)
                                .text_xs()
                                .text_color(theme.muted_foreground),
                        ),
                )
                .child(
                    Label::new(hint.to_owned())
                        .text_xs()
                        .text_color(theme.muted_foreground),
                ),
        )
}
