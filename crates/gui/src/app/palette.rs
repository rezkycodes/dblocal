//! Command palette built on `gpui_component::command::Command`.
//!
//! A filtered list of actions over the current instances (`F-PLH-01` AC2).
//! `CommandState` owns the query, filtering and keyboard navigation; we only
//! supply the entries and run the confirmed action.

use gpui::{IntoElement, ParentElement, SharedString, Styled, div, prelude::*};
use gpui_component::{
    ActiveTheme,
    command::{Command, CommandGroup, CommandItem},
};

use super::Workspace;

/// Renders the command palette overlay.
pub(super) fn render(workspace: &Workspace, cx: &mut gpui::Context<Workspace>) -> impl IntoElement {
    let theme = cx.theme();
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
        .max_h(gpui::px(360.))
        .on_confirm({
            let owner = owner.clone();
            move |index: gpui_component::IndexPath,
                  window: &mut gpui::Window,
                  cx: &mut gpui::App| {
                let _ = owner.update(cx, |this, cx| {
                    this.run_palette_action(index.row, window, cx);
                    cx.notify();
                });
            }
        })
        .on_cancel({
            let owner = owner.clone();
            move |_window: &mut gpui::Window, cx: &mut gpui::App| {
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
        .pt(gpui::px(90.))
        .bg(gpui::hsla(0.0, 0.0, 0.0, 0.45))
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _window, cx| {
                this.set_palette_open(false);
                cx.notify();
            }),
        )
        .child(
            div()
                .id("palette-panel")
                .w(gpui::px(560.))
                .rounded(gpui::px(8.))
                .bg(theme.popover)
                .text_color(theme.popover_foreground)
                .border_1()
                .border_color(theme.border)
                .overflow_hidden()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(palette),
        )
        .into_any_element()
}
