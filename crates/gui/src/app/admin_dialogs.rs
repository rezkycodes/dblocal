//! Dialogs for the Databases tab: drop confirmation, reset confirmation and the
//! one-time secret (`F-DBU-01` AC3, `F-DBU-02` AC2, `F-DBU-03`).
//!
//! Each dialog carries an element id derived from the instance (and the
//! database name for the drop dialog), so two instances with a database of the
//! same name cannot collide in the accessibility tree — the exact trap W2-B2
//! fixed. Nothing here holds a secret after the dialog closes: the value lives
//! in [`SecretDialog`] and is dropped with it.

use dblocal_core::model::InstanceId;
use dblocal_core::secret::SecretString;
use dblocal_core::strings;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, Entity, IntoElement, ParentElement, SharedString, Styled, Window, div, px,
};

use super::Workspace;
use super::admin_state::{dialog_id, dialog_row_id};
use super::create_dialog::overlay;

/// The type-the-name drop confirmation of one database.
#[derive(Clone)]
pub struct DropDialog {
    /// Instance that owns the database.
    pub instance: InstanceId,
    /// Exact name the user must type.
    pub name: String,
    /// The confirmation field.
    pub input: Entity<InputState>,
}

/// What the one-time value is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretKind {
    /// A freshly created user's password (`F-DBU-02` AC2).
    UserPassword {
        /// The role the password belongs to.
        user: String,
    },
    /// The new superuser URI after a reset (`F-DBU-03` AC1).
    ResetUri,
}

/// The one-time value dialog: never reopened from state, dropped on close.
#[derive(Clone)]
pub struct SecretDialog {
    /// Instance the value belongs to (unused by the text, kept for its ids).
    pub instance: InstanceId,
    /// What this value is.
    pub kind: SecretKind,
    /// The value, shown once.
    pub value: SecretString,
}

impl SecretDialog {
    /// The dialog's title line, naming the instance-side subject.
    pub fn subject(&self) -> String {
        match &self.kind {
            SecretKind::UserPassword { user } => {
                format!("{} — {user}", strings::UI_DATABASES_SECRET_TITLE)
            }
            SecretKind::ResetUri => strings::UI_DATABASES_SECRET_TITLE.to_owned(),
        }
    }

    /// The label above the value.
    pub fn value_label(&self) -> &'static str {
        match self.kind {
            SecretKind::UserPassword { .. } => strings::UI_DATABASES_SECRET_VALUE,
            SecretKind::ResetUri => strings::UI_DATABASES_SECRET_URI,
        }
    }
}

/// The drop-database confirmation (`F-DBU-01` AC3).
pub(super) fn render_drop(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let Some(dialog) = workspace.drop_dialog().cloned() else {
        return div().into_any_element();
    };
    let confirm_ok = workspace.drop_confirmation_ok(cx);
    let name = dialog.name.clone();
    let ids = dialog_row_id("drop-db", &dialog.instance, &dialog.name);

    let body = v_flex()
        .gap_3()
        .p(px(20.))
        .child(
            Label::new(strings::UI_DATABASES_DROP_PROMPT)
                .id(SharedString::from(format!("{ids}-prompt")))
                .text_xs()
                .text_color(theme.muted_foreground),
        )
        // One distinct danger line under the explanation: the sentence above is
        // rendered once, and the heading names the database instead of repeating
        // the title (F4).
        .child(
            Label::new(strings::UI_DATABASES_DROP_WARNING)
                .id(SharedString::from(format!("{ids}-warning")))
                .text_xs()
                .text_color(theme.danger),
        )
        .child(Input::new(&dialog.input).small())
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
                        .label(strings::UI_DATABASES_DROP_CONFIRM)
                        .compact()
                        .danger()
                        .disabled(!confirm_ok)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.drop_confirmation_ok(cx) {
                                this.confirm_drop(cx);
                            }
                        })),
                ),
        );

    overlay(
        "drop-db-overlay",
        "drop-db-panel",
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
                        Label::new(strings::UI_DATABASES_DROP_TITLE)
                            .id(SharedString::from(format!("{ids}-heading")))
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                    )
                    .child(div().flex_1())
                    .child(
                        // The heading names the database; the body no longer
                        // repeats it (F4).
                        Label::new(name)
                            .id(SharedString::from(format!("{ids}-subject")))
                            .text_sm()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground),
                    ),
            )
            .child(body),
        workspace,
        window,
        cx,
    )
}

