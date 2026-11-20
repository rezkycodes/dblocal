//! Create-instance modal (`F-INS-01`, `F-CAT-02`).
//!
//! A real form on `InputState`: engine and version come from the catalog read
//! model (unavailable combinations are shown disabled with their reason), and
//! the name and port fields validate through the same rules the service
//! enforces. The modal is an in-tree overlay so it can read the workspace
//! entity directly.

use dblocal_core::model::AuthMode;
use dblocal_core::strings;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, MouseButton, ParentElement,
    SharedString, Styled, div, prelude::*, px,
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

/// Renders the create-instance modal when a form is open.
pub(super) fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let theme = cx.theme().clone();
    let Some(form) = workspace.create_form().cloned() else {
        return div().into_any_element();
    };
    let catalog = workspace.catalog().clone();

    let mut engines = h_flex().gap_2().flex_wrap();
    let entries = catalog.engines.clone();
    for entry in &entries {
        let engine = entry.engine;
        let selected = engine == form.engine;
        let disabled = !entry.available;
        let mut cell = v_flex().gap_1();
        let mut button = Button::new(SharedString::from(format!("engine-{}", engine.as_str())))
            .label(engine.display_name())
            .small()
            .compact()
            .disabled(disabled);
        button = if selected {
            button.primary()
        } else {
            button.secondary()
        };
        button = button.on_click(cx.listener(move |this, _, _window, cx| {
            this.choose_engine_action(engine);
            cx.notify();
        }));
        cell = cell.child(button);
        if let Some(reason) = format::engine_unavailable_reason(entry) {
            cell = cell.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(reason.to_owned()),
            );
        }
        engines = engines.child(cell);
    }

    let versions = entries
        .iter()
        .find(|entry| entry.engine == form.engine)
        .map(|entry| entry.versions.clone())
        .unwrap_or_default();
    let mut version_rows = v_flex().gap_1();
    for version in versions {
        let selected = version.version == form.version;
        let tag = version.version.clone();
        let mut labels = h_flex()
            .items_center()
            .gap_2()
            .child(div().text_sm().child(tag.clone()));
        if version.installed {
            labels = labels.child(
                div()
                    .text_xs()
                    .text_color(theme.success)
                    .child(strings::UI_VERSION_INSTALLED),
            );
        }
        if version.eol {
            labels = labels.child(
                div()
                    .text_xs()
                    .text_color(theme.warning)
                    .child(strings::UI_VERSION_EOL),
            );
        }
        version_rows = version_rows.child(
            h_flex()
                .id(SharedString::from(format!("version-{tag}")))
                .items_center()
                .justify_between()
                .px_2()
                .py(px(4.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(selected, |this| this.bg(theme.accent))
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.choose_version_action(tag.clone());
                    cx.notify();
                }))
                .child(labels)
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format::format_bytes(version.size_bytes)),
                ),
        );
    }

    let mut auth_row = h_flex().gap_2();
    for (mode, label) in [
        (AuthMode::Password, strings::UI_AUTH_PASSWORD),
        (AuthMode::None, strings::UI_AUTH_NONE),
    ] {
        let selected = form.auth == mode;
        let mut button = Button::new(SharedString::from(format!("auth-{}", mode.as_str())))
            .label(label)
            .small()
            .compact();
        button = if selected {
            button.primary()
        } else {
            button.secondary()
        };
        button = button.on_click(cx.listener(move |this, _, _window, cx| {
            this.choose_auth_action(mode);
            cx.notify();
        }));
        auth_row = auth_row.child(button);
    }

    let suggested = form
        .suggested_port
        .map(|port| port.to_string())
        .unwrap_or_default();

    let mut body = v_flex()
        .gap_3()
        .p_4()
        .child(field(strings::UI_FIELD_ENGINE, engines))
        .child(field(strings::UI_FIELD_VERSION, version_rows))
        .child(field(
            strings::UI_FIELD_NAME,
            Input::new(&form.name_input).small(),
        ))
        .child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .child(format!("{}   ({suggested})", strings::UI_FIELD_PORT)),
                )
                .child(Input::new(&form.port_input).small()),
        )
        .child(field(strings::UI_FIELD_AUTH, auth_row));

    if form.auth == AuthMode::None {
        body = body.child(
            div()
                .text_xs()
                .p_2()
                .rounded(px(4.))
                .bg(theme.warning)
                .text_color(theme.warning_foreground)
                .child(strings::UI_AUTH_NONE_WARNING),
        );
    }

    body = body.child(
        h_flex()
            .justify_end()
            .gap_2()
            .child(
                Button::new("create-cancel")
                    .label(strings::UI_CANCEL)
                    .compact()
                    .ghost()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.cancel_create_action();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("create-submit")
                    .label(strings::UI_CREATE_SUBMIT)
                    .compact()
                    .primary()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.submit_create_action(cx);
                    })),
            ),
    );

    overlay(
        "create-overlay",
        "create-panel",
        div()
            .w(px(560.))
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
                    .child(strings::UI_CREATE_TITLE),
            )
            .child(body),
        cx,
    )
}

fn field(label: &str, content: impl IntoElement) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_xs().child(label.to_owned()))
        .child(content)
}

/// Shared modal chrome: a full-screen backdrop that closes on click, and a
/// centred panel that swallows clicks.
pub(super) fn overlay(
    overlay_id: &'static str,
    panel_id: &'static str,
    panel: impl IntoElement,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    gpui::deferred(
        div()
            .id(overlay_id)
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_center()
            .bg(gpui::hsla(0.0, 0.0, 0.0, 0.45))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close_modals_action();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id(panel_id)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(panel),
            ),
    )
    .with_priority(10)
    .into_any_element()
}
