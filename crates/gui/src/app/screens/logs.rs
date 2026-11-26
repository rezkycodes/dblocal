//! The Logs tab: the per-instance buffer with follow, copy, and the diagnosis
//! banner for a failed start (`F-LOG-01`, `F-LOG-02`).
//!
//! The banner offers the one-click "Move to <port> and start" only
//! when the failure is the typed port-in-use error, implemented as
//! `suggest_port` + `change_port` + `start_instance` (`F-PRT-02` AC1–AC2).

use dblocal_core::strings;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::label::Label;
use gpui_kit::component::{
    ActiveTheme, Sizable, Theme, WindowExt, h_flex, notification::Notification, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{Context, IntoElement, ParentElement, SharedString, Styled, div, px};

use crate::format;
use crate::logs::LogBuffer;

use super::Workspace;
use super::page_fill;
use crate::app::components::{banner, icon, mono_themed};

/// Height of one log row, in logical px (W4-I D3). `UniformList` gives every row
/// this height, which is why a line must never wrap into the next row.
const LOG_ROW_HEIGHT: f32 = 18.0;

impl Workspace {
    pub(crate) fn render_logs(
        &self,
        view: &dblocal_core::service::InstanceView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut body = page_fill();
        if format::offers_port_fix(view) {
            body = body.child(self.render_diagnosis_banner(view, &theme, cx));
        }
        body.child(self.render_log_viewer(view, &theme, cx))
    }

    /// The failed-start banner: code, diagnosis, the fix and the report action.
    fn render_diagnosis_banner(
        &self,
        view: &dblocal_core::service::InstanceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let suggested = self.instance_port_proposal(&view.id);
        let id_fix = view.id.clone();
        let failure = view.failure.clone();
        let (code, message) = match failure {
            Some(failure) => (failure.code, failure.message),
            None => (String::new(), String::new()),
        };
        let mono = theme.mono_font_family.clone();
        banner(theme.danger)
            .child(icon(IconName::TriangleAlert, 20.0, theme.danger))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(280.))
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Label::new(strings::diagnosis_port_heading(view.port.get()))
                                    .text_sm()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                            )
                            .child(
                                Label::new(code)
                                    .text_xs()
                                    .font_family(mono.clone())
                                    .text_color(theme.muted_foreground),
                            ),
                    )
                    .child(
                        Label::new(strings::diagnosis_port_body(view.port.get(), suggested))
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    )
                    .when(!message.is_empty(), |this| {
                        this.child(
                            Label::new(message)
                                .text_xs()
                                .text_color(theme.muted_foreground),
                        )
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(
                        Button::new("move-and-start")
                            .primary()
                            .label(strings::diagnosis_move_and_start(suggested))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.move_to_port_and_start(&id_fix, suggested, cx);
                            })),
                    )
                    .child(
                        Button::new("report-issue")
                            .label(strings::UI_DIAGNOSIS_REPORT)
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.push_notification(
                                    Notification::info(strings::UI_DIAGNOSIS_REPORT_READY),
                                    cx,
                                );
                            })),
                    ),
            )
    }

    /// The log panel: header with follow and copy, then the virtualised lines.
    fn render_log_viewer(
        &self,
        view: &dblocal_core::service::InstanceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let buffer = self.log_lines(&view.id);
        let count = buffer.len();
        let following = buffer.is_following();
        let follow_word = if following {
            strings::UI_LOGS_FOLLOWING
        } else {
            strings::UI_LOGS_PAUSED
        };
        let path = format::engine_log_path(self.home_root(), self.home_dir(), &view.name);

        let mut viewer = v_flex()
            .id("log-viewer")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .px(px(10.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(theme.table_row_border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Label::new(strings::UI_LOGS_SHOW_FILE)
                                    .text_sm()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                            )
                            .child(Label::new(follow_word).text_xs().text_color(if following {
                                theme.success
                            } else {
                                theme.muted_foreground
                            })),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("toggle-follow")
                                    .xsmall()
                                    .compact()
                                    .label(if following {
                                        strings::UI_LOGS_PAUSED
                                    } else {
                                        strings::UI_LOGS_FOLLOWING
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_follow();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("copy-logs")
                                    .xsmall()
                                    .compact()
                                    .icon(IconName::Copy)
                                    .label(strings::UI_LOGS_COPY)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.copy_logs(window, cx);
                                    })),
                            ),
                    ),
            );

        let body = if count == 0 {
            div()
                .p(px(16.))
                .child(
                    Label::new(strings::UI_LOGS_EMPTY)
                        .text_sm()
                        .text_color(theme.muted_foreground),
                )
                .into_any_element()
        } else {
            self.render_log_lines(&buffer, theme)
        };
        viewer = viewer.child(
            div()
                .id("log-lines-wrap")
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .p(px(8.))
                .child(body),
        );
        viewer.child(
            h_flex()
                .gap_4()
                .px(px(14.))
                .py(px(6.))
                .border_t_1()
                .border_color(theme.table_row_border)
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(mono_themed(path, theme))
                .child(Label::new(strings::UI_LOGS_ROTATION))
                .child(div().flex_1())
                .child(Label::new(format!(
                    "{count} {}",
                    strings::UI_LOGS_LINE_COUNT
                ))),
        )
    }

    /// A virtualised list over the buffer, so a frame clones pointers only.
    ///
    /// `size_full` gives the list a definite height inside the flex viewer; without
    /// it the default `Auto` sizing collapses to zero in a `min_h_0` chain and no
    /// lines paint (the "8 lines" footer proved the buffer had lines).
    ///
    /// Rows are **one line, never wrapped** (W4-I D3): `UniformList` gives every
    /// row the same height, so a wrapped line painted a second visual line into
    /// the next row's box and the two overprinted (PostgreSQL's ~120-char
    /// `LOG: ... listening on IPv4 address` line was unreadable). The row is
    /// clipped and its text ellipsised; horizontal scrolling was rejected because
    /// it would need a second scroll axis inside the uniform virtual list and
    /// break the fixed-row-height model it relies on. The full line is still
    /// available: "Copy logs" copies the complete buffer lines unchanged.
    fn render_log_lines(&self, buffer: &LogBuffer, theme: &Theme) -> gpui_kit::AnyElement {
        let lines = buffer.clone();
        let mono = theme.mono_font_family.clone();
        let count = lines.len();
        let scroll = self.log_scroll();
        gpui_kit::uniform_list("log-lines", count, move |range, _window, _cx| {
            range
                .map(|index| {
                    let text = lines
                        .line(index)
                        .cloned()
                        .unwrap_or_else(|| SharedString::from(""));
                    div()
                        .id(("log-line", index))
                        .h(px(LOG_ROW_HEIGHT))
                        .w_full()
                        .min_w_0()
                        .px(px(8.))
                        .overflow_hidden()
                        .child(
                            Label::new(text)
                                .font_family(mono.clone())
                                .text_xs()
                                .w_full()
                                .truncate(),
                        )
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&scroll)
        .size_full()
        .into_any_element()
    }
}
