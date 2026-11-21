//! Title bar: app name, the primary action and the palette hint.
//!
//! Window controls are drawn by `gpui_kit::component::TitleBar`'s client-side
//! decorations; on macOS the native traffic lights are used. The tray seam is
//! here: `Workspace::tray_available` remains `false` until Wave 2 wires it.

use dblocal_core::strings;
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants as _},
    h_flex,
};
use gpui_kit::{FontWeight, InteractiveElement as _, IntoElement, ParentElement, Styled};

use super::Workspace;

use super::actions;

/// Renders the client-side title bar.
pub(super) fn render(
    _workspace: &Workspace,
    cx: &mut gpui_kit::Context<Workspace>,
) -> impl IntoElement {
    let theme = cx.theme();
    let hint = actions::palette_hint();
    h_flex()
        .id("title-bar")
        .h(gpui_kit::px(34.))
        .w_full()
        .items_center()
        .justify_between()
        .bg(theme.title_bar)
        .border_b_1()
        .border_color(theme.title_bar_border)
        .px_3()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    div_label(dblocal_core::strings::APP_NAME, theme.foreground)
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    Button::new("title-new")
                        .label(strings::UI_NEW_INSTANCE)
                        .xsmall()
                        .compact()
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_new_instance(window, cx);
                        })),
                ),
        )
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(div_label(&hint, theme.muted_foreground).text_xs())
                .child(
                    gpui_kit::div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(strings::UI_CATALOG.to_owned()),
                ),
        )
}

fn div_label(text: &str, colour: gpui_kit::Hsla) -> gpui_kit::Div {
    gpui_kit::div().text_color(colour).child(text.to_owned())
}
