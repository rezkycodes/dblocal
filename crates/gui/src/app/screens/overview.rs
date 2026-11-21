//! The Overview tab: connection, detail and process cards (D-25 prototype).
//!
//! The connection string is held masked in [`crate::app::UriView`]; the
//! plaintext is fetched from the service only while the user has it revealed
//! and is dropped again on hide and on tab close.

use dblocal_core::model::{AuthMode, InstanceStatus};
use dblocal_core::strings;
use gpui_kit::component::{ActiveTheme, Sizable, Theme, h_flex, v_flex};
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::assets::IconName;
use gpui_kit::component::label::Label;
use gpui_kit::prelude::*;
use gpui_kit::{Context, IntoElement, ParentElement, Styled, div, px};

use crate::format;

use super::Workspace;
use crate::app::components::{card, icon, kv, mono_themed};
use super::hint_row;
use super::page;

impl Workspace {
    pub(crate) fn render_overview(
        &self,
        view: &dblocal_core::service::InstanceView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        page()
            .child(self.render_connection(view, &theme, cx))
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .flex_wrap()
                    .child(div().flex_1().min_w(px(320.)).child(self.render_detail(view, &theme)))
                    .child(div().flex_1().min_w(px(320.)).child(self.render_process(view, &theme, cx))),
            )
            .child(self.render_recent_logs(view, &theme, cx))
    }

    /// The Connection card: masked URI, reveal, copy actions, the `.env` block.
    fn render_connection(
        &self,
        view: &dblocal_core::service::InstanceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let revealed = self.uri_revealed(&view.id);
        let uri = self.uri_text(&view.id);
        let credential_note = if view.auth == AuthMode::None {
            strings::UI_AUTH_NONE.to_owned()
        } else {
            format::credential_backend_label(view.credential_backend).to_owned()
        };
        let credential_colour = if format::credential_is_file(view.credential_backend) {
            theme.warning
        } else {
            theme.muted_foreground
        };
        let id_reveal = view.id.clone();
        let id_copy = view.id.clone();
        let id_env = view.id.clone();
        let mono_font = theme.mono_font_family.clone();

        let mut env_block = v_flex()
            .gap_0p5()
            .p(px(12.))
            .rounded(px(6.))
            .bg(theme.background)
            .border_1()
            .border_color(theme.table_row_border)
            .font_family(mono_font)
            .text_xs()
            .child(
                Label::new(strings::UI_OVERVIEW_DOTENV)
                    .text_color(theme.muted_foreground),
            );
        for row in self.dotenv_rows(&view.id) {
            env_block = env_block.child(match row.key {
                Some(key) => h_flex()
                    .child(Label::new(key).text_color(theme.primary))
                    .child(Label::new(format!("={}", row.value))),
                None => h_flex().child(Label::new(row.value)),
            });
        }

        card(Some(strings::UI_OVERVIEW_CONNECTION), theme)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Label::new(strings::UI_OVERVIEW_CONNECTION)
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(icon(IconName::KeyRound, 14.0, credential_colour))
                            .child(
                                Label::new(credential_note)
                                    .text_xs()
                                    .text_color(credential_colour),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(320.))
                            .h(px(36.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .font_family(theme.mono_font_family.clone())
                            .overflow_hidden()
                            .child(Label::new(uri).truncate()),
                    )
                    .child(
                        Button::new("reveal-uri")
                            .icon(if revealed {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            })
                            .label(if revealed {
                                strings::UI_HIDE_PASSWORD
                            } else {
                                strings::UI_SHOW_PASSWORD
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_reveal(&id_reveal);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("copy-uri")
                            .primary()
                            .icon(IconName::Copy)
                            .label(strings::UI_COPY_URI)
                            .tooltip(strings::UI_COPY_URI)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.copy_uri(id_copy.clone(), dblocal_core::service::UriFormat::Uri, window, cx);
                            })),
                    )
                    .child(
                        Button::new("copy-env")
                            .icon(IconName::Copy)
                            .label(strings::UI_COPY_DOTENV)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.copy_uri(
                                    id_env.clone(),
                                    dblocal_core::service::UriFormat::DotEnv,
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .child(env_block)
            .when(view.status == InstanceStatus::Stopped, |this| {
                this.child(hint_row(
                    IconName::Info,
                    strings::UI_OVERVIEW_URI_STOPPED_NOTE.to_owned(),
                    theme,
                ))
            })
            .child(hint_row(
                IconName::ShieldCheck,
                strings::UI_OVERVIEW_LOOPBACK_ONLY.to_owned(),
                theme,
            ))
    }

    /// The Detail card: engine, superuser, port, data dir and data size.
    fn render_detail(
        &self,
        view: &dblocal_core::service::InstanceView,
        theme: &Theme,
    ) -> impl IntoElement {
        let arch = std::env::consts::ARCH;
        let size = self
            .data_size(&view.id)
            .map(format::format_bytes_id)
            .unwrap_or_else(|| strings::UI_VALUE_UNKNOWN.to_owned());
        let canonical = format::is_canonical_port(view);
        card(Some(strings::UI_OVERVIEW_DETAIL), theme)
            .child(kv(
                strings::UI_OVERVIEW_ENGINE,
                Label::new(format::engine_detail(view, arch)),
                theme,
            ))
            .child(kv(
                strings::UI_OVERVIEW_SUPERUSER,
                mono_themed(view.engine.default_superuser(), theme),
                theme,
            ))
            .child(kv(
                strings::UI_OVERVIEW_PORT,
                h_flex()
                    .gap_1()
                    .child(Label::new(view.port.to_string()))
                    .when(canonical, |this| {
                        this.child(
                            Label::new(strings::UI_OVERVIEW_CANONICAL_PORT)
                                .text_color(theme.muted_foreground),
                        )
                    }),
                theme,
            ))
            .child(kv(
                strings::UI_OVERVIEW_DATA_DIR,
                Label::new(format::data_dir(&view.name))
                    .font_family(theme.mono_font_family.clone())
                    .truncate(),
                theme,
            ))
            .child(kv(
                strings::UI_OVERVIEW_DATA_SIZE,
                Label::new(size),
                theme,
            ))
    }

    /// The Process card: PID, health, process mode, credential reference and
    /// the autostart checkbox (`set_autostart`).
    fn render_process(
        &self,
        view: &dblocal_core::service::InstanceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pid = self
            .pid_for(view)
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| strings::UI_VALUE_UNKNOWN.to_owned());
        let (health, health_colour) = match view.status {
            InstanceStatus::Running => (strings::UI_OVERVIEW_HEALTH_OK, theme.success),
            InstanceStatus::Failed => (strings::UI_OVERVIEW_HEALTH_FAILED, theme.danger),
            _ => (strings::UI_OVERVIEW_HEALTH_OFFLINE, theme.muted_foreground),
        };
        let id = view.id.clone();
        card(Some(strings::UI_OVERVIEW_PROCESS), theme)
            .child(kv(
                strings::UI_OVERVIEW_PID,
                Label::new(pid).font_family(theme.mono_font_family.clone()),
                theme,
            ))
            .child(kv(
                strings::UI_OVERVIEW_HEALTH,
                Label::new(health).text_color(health_colour),
                theme,
            ))
            .child(kv(
                strings::UI_OVERVIEW_PROCESS_MODE,
                Label::new(strings::UI_OVERVIEW_PROCESS_DETACHED),
                theme,
            ))
            .child(kv(
                strings::UI_OVERVIEW_CREDENTIAL,
                Label::new(format::credential_ref(&view.name))
                    .font_family(theme.mono_font_family.clone()),
                theme,
            ))
            .child(
                div()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme.table_row_border)
                    .child(
                        Checkbox::new("autostart")
                            .label(strings::UI_OVERVIEW_AUTOSTART)
                            .checked(view.autostart)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                this.set_autostart(&id, *checked, cx);
                            })),
                    ),
            )
    }

    /// The last few log lines, with a link into the Logs tab.
    fn render_recent_logs(
        &self,
        view: &dblocal_core::service::InstanceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let buffer = self.log_lines(&view.id);
        let total = buffer.len();
        let start = total.saturating_sub(5);
        let mut lines = v_flex().font_family(theme.mono_font_family.clone()).text_xs();
        for index in start..total {
            if let Some(line) = buffer.line(index) {
                lines = lines.child(
                    Label::new(line.clone()).py_0p5().text_color(theme.foreground),
                );
            }
        }
        if total == 0 {
            lines = lines.child(
                Label::new(strings::UI_LOGS_EMPTY).text_color(theme.muted_foreground),
            );
        }
        card(Some(strings::UI_OVERVIEW_RECENT_LOGS), theme)
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        Label::new(strings::UI_OVERVIEW_RECENT_LOGS)
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                    )
                    .child(
                        Button::new("goto-logs")
                            .link()
                            .small()
                            .compact()
                            .label(strings::UI_OVERVIEW_GOTO_LOGS)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_inner_tab(crate::app::InnerTab::Logs);
                                cx.notify();
                            })),
                    ),
            )
            .child(lines)
    }

    /// The engine PID, when the view carries one. The frozen read model does
    /// not expose a PID, so this is `None` in the product build and the card
    /// prints the prototype's `—`.
    fn pid_for(&self, _view: &dblocal_core::service::InstanceView) -> Option<u32> {
        None
    }
}
