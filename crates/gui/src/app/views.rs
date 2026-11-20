//! The visible shell: sidebar, centre pane, overview and log view.
//!
//! Rendering only. Every branch reads view-model values from [`super::format`]
//! and [`super::logs`]; no business logic lives here.

use dblocal_core::model::{AuthMode, InstanceStatus};
use dblocal_core::service::{FailureInfo, InstanceView, UriFormat};
use dblocal_core::strings;
use gpui::{
    Hsla, InteractiveElement as _, IntoElement, ParentElement, SharedString, Styled, div,
    prelude::*, px,
};
use gpui_component::{
    ActiveTheme, Disableable as _, Sizable,
    button::{Button, ButtonVariants as _},
    h_flex,
    tab::{Tab, TabBar},
    v_flex,
};

use crate::format;
use crate::logs::LogBuffer;

use super::{InnerTab, Workspace};

impl Workspace {
    // ----- Rendering -------------------------------------------------------

    pub(crate) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let groups = format::group_by_engine(&self.state.instances);
        let mut body = v_flex().gap_3().p_2();
        if groups.is_empty() {
            body = body.child(
                div()
                    .px_2()
                    .py_3()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(strings::UI_WELCOME_SUBTITLE),
            );
        }
        for group in groups {
            let mut rows = v_flex().gap_1();
            for view in group.instances {
                rows = rows.child(self.render_instance_row(view, cx));
            }
            body = body.child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .px_2()
                            .pt_1()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground)
                            .child(group.label.to_uppercase()),
                    )
                    .child(rows),
            );
        }

        div()
            .id("sidebar")
            .w(px(260.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.sidebar_border)
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(strings::UI_INSTANCES.to_uppercase()),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(
                div()
                    .p_2()
                    .border_t_1()
                    .border_color(theme.sidebar_border)
                    .child(
                        Button::new("sidebar-new")
                            .label(strings::UI_NEW_INSTANCE_FOOTER)
                            .small()
                            .w_full()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_create_dialog(window, cx);
                            })),
                    ),
            )
    }

    pub(crate) fn render_instance_row(
        &self,
        view: &InstanceView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = self.selected_instance.as_ref() == Some(&view.id);
        let dot = status_colour(view.status, &theme);
        let id = view.id.clone();
        let name = view.name.clone();
        let meta = format::instance_meta(view);
        let label = format::status_label(view.status);
        div()
            .id(SharedString::from(format!("row-{}", view.id)))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py(px(5.))
            .rounded(px(4.))
            .cursor_pointer()
            .when(selected, |this| {
                this.bg(theme.sidebar_accent)
                    .text_color(theme.sidebar_accent_foreground)
            })
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.open_tab(id.clone(), cx);
            }))
            .child(div().size(px(8.)).rounded_full().bg(dot))
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_sm().child(name))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(label),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(meta),
                    ),
            )
    }

    pub(crate) fn render_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if self.open_tabs.is_empty() {
            return div()
                .flex_1()
                .h_full()
                .min_w_0()
                .bg(theme.background)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(strings::UI_WELCOME_TITLE),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(strings::UI_WELCOME_SUBTITLE),
                )
                .child(
                    Button::new("welcome-create")
                        .label(strings::UI_CREATE_FIRST)
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_create_dialog(window, cx);
                        })),
                );
        }

        let mut tabs = TabBar::new("center-tabs").underline();
        for tab in &self.open_tabs {
            let name = self
                .instance(&tab.instance)
                .map(|view| view.name.clone())
                .unwrap_or_else(|| strings::UI_INSTANCES.to_owned());
            let status = self
                .instance(&tab.instance)
                .map(|view| view.status)
                .unwrap_or(InstanceStatus::Stopped);
            let dot = status_colour(status, &theme);
            tabs = tabs.child(
                Tab::new()
                    .label(name)
                    .prefix(div().mr_1().size(px(8.)).rounded_full().bg(dot)),
            );
        }
        let tabs = tabs.selected_index(self.selected_tab).on_click(cx.listener(
            |this, index: &usize, _window, cx| {
                this.select_tab(*index);
                cx.notify();
            },
        ));

        let content = self.render_selected_tab(cx).into_any_element();
        // F-CAT-01: a stale or offline catalog is a banner, never a modal.
        let banner = self.catalog().banner.clone();
        let banner_colour = match self.catalog().status {
            dblocal_core::model::CatalogStatus::Offline => theme.danger,
            _ => theme.warning,
        };
        let banner_fg = match self.catalog().status {
            dblocal_core::model::CatalogStatus::Offline => theme.danger_foreground,
            _ => theme.warning_foreground,
        };

        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .bg(theme.background)
            .when_some(banner, |this, banner| {
                this.child(
                    div()
                        .id("catalog-banner")
                        .w_full()
                        .px_3()
                        .py_1()
                        .bg(banner_colour)
                        .text_color(banner_fg)
                        .text_xs()
                        .child(banner),
                )
            })
            .child(
                div()
                    .w_full()
                    .bg(theme.tab_bar)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(tabs),
            )
            .child(div().flex_1().min_h_0().overflow_hidden().child(content))
    }

    pub(crate) fn render_selected_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Some(tab) = self.open_tabs.get(self.selected_tab).cloned() else {
            return div().into_any_element();
        };
        let Some(view) = self.instance(&tab.instance).cloned() else {
            return div().into_any_element();
        };
        let inner = TabBar::new("inner-tabs")
            .underline()
            .child(Tab::new().label(strings::UI_TAB_OVERVIEW))
            .child(Tab::new().label(strings::UI_TAB_LOGS))
            .selected_index(match tab.inner {
                InnerTab::Overview => 0,
                InnerTab::Logs => 1,
            })
            .on_click(cx.listener(|this, index: &usize, _window, cx| {
                if let Some(tab) = this.open_tabs.get_mut(this.selected_tab) {
                    tab.inner = if *index == 0 {
                        InnerTab::Overview
                    } else {
                        InnerTab::Logs
                    };
                }
                cx.notify();
            }));

        let body = match tab.inner {
            InnerTab::Overview => self.render_overview(&view, cx).into_any_element(),
            InnerTab::Logs => self.render_logs(cx).into_any_element(),
        };

        v_flex()
            .size_full()
            .child(
                div()
                    .w_full()
                    .bg(theme.tab_bar)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(inner),
            )
            .child(body)
            .into_any_element()
    }

    pub(crate) fn render_overview(
        &self,
        view: &InstanceView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let dot = status_colour(view.status, &theme);
        let label = format::status_label(view.status);
        let id_start = view.id.clone();
        let id_stop = view.id.clone();
        let id_copy = view.id.clone();
        let id_env = view.id.clone();
        let id_delete = view.id.clone();
        let revealed = self.is_revealed(&view.id);
        let id_reveal = view.id.clone();
        let uri = self
            .service
            .connection_uri(&view.id, UriFormat::Uri)
            .map(|secret| {
                if revealed {
                    secret.expose().to_owned()
                } else {
                    format::mask_uri(secret.expose())
                }
            })
            .unwrap_or_default();
        let mut sections = v_flex().id("overview").size_full().p_6().gap_4();

        sections = sections.child(
            h_flex()
                .items_center()
                .gap_3()
                .child(div().size(px(10.)).rounded_full().bg(dot))
                .child(
                    div()
                        .text_lg()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(view.name.clone()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .when(view.auth == AuthMode::None, |this| {
                    this.child(
                        div()
                            .px_2()
                            .py(px(2.))
                            .rounded_full()
                            .bg(theme.warning)
                            .text_color(theme.warning_foreground)
                            .text_xs()
                            .child(strings::BADGE_NO_AUTH),
                    )
                }),
        );

        if let Some(failure) = &view.failure {
            sections = sections.child(render_failure(failure, view, cx));
        }

        sections = sections.child(
            div()
                .p_4()
                .rounded(px(6.))
                .bg(theme.muted)
                .border_1()
                .border_color(theme.border)
                .child(
                    v_flex()
                        .gap_2()
                        .child(info_row(
                            strings::UI_OVERVIEW_VERSION,
                            format!("{} {}", view.engine.display_name(), view.version),
                            theme.muted_foreground,
                        ))
                        .child(info_row(
                            strings::UI_OVERVIEW_PORT,
                            view.port.to_string(),
                            theme.muted_foreground,
                        ))
                        .child(info_row(
                            strings::UI_OVERVIEW_HOST,
                            "127.0.0.1".to_owned(),
                            theme.muted_foreground,
                        ))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_start()
                                .child(info_row(
                                    strings::UI_OVERVIEW_URI,
                                    uri,
                                    theme.muted_foreground,
                                ))
                                .child(
                                    Button::new(SharedString::from(format!("reveal-{}", view.id)))
                                        .label(if revealed {
                                            strings::UI_HIDE_PASSWORD
                                        } else {
                                            strings::UI_SHOW_PASSWORD
                                        })
                                        .xsmall()
                                        .compact()
                                        .ghost()
                                        .on_click(cx.listener(move |this, _, _window, cx| {
                                            this.toggle_reveal(&id_reveal);
                                            cx.notify();
                                        })),
                                ),
                        ),
                ),
        );

        let can_start = matches!(
            view.status,
            InstanceStatus::Stopped | InstanceStatus::Failed
        );
        let can_stop = view.status == InstanceStatus::Running;
        let can_delete = matches!(
            view.status,
            InstanceStatus::Stopped | InstanceStatus::Failed
        );

        sections.child(
            h_flex()
                .gap_2()
                .child(
                    Button::new(SharedString::from(format!("start-{}", view.id)))
                        .label(strings::UI_START)
                        .compact()
                        .primary()
                        .disabled(!can_start)
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.start_instance(id_start.clone());
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("stop-{}", view.id)))
                        .label(strings::UI_STOP)
                        .compact()
                        .secondary()
                        .disabled(!can_stop)
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.stop_instance(id_stop.clone());
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("copy-uri-{}", view.id)))
                        .label(strings::UI_COPY_URI)
                        .compact()
                        .ghost()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.copy_uri(id_copy.clone(), UriFormat::Uri, window, cx);
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("copy-env-{}", view.id)))
                        .label(strings::UI_COPY_DOTENV)
                        .compact()
                        .ghost()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.copy_uri(id_env.clone(), UriFormat::DotEnv, window, cx);
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("delete-{}", view.id)))
                        .label(strings::UI_DELETE)
                        .compact()
                        .danger()
                        .disabled(!can_delete)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_delete_dialog(id_delete.clone(), window, cx);
                        })),
                ),
        )
    }

    pub(crate) fn render_logs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let instance = self
            .open_tabs
            .get(self.selected_tab)
            .map(|tab| tab.instance.clone());
        let buffer = instance.as_ref().and_then(|id| self.logs.get(id));
        let count = buffer.map(LogBuffer::len).unwrap_or(0);
        let following = buffer.is_some_and(LogBuffer::is_following);
        let lines = buffer.cloned().unwrap_or_default();
        let follow_label = if following {
            strings::UI_LOGS_FOLLOWING
        } else {
            strings::UI_LOGS_PAUSED
        };
        let scroll = self.log_scroll.clone();

        let body = if count == 0 {
            div()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(strings::UI_LOGS_EMPTY)
                .into_any_element()
        } else {
            gpui::uniform_list("log-lines", count, move |range, _window, _cx| {
                range
                    .map(|index| {
                        let text = lines
                            .line(index)
                            .cloned()
                            .unwrap_or_else(|| SharedString::from(""));
                        div()
                            .id(("log-line", index))
                            .h(px(18.))
                            .px_3()
                            .font_family(mono.clone())
                            .text_xs()
                            .child(text)
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&scroll)
            .flex_1()
            .h_full()
            .into_any_element()
        };

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(follow_label),
                            )
                            .child(
                                Button::new("toggle-follow")
                                    .label(if following {
                                        strings::UI_LOGS_PAUSED
                                    } else {
                                        strings::UI_LOGS_FOLLOWING
                                    })
                                    .xsmall()
                                    .compact()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.toggle_follow(cx);
                                    })),
                            ),
                    )
                    .child(
                        Button::new("copy-logs")
                            .label(strings::UI_LOGS_COPY)
                            .xsmall()
                            .compact()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.copy_logs(window, cx);
                            })),
                    ),
            )
            .child(body)
    }
    // ----- Accessors used by submodules ----------------------------------
}

