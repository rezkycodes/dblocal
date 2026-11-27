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
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled,
    Window, div, prelude::*, px,
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
    // Each mode card is a tab stop with the ring and Enter/Space activation, so a
    // keyboard-only user can reach purge mode (W4-I: the cards used to only
    // listen for the mouse, leaving "Delete instance and data" unreachable by key).
    let [archive_focus, purge_focus] = workspace.delete_mode_focus();

    let modes = v_flex()
        .gap_2()
        .child(mode_card(
            DeleteMode::ArchiveData,
            strings::DELETE_ARCHIVE,
            strings::UI_DELETE_ARCHIVE_HINT,
            format!("~{}", format::format_bytes(size)),
            mode,
            &archive_focus,
            &theme,
            window,
            cx,
        ))
        .child(mode_card(
            DeleteMode::PurgeData,
            strings::DELETE_PURGE,
            strings::UI_DELETE_PURGE_HINT,
            format!("~{}", format::format_bytes(size)),
            mode,
            &purge_focus,
            &theme,
            window,
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

/// One selectable delete mode: title, hint, size, the selected border, and a
/// keyboard path (focus ring, Tab stop, Enter/Space activation).
#[allow(clippy::too_many_arguments)]
fn mode_card(
    candidate: DeleteMode,
    label: &str,
    hint: &str,
    size: String,
    mode: DeleteMode,
    focus: &gpui_kit::FocusHandle,
    theme: &gpui_kit::component::Theme,
    window: &Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let selected = candidate == mode;
    let card = div()
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
        .when(selected, |this| {
            this.bg(crate::theme::tokens::active(theme))
        })
        // `on_click` (not `on_mouse_down`) so gpui synthesises Enter/Space from
        // the focused element as well as a pointer click.
        .on_click(cx.listener(move |this, _, _, cx| {
            this.set_delete_mode(candidate);
            cx.notify();
        }))
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
        );
    // `apply_inset_focus_ring` adds the Tab stop and the 2px inset `theme.ring`
    // outline; the card sits inside the dialog's bordered panel, so the inset
    // variant cannot be clipped.
    super::components::apply_inset_focus_ring(card, focus, window, cx)
}
