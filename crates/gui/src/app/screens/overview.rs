//! The Overview tab: connection, detail and process cards (D-25 prototype).
//!
//! The connection string is held masked in [`crate::app::UriView`]; the
//! plaintext is fetched from the service only while the user has it revealed
//! and is dropped again on hide and on tab close.

use dblocal_core::model::{AuthMode, InstanceStatus};
use dblocal_core::strings;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, Sizable, Theme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{Context, IntoElement, ParentElement, SharedString, Styled, div, px};

use crate::format;

use super::Workspace;
use super::hint_row;
use super::page;
use crate::app::components::{card, icon, kv, mono_themed};

impl Workspace {
    pub(crate) fn render_overview(
        &self,
        view: &dblocal_core::service::InstanceView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut content = page();
        if crate::format::offers_port_fix(view) {
            content = content.child(self.render_overview_diagnosis(view, &theme, cx));
        }
        // F-CFG-01 AC3: a failure whose config file changed offers the restore
        // from where the failure is shown, not only from the Config tab.
        if crate::format::offers_restore(view) {
            content = content.child(self.render_restore_banner(view, &theme, cx));
        }
        content
            .child(self.render_connection(view, &theme, cx))
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .flex_wrap()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(320.))
                            .child(self.render_detail(view, &theme)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(320.))
                            .child(self.render_process(view, &theme, cx)),
                    ),
            )
            .child(self.render_recent_logs(view, &theme, cx))
    }

    fn render_overview_diagnosis(
        &self,
        view: &dblocal_core::service::InstanceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let suggested = self.instance_port_proposal(&view.id);
        let id_fix = view.id.clone();
        let code = view
            .failure
            .as_ref()
            .map(|f| f.code.clone())
            .unwrap_or_default();
        super::super::components::banner(theme.danger)
            .child(super::super::components::icon(
                gpui_kit::assets::IconName::TriangleAlert,
                20.0,
                theme.danger,
            ))
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
                                    .font_family(theme.mono_font_family.clone())
                                    .text_color(theme.muted_foreground),
                            ),
                    )
                    .child(
                        Label::new(strings::diagnosis_port_body(view.port.get(), suggested))
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    ),
            )
            .child(
                h_flex().flex_none().gap_2().child(
                    Button::new("overview-move-port")
                        .primary()
                        .label(strings::diagnosis_move_and_start(suggested))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.move_to_port_and_start(&id_fix, suggested, cx);
                        })),
                ),
            )
    }

    /// The failure banner that offers "Restore the configuration and restart"
    /// (`F-CFG-01` AC3): shown where the failure is, not only in the Config tab.
    fn render_restore_banner(
        &self,
        view: &dblocal_core::service::InstanceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id_restore = view.id.clone();
        let code = view
            .failure
            .as_ref()
            .map(|f| f.code.clone())
            .unwrap_or_default();
        super::super::components::banner(theme.danger)
            .child(super::super::components::icon(
                gpui_kit::assets::IconName::TriangleAlert,
                20.0,
                theme.danger,
            ))
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
                                Label::new(strings::UI_CONFIG_RESTORE_TITLE)
                                    .text_sm()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                            )
                            .child(
                                Label::new(code)
                                    .text_xs()
                                    .font_family(theme.mono_font_family.clone())
                                    .text_color(theme.muted_foreground),
                            ),
                    )
                    .child(
                        Label::new(strings::UI_CONFIG_RESTORE_BANNER_BODY)
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    ),
            )
            .child(
                h_flex().flex_none().gap_2().child(
                    Button::new("overview-restore-config")
                        .debug_selector(|| "overview-restore-config".to_owned())
                        .primary()
                        .label(strings::UI_CONFIG_RESTORE_AND_RESTART)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.restore_and_restart(&id_restore, cx);
                        })),
                ),
            )
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
            .child(Label::new(strings::UI_OVERVIEW_DOTENV).text_color(theme.muted_foreground));
        for (index, row) in self.dotenv_rows(&view.id).into_iter().enumerate() {
            env_block = env_block.child(match row.key {
                Some(key) => h_flex()
                    .id(SharedString::from(format!("env-row-{index}")))
                    .child(
                        Label::new(key)
                            .id(SharedString::from(format!("env-key-{index}")))
                            .text_color(theme.primary),
                    )
                    .child(
                        Label::new(format!("={}", row.value))
                            .id(SharedString::from(format!("env-value-{index}"))),
                    ),
                None => h_flex()
                    .id(SharedString::from(format!("env-line-{index}")))
                    .child(
                        Label::new(row.value)
                            .id(SharedString::from(format!("env-line-text-{index}"))),
                    ),
            });
        }

        let header = format::connection_header(&credential_note);
        card(header.card_title, Some("connection"), theme)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Label::new(header.title)
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(icon(IconName::KeyRound, 14.0, credential_colour))
                            .child(
                                Label::new(header.credential)
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
                                this.toggle_reveal(&id_reveal, cx);
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
                                this.copy_uri(
                                    id_copy.clone(),
                                    dblocal_core::service::UriFormat::Uri,
                                    window,
                                    cx,
                                );
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
                    )
                    // The second "Open in…" entry point the task names, beside
                    // the header's one (`F-OPN-01..02`); its slot keeps the ids
                    // distinct from the header menu's. RustFS is an S3 server
                    // and has no SQL/Redis client here.
                    .when(
                        view.engine != dblocal_core::model::EngineKey::Rustfs,
                        |this| this.child(self.render_open_in(view, "connection", theme, cx)),
                    ),
            )
            .when_some(
                (view.engine == dblocal_core::model::EngineKey::Rustfs)
                    .then_some(view.extra_port)
                    .flatten(),
                |this, console_port| {
                    let cp = console_port.get();
                    let console_url = format!("http://127.0.0.1:{cp}");
                    this.child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(280.))
                                    .h(px(36.))
                                    .px(px(12.))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.background)
                                    .child(
                                        Label::new("Console")
                                            .text_xs()
                                            .text_color(theme.muted_foreground),
                                    )
                                    .child(
                                        Label::new(console_url)
                                            .font_family(theme.mono_font_family.clone())
                                            .truncate(),
                                    ),
                            )
                            .child(
                                Button::new("open-rustfs-console")
                                    .icon(IconName::ExternalLink)
                                    .label("Open console")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_console_url_action(cp, cx);
                                    })),
                            ),
                    )
                },
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
            .map(format::format_bytes)
            .unwrap_or_else(|| strings::UI_VALUE_UNKNOWN.to_owned());
        let canonical = format::is_canonical_port(view);
        card(Some(strings::UI_OVERVIEW_DETAIL), None, theme)
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
                Label::new(format::data_dir(
                    self.home_root(),
                    self.home_dir(),
                    &view.name,
                ))
                .font_family(theme.mono_font_family.clone())
                .truncate(),
                theme,
            ))
            .child(kv(strings::UI_OVERVIEW_DATA_SIZE, Label::new(size), theme))
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
        card(Some(strings::UI_OVERVIEW_PROCESS), None, theme)
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
        let mut lines = v_flex()
            .font_family(theme.mono_font_family.clone())
            .text_xs();
        for index in start..total {
            if let Some(line) = buffer.line(index) {
                lines = lines.child(
                    Label::new(line.clone())
                        .id(SharedString::from(format!("recent-log-{index}")))
                        .py_0p5()
                        .text_color(theme.foreground),
                );
            }
        }
        if total == 0 {
            lines =
                lines.child(Label::new(strings::UI_LOGS_EMPTY).text_color(theme.muted_foreground));
        }
        // The heading row carries the card title and the link, so the card
        // itself is built without a title of its own.
        card(None, Some(strings::UI_OVERVIEW_RECENT_LOGS), theme)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Label::new(strings::UI_OVERVIEW_RECENT_LOGS)
                            .id(SharedString::from(format!(
                                "card-title-{}",
                                strings::UI_OVERVIEW_RECENT_LOGS
                            )))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                    )
                    .child(
                        Button::new("goto-logs")
                            .link()
                            .small()
                            .compact()
                            .label(strings::UI_OVERVIEW_GOTO_LOGS)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_inner_tab(crate::app::InnerTab::Logs, cx);
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
