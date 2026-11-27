//! The confirm-remove-runtime modal (`F-RTM-04`).
//!
//! Opened from an unused runtime's row in the dock. It names the engine and
//! version, the space to be freed, and says the runtime can be downloaded again;
//! two buttons run or cancel it. It follows the delete-dialog shape and uses the
//! same overlay, focus-trap, theme tokens and button styles.

use dblocal_core::model::Runtime;
use dblocal_core::strings;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, Styled, Window, div, px};

use crate::format;

use super::Workspace;
use super::create_dialog::overlay;

/// Renders the runtime-removal dialog when a target is selected.
pub(super) fn render(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let Some(runtime) = workspace.runtime_remove_runtime() else {
        return div().into_any_element();
    };
    let title = format!("{} {}", runtime.engine().display_name(), runtime.version());
    let size = format!("~{}", format::format_bytes(runtime.size_bytes()));

    let body = v_flex()
        .gap_3()
        .p(px(20.))
        .child(
            Label::new(format!("{} — {title}", strings::UI_RUNTIME_REMOVE_TITLE))
                .text_sm()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD),
        )
        .child(
            Label::new(strings::UI_RUNTIME_REMOVE_BODY)
                .text_xs()
                .text_color(theme.muted_foreground),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Label::new(strings::UI_RUNTIME_REMOVE_SPACE)
                        .text_xs()
                        .text_color(theme.muted_foreground),
                )
                .child(Label::new(size).text_xs()),
        )
        .child(
            h_flex()
                .justify_end()
                .gap_2()
                .child(
                    Button::new("runtime-remove-cancel")
                        .label(strings::UI_CANCEL)
                        .compact()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_modals_action();
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("runtime-remove-confirm")
                        .label(strings::UI_RUNTIME_REMOVE_CONFIRM)
                        .compact()
                        .danger()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_runtime_remove(cx);
                        })),
                ),
        );

    overlay(
        "runtime-remove-overlay",
        "runtime-remove-panel",
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
                        Label::new(strings::UI_RUNTIME_REMOVE_TITLE)
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

/// The runtime the open dialog targets, read from the workspace's cached list.
pub(super) fn target_runtime(workspace: &Workspace) -> Option<Runtime> {
    let id = workspace.runtime_remove_target.as_ref()?;
    workspace
        .runtimes()
        .iter()
        .find(|runtime| runtime.id() == id)
        .cloned()
}
