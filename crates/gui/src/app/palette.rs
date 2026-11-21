//! The command palette, built on `gpui_kit::component::command::Command`.
//!
//! A filtered list of actions over the current instances (`F-PLH-01` AC2).
//! `CommandState` owns the query, filtering and keyboard navigation; we only
//! supply the entries and run the confirmed action. `Ctrl/Cmd+K` stays from
//! W1-G and the palette is restyled in the prototype's language.

use gpui_kit::component::{
    ActiveTheme,
    command::{Command, CommandGroup, CommandItem},
};
use gpui_kit::{IntoElement, ParentElement, SharedString, Styled, div, prelude::*};

use crate::theme::tokens;

use super::Workspace;

/// Renders the command palette overlay.
pub(super) fn render(
    workspace: &Workspace,
    cx: &mut gpui_kit::Context<Workspace>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let labels = workspace.palette_labels();
    let items = labels
        .iter()
        .map(|label| CommandItem::new().label(SharedString::from(label.clone())));
    let group = CommandGroup::new()
        .label(dblocal_core::strings::UI_PALETTE_PLACEHOLDER)
        .items(items);
    let state = workspace.command_state().clone();
    let owner = cx.weak_entity();

    let palette = Command::new(&state)
        .group(group)
        .placeholder(dblocal_core::strings::UI_PALETTE_PLACEHOLDER)
        .max_h(gpui_kit::px(360.))
        .on_confirm({
            let owner = owner.clone();
            move |index: gpui_kit::component::IndexPath,
                  window: &mut gpui_kit::Window,
                  cx: &mut gpui_kit::App| {
                let _ = owner.update(cx, |this, cx| {
                    this.run_palette_action(index.row, window, cx);
                    cx.notify();
                });
            }
        })
        .on_cancel({
            let owner = owner.clone();
            move |_window: &mut gpui_kit::Window, cx: &mut gpui_kit::App| {
                let _ = owner.update(cx, |this, cx| {
                    this.set_palette_open(false);
                    cx.notify();
                });
            }
        });

    div()
        .id("palette-overlay")
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .pt(gpui_kit::px(90.))
        .bg(tokens::scrim_from(&theme))
        .on_mouse_down(
            gpui_kit::MouseButton::Left,
            cx.listener(|this, _, _window, cx| {
                this.set_palette_open(false);
                cx.notify();
            }),
        )
        .child(
            div()
                .id("palette-panel")
                .w(gpui_kit::px(560.))
                .rounded(gpui_kit::px(8.))
                .bg(theme.popover)
                .text_color(theme.popover_foreground)
                .border_1()
                .border_color(theme.border)
                .overflow_hidden()
                .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation()
                })
                .child(palette),
        )
        .into_any_element()
}
