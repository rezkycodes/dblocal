//! Modals for the Toolchains GUI:
//!
//! * [`render_remove`] — confirmation before removing an installed toolchain
//!   version, explaining the active-version / no-auto-switch rule.
//! * [`render_path_consent`] — explicit consent before modifying shell startup
//!   files or the Windows user `Path`, showing the exact lines and target files.

use dblocal_core::strings;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, Window, div, px,
};

use crate::format;
use crate::theme::tokens;

use super::Workspace;
use super::components::icon;
use super::create_dialog::overlay;
use super::design;
use super::toolchain_state::tool_display_name;

/// Renders the toolchain-version removal confirmation dialog.
pub(super) fn render_remove(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let Some(dlg) = workspace.toolchain.remove_dialog.as_ref() else {
        return div().into_any_element();
    };
    let tool_name = tool_display_name(dlg.tool);
    let label = match dlg.profile {
        Some(p) => format!("{tool_name} {} ({})", dlg.version, p.as_str()),
        None => format!("{tool_name} {}", dlg.version),
    };
    let size = format!("~{}", format::format_bytes(dlg.size_bytes));
    let consequence = dlg.consequence_text();

    let close = Button::new("tc-remove-close")
        .ghost()
        .small()
        .compact()
        .icon(IconName::Close)
        .tooltip(strings::UI_CLOSE)
        .accessibility_label(strings::UI_CLOSE)
        .on_click(cx.listener(|this, _, _, cx| {
            this.close_modals_action();
            cx.notify();
        }));

    let body = v_flex()
        .gap_3()
        .p(px(20.))
        .child(
            Label::new(format!("Remove {label}?"))
                .id("tc-remove-question")
                .text_sm()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD),
        )
        .child(
            Label::new(consequence)
                .id("tc-remove-consequence")
                .text_xs()
                .text_color(if dlg.is_active {
                    theme.warning
                } else {
                    theme.muted_foreground
                }),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Label::new(strings::UI_TOOLCHAIN_REMOVE_SPACE)
                        .id("tc-remove-space-label")
                        .text_xs()
                        .text_color(theme.muted_foreground),
                )
                .child(Label::new(size).id("tc-remove-space-value").text_xs()),
        )
        .child(
            h_flex()
                .justify_end()
                .gap_2()
                .pt_2()
                .child(
                    Button::new("tc-remove-cancel")
                        .label(strings::UI_CANCEL)
                        .compact()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_modals_action();
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("tc-remove-confirm")
                        .label(strings::UI_TOOLCHAIN_REMOVE_CONFIRM)
                        .compact()
                        .danger()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_toolchain_remove(cx);
                        })),
                ),
        );

    overlay(
        "tc-remove-overlay",
        "tc-remove-panel",
        div()
            .w(px(520.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(px(12.))
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        design::dialog_header(
                            "tc-remove",
                            IconName::Trash,
                            strings::UI_TOOLCHAIN_REMOVE_TITLE,
                            Some(&label),
                            &theme,
                        )
                        .flex_1(),
                    )
                    .child(div().pr(px(14.)).child(close)),
            )
            .child(body),
        workspace,
        window,
        cx,
    )
}

/// Renders the Add-to-PATH consent dialog, showing the exact files and lines
/// that will be written before the user confirms.
pub(super) fn render_path_consent(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let path_state = &workspace.toolchain.path;
    let mono = theme.mono_font_family.clone();

    let close = Button::new("tc-path-consent-close")
        .ghost()
        .small()
        .compact()
        .icon(IconName::Close)
        .tooltip(strings::UI_CLOSE)
        .accessibility_label(strings::UI_CLOSE)
        .on_click(cx.listener(|this, _, _, cx| {
            this.close_modals_action();
            cx.notify();
        }));

    let mut files_list = v_flex().gap_1();
    if path_state.target_files.is_empty() {
        files_list = files_list.child(
            Label::new(strings::UI_TOOLCHAIN_PATH_CONSENT_NO_FILES)
                .id("tc-path-no-files")
                .text_xs()
                .text_color(theme.muted_foreground),
        );
    } else {
        for (i, file) in path_state.target_files.iter().enumerate() {
            files_list = files_list.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(icon(IconName::File, 12.0, theme.muted_foreground))
                    .child(
                        Label::new(file.clone())
                            .id(SharedString::from(format!("tc-path-file-{i}")))
                            .text_xs()
                            .font_family(mono.clone()),
                    ),
            );
        }
    }

    let preview_lines: Vec<String> = path_state
        .block_preview
        .lines()
        .map(str::to_owned)
        .collect();
    let mut code_well = v_flex()
        .gap_0p5()
        .p(px(12.))
        .rounded(px(8.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.background);
    for (i, line) in preview_lines.iter().enumerate() {
        code_well = code_well.child(
            Label::new(line.clone())
                .id(SharedString::from(format!("tc-path-preview-line-{i}")))
                .text_xs()
                .font_family(mono.clone()),
        );
    }

    let body = v_flex()
        .gap_3()
        .p(px(20.))
        .child(
            Label::new(strings::UI_TOOLCHAIN_PATH_CONSENT_DESC)
                .id("tc-path-consent-desc")
                .text_xs()
                .text_color(theme.muted_foreground),
        )
        .child(
            v_flex()
                .gap_1p5()
                .child(
                    Label::new(strings::UI_TOOLCHAIN_PATH_CONSENT_FILES)
                        .id("tc-path-files-heading")
                        .text_xs()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                )
                .child(
                    div()
                        .p(px(10.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(tokens::hover(&theme))
                        .child(files_list),
                ),
        )
        .child(
            v_flex()
                .gap_1p5()
                .child(
                    Label::new(strings::UI_TOOLCHAIN_PATH_CONSENT_LINES)
                        .id("tc-path-block-heading")
                        .text_xs()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                )
                .child(code_well),
        )
        .child(
            h_flex()
                .justify_end()
                .gap_2()
                .pt_2()
                .child(
                    Button::new("tc-path-consent-cancel")
                        .label(strings::UI_CANCEL)
                        .compact()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_modals_action();
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("tc-path-consent-confirm")
                        .label(strings::UI_TOOLCHAIN_PATH_CONSENT_CONFIRM)
                        .compact()
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_apply_path(cx);
                        })),
                ),
        );

    overlay(
        "tc-path-consent-overlay",
        "tc-path-consent-panel",
        div()
            .w(px(600.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(px(12.))
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        design::dialog_header(
                            "tc-path-consent",
                            IconName::Terminal,
                            strings::UI_TOOLCHAIN_PATH_CONSENT_TITLE,
                            Some(strings::UI_TOOLCHAIN_PATH_CONSENT_SUBTITLE),
                            &theme,
                        )
                        .flex_1(),
                    )
                    .child(div().pr(px(14.)).child(close)),
            )
            .child(body),
        workspace,
        window,
        cx,
    )
}
