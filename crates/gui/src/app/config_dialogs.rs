//! Dialogs for the Config tab: the restore confirmation (`F-CFG-01` AC3).
//!
//! The restore dialog names the newest backup's age and says the current file is
//! kept as a backup too, so the action is explained before it runs. `Escape`
//! closes it through `Workspace::close_modals`, which also returns focus.

use dblocal_core::strings;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, Window, div, px,
};

use super::Workspace;
use super::config_state::config_action_id;
use super::create_dialog::overlay;

/// The restore-confirmation dialog (`F-CFG-01` AC3).
pub(super) fn render_restore(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let Some(id) = workspace.config_restore_confirm().cloned() else {
        return div().into_any_element();
    };
    let ids = config_action_id("config-restore-dialog", &id);
    let backup_age = workspace
        .config_state_for(&id)
        .and_then(|config| config.view())
        .and_then(|view| view.backup_available.as_ref())
        .map(|backup| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            strings::config_backup_age(backup.taken_at_secs, now)
        })
        .unwrap_or_default();

    let body = v_flex()
        .gap_3()
        .p(px(20.))
        .child(
            Label::new(strings::config_restore_prompt(&backup_age))
                .id(SharedString::from(format!("{ids}-prompt")))
                .text_xs()
                .text_color(theme.muted_foreground),
        )
        .child(
            h_flex()
                .justify_end()
                .gap_2()
                .child(
                    Button::new(SharedString::from(format!("{ids}-cancel")))
                        .label(strings::UI_CANCEL)
                        .compact()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_modals();
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("{ids}-confirm")))
                        .label(strings::UI_CONFIG_RESTORE_CONFIRM)
                        .compact()
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_config_restore(cx);
                        })),
                ),
        );

    overlay(
        "config-restore-overlay",
        "config-restore-panel",
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
                        Label::new(strings::UI_CONFIG_RESTORE_TITLE)
                            .id(SharedString::from(format!("{ids}-heading")))
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                    ),
            )
            .child(body),
        workspace,
        window,
        cx,
    )
    .into_any_element()
}
