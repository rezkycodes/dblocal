//! The command palette, built on `gpui_kit::component::command::Command`.
//!
//! A filtered list of actions over the current instances (`F-PLH-01` AC2).
//! `CommandState` owns the query, filtering and keyboard navigation; we only
//! supply the entries and run the confirmed action. `Ctrl/Cmd+K` stays from
//! W1-G and the palette is restyled in the prototype's language.
//!
//! Every instance offers start/stop/open/copy-URI. A command that does not
//! apply right now (start on a running instance, stop on a stopped one) is
//! listed but **disabled with its reason as text**, so the palette explains
//! itself instead of hiding rows and the reason is never colour alone.

use gpui_kit::component::{
    ActiveTheme, Disableable as _,
    command::{Command, CommandGroup, CommandItem},
};
use gpui_kit::{IntoElement, ParentElement, SharedString, Styled, div, prelude::*};

use crate::theme::tokens;

use super::Workspace;

/// Renders the command palette overlay.
pub(super) fn render(
    workspace: &Workspace,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::Context<Workspace>,
) -> impl IntoElement {
    use gpui_kit::component::FocusTrapElement as _;
    let theme = cx.theme().clone();
    // `F2`: the palette is a modal variant, so a focus trap keeps Tab inside
    // its query field and rows instead of walking the page behind the scrim.
    // The container handle comes from `open_palette` (`begin_modal`).
    let trap = workspace.modal_focus().unwrap_or_else(|| {
        window
            .use_keyed_state(SharedString::from("modal-focus-palette"), cx, |_, cx| {
                cx.focus_handle()
            })
            .read(cx)
            .clone()
    });
    let items = workspace.palette_items();
    let entries = items.into_iter().map(|item| {
        let label = SharedString::from(item.label.clone());
        let mut command_item = CommandItem::new().label(label.clone());
        match item.action {
            Some(_) => command_item = command_item.keywords(item.keywords),
            None => {
                // A disabled row shows its label plus its reason as muted text
                // and never runs: the reason is a plain sentence, so the "this
                // does not apply" signal is text, never colour alone
                // (F-PLH-01 AC2).
                let reason = item.reason.unwrap_or_default();
                command_item = command_item.child(move |_, cx| {
                    let muted = cx.theme().muted_foreground;
                    gpui_kit::component::h_flex()
                        .gap_2()
                        .items_center()
                        .child(label.clone())
                        .child(SharedString::from(reason.clone()))
                        .text_color(muted)
                });
                command_item = command_item.disabled(true);
            }
        }
        command_item
    });
    let group = CommandGroup::new()
        .label(dblocal_core::strings::UI_PALETTE_PLACEHOLDER)
        .items(entries);
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
                    this.close_palette(cx);
                    cx.notify();
                });
            }
        });

    div()
        .id("palette-overlay")
        .absolute()
        .inset_0()
        .flex()
        // `items_start` is the fix for the panel stretching to the window
        // bottom: a flex row's default cross-axis alignment is `stretch`, so
        // without it the panel filled the whole height below `pt(90)` and the
        // white column hid the page even with one row left. The panel's height
        // now follows its content (`Command`'s `max_h(360)` bounds the list).
        .items_start()
        .justify_center()
        .pt(gpui_kit::px(90.))
        .bg(tokens::scrim_from(&theme))
        .on_mouse_down(
            gpui_kit::MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.close_palette(cx);
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
                .child(palette)
                .focus_trap(SharedString::from("trap-palette"), &trap),
        )
        .into_any_element()
}