/// The reset-password confirmation (`F-DBU-03`).
pub(super) fn render_reset(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let Some(id) = workspace.reset_confirm().cloned() else {
        return div().into_any_element();
    };
    let ids = dialog_id("reset", &id);

    let body = v_flex()
        .gap_3()
        .p(px(20.))
        .child(
            Label::new(strings::UI_DATABASES_RESET_PROMPT)
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
                        .label(strings::UI_DATABASES_RESET_CONFIRM)
                        .compact()
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_reset(cx);
                        })),
                ),
        );

    overlay(
        "reset-overlay",
        "reset-panel",
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
                        Label::new(strings::UI_DATABASES_RESET_TITLE)
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

/// The one-time secret dialog (`F-DBU-02` AC2, `F-DBU-03` AC1).
pub(super) fn render_secret(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let Some(dialog) = workspace.secret_dialog().cloned() else {
        return div().into_any_element();
    };
    let ids = dialog_id("secret", &dialog.instance);
    let value = dialog.value.expose().to_owned();
    let subject = dialog.subject();
    let value_label = dialog.value_label();

    let body = v_flex()
        .gap_3()
        .p(px(20.))
        .child(
            Label::new(strings::UI_DATABASES_SECRET_ONCE)
                .id(SharedString::from(format!("{ids}-once")))
                .text_xs()
                .text_color(theme.warning),
        )
        .child(
            Label::new(value_label.to_owned())
                .id(SharedString::from(format!("{ids}-value-label")))
                .text_xs()
                .text_color(theme.muted_foreground),
        )
        .child(
            div()
                .px(px(12.))
                .py(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .font_family(theme.mono_font_family.clone())
                .child(Label::new(value).id(SharedString::from(format!("{ids}-value")))),
        )
        .child(
            h_flex()
                .justify_end()
                .gap_2()
                .child(
                    Button::new(SharedString::from(format!("{ids}-copy")))
                        .label(strings::UI_DATABASES_SECRET_COPY)
                        .compact()
                        .icon(gpui_kit::assets::IconName::Copy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.copy_secret_dialog(cx);
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("{ids}-close")))
                        .label(strings::UI_DATABASES_SECRET_CLOSE)
                        .compact()
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_secret_dialog(cx);
                        })),
                ),
        );

    overlay(
        "secret-overlay",
        "secret-panel",
        div()
            .w(px(560.))
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
                        Label::new(subject)
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

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_user_dialog_names_the_role_and_labels_a_password() {
        let dialog = SecretDialog {
            instance: InstanceId::new(),
            kind: SecretKind::UserPassword {
                user: "app_rw".to_owned(),
            },
            value: SecretString::new("pwd-user-1"),
        };
        assert!(dialog.subject().contains("app_rw"));
        assert_eq!(dialog.value_label(), strings::UI_DATABASES_SECRET_VALUE);
    }

    #[test]
    fn a_reset_dialog_labels_a_uri() {
        let dialog = SecretDialog {
            instance: InstanceId::new(),
            kind: SecretKind::ResetUri,
            value: SecretString::new("postgresql://postgres:x@127.0.0.1:5432/postgres"),
        };
        assert_eq!(dialog.value_label(), strings::UI_DATABASES_SECRET_URI);
        assert_eq!(dialog.subject(), strings::UI_DATABASES_SECRET_TITLE);
    }

    #[test]
    fn a_secret_dialog_never_debugs_its_value() {
        let dialog = SecretDialog {
            instance: InstanceId::new(),
            kind: SecretKind::UserPassword {
                user: "u".to_owned(),
            },
            value: SecretString::new("canary-password"),
        };
        // `SecretString` redacts itself, so a stray `{dialog:?}` is safe even
        // though the whole struct derives nothing.
        assert!(!format!("{:?}", dialog.value).contains("canary-password"));
    }

    /// F4: the danger line must not repeat the prompt sentence.
    #[test]
    fn the_drop_dialog_renders_the_explanation_once() {
        assert_ne!(
            strings::UI_DATABASES_DROP_PROMPT,
            strings::UI_DATABASES_DROP_WARNING,
            "the danger line must be a distinct short warning, not the sentence again"
        );
        assert!(
            !strings::UI_DATABASES_DROP_WARNING.contains(strings::UI_DATABASES_DROP_PROMPT),
            "the warning repeats the explanation"
        );
        // Two instances with a database of the same name still get distinct ids
        // for the prompt, the warning and the subject line.
        let one = InstanceId::new();
        let two = InstanceId::new();
        let a = dialog_row_id("drop-db", &one, "app");
        let b = dialog_row_id("drop-db", &two, "app");
        assert_ne!(
            format!("{a}-warning"),
            format!("{b}-warning"),
            "the warning id must carry the instance"
        );
        assert_ne!(format!("{a}-prompt"), format!("{a}-warning"));
    }
}