fn status_colour(status: InstanceStatus, theme: &gpui_component::Theme) -> Hsla {
    match status {
        InstanceStatus::Running => theme.success,
        InstanceStatus::Failed => theme.danger,
        InstanceStatus::Provisioning
        | InstanceStatus::Initializing
        | InstanceStatus::Starting
        | InstanceStatus::Stopping
        | InstanceStatus::Deleting => theme.warning,
        _ => theme.muted_foreground,
    }
}

fn info_row(label: &str, value: String, muted: Hsla) -> impl IntoElement {
    h_flex()
        .gap_3()
        .items_start()
        .child(
            div()
                .w(px(90.))
                .text_sm()
                .text_color(muted)
                .child(label.to_owned()),
        )
        .child(div().text_sm().flex_1().min_w_0().child(value))
}

fn render_failure(
    failure: &FailureInfo,
    view: &InstanceView,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let id_port = view.id.clone();
    // F-LOG-02 AC1: a known cause becomes an action. Port in use offers the
    // one-click "move to <port>" that `F-PRT-02` AC1 requires.
    let port_action = if failure.diagnosis == dblocal_core::model::StartFailure::PortInUse {
        Some(
            Button::new(SharedString::from(format!("move-port-{}", view.id)))
                .label(format!(
                    "{} {}",
                    strings::UI_CHANGE_PORT,
                    view.port.get() + 1
                ))
                .small()
                .compact()
                .primary()
                .on_click(cx.listener(move |this, _, _window, cx| {
                    let next = this.instance_port_proposal(&id_port);
                    this.move_to_port(&id_port, next, cx);
                })),
        )
    } else {
        None
    };
    let mut column = v_flex().gap_1();
    for line in &failure.log_tail {
        column = column.child(
            div()
                .font_family(theme.mono_font_family.clone())
                .text_xs()
                .child(line.clone()),
        );
    }
    v_flex()
        .id("failure-panel")
        .p_4()
        .gap_2()
        .rounded(px(6.))
        .bg(theme.danger)
        .text_color(theme.danger_foreground)
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(strings::UI_FAILURE_HEADING),
                )
                .child(div().text_xs().child(format!(
                    "{} {}",
                    strings::UI_FAILURE_CODE,
                    failure.code
                ))),
        )
        .child(div().text_sm().child(failure.message.clone()))
        .child(div().text_xs().child(format!(
            "{}: {}",
            strings::UI_FAILURE_DIAGNOSIS,
            crate::format::diagnosis_label(failure.diagnosis)
        )))
        .when_some(port_action, |this, action| {
            this.child(h_flex().gap_2().child(action))
        })
        .child(div().text_xs().child(strings::UI_FAILURE_LOG_TAIL))
        .child(column)
}
