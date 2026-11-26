//! Isi tab per instance: Overview, Logs, Databases, Config.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::Input;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{ActiveTheme, Disableable, IconName, Sizable, WindowExt, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, div, px, svg};

use crate::model::{EngineKind, Failure, LogLevel, Status};
use crate::theme::tokens;
use crate::ui::components::{banner, card, kv, mono};
use crate::ui::workspace::Workspace;

fn page() -> gpui_kit::Div {
    v_flex()
        .gap_4()
        .px(px(32.))
        .pt(px(22.))
        .pb(px(32.))
        .max_w(px(1080.))
}

impl Workspace {
    // ------------------------------------------------------------ Overview
    pub(crate) fn render_overview(&self, cx: &mut Context<Self>) -> AnyElement {
        let inst = self.current();
        let mono_font = cx.theme().mono_font_family.clone();

        if inst.status == Status::Provisioning {
            return page().child(self.render_provisioning(cx)).into_any_element();
        }

        let uri = inst.uri(self.reveal);
        let env = inst.env_block(self.reveal);

        let connection = card(None)
            .child(
                h_flex()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("Connection"),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .text_xs()
                            .text_color(tokens::text_muted())
                            .child(svg().path("icons/key-round.svg").size(px(14.)).text_color(tokens::text_muted()))
                            .child(if inst.no_auth {
                                "Tanpa password (No auth)"
                            } else {
                                "Password di Keychain"
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(360.))
                            .h(px(36.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(tokens::border())
                            .bg(tokens::bg_editor())
                            .font_family(mono_font.clone())
                            .overflow_hidden()
                            .child(div().truncate().child(uri)),
                    )
                    .child(
                        Button::new("reveal")
                            .icon(if self.reveal { IconName::EyeOff } else { IconName::Eye })
                            .label(if self.reveal { "Sembunyikan" } else { "Tampilkan" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reveal = !this.reveal;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("copy-uri")
                            .primary()
                            .icon(IconName::Copy)
                            .label("Copy URI")
                            .tooltip("Ctrl+Shift+C")
                            .on_click(cx.listener(|this, _, window, cx| this.copy_uri(window, cx))),
                    )
                    .child(
                        Button::new("copy-env")
                            .icon(IconName::Copy)
                            .label("Copy .env")
                            .on_click(cx.listener(|this, _, window, cx| this.copy_env(window, cx))),
                    ),
            )
            .child(
                div()
                    .p(px(12.))
                    .rounded(px(6.))
                    .bg(tokens::bg_editor())
                    .border_1()
                    .border_color(tokens::border_subtle())
                    .font_family(mono_font.clone())
                    .text_xs()
                    .child(div().text_color(tokens::text_muted()).child("# .env"))
                    .children(env.lines().map(|line| {
                        let (key, value) = line.split_once('=').unwrap_or((line, ""));
                        h_flex()
                            .child(div().text_color(tokens::accent()).child(key.to_string()))
                            .child("=")
                            .child(value.to_string())
                    })),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .text_color(tokens::text_muted())
                    .child(svg().path("icons/shield-check.svg").size(px(14.)).text_color(tokens::text_muted()))
                    .child("Hanya mendengarkan 127.0.0.1 dan ::1"),
            );

        let idx = self.selected;
        let details = card(Some("Details"))
            .child(kv("Engine", format!("{} {} · {}", inst.engine.name(), inst.version, std::env::consts::ARCH)))
            .child(kv("Superuser", mono(inst.engine.superuser(), cx)))
            .child(kv(
                "Port",
                h_flex()
                    .gap_1()
                    .child(mono(inst.port.to_string(), cx))
                    .when(inst.port == inst.engine.canonical_port(), |s| {
                        s.child(div().text_color(tokens::text_muted()).child("(canonical)"))
                    }),
            ))
            .child(kv("Data dir", mono(inst.data_dir(), cx).truncate()))
            .child(kv("Data size", inst.data_size.clone()));

        let process = card(Some("Process"))
            .child(kv(
                "PID",
                mono(inst.pid.map(|p| p.to_string()).unwrap_or_else(|| "—".into()), cx),
            ))
            .child(kv(
                "Health check",
                div()
                    .text_color(if inst.status == Status::Running { tokens::ok() } else { tokens::text_muted() })
                    .child(match inst.status {
                        Status::Running => "OK · protocol handshake",
                        Status::Failed => "Failed — see Logs",
                        _ => "Not running",
                    }),
            ))
            .child(kv("Process mode", "Detached · keeps running when the app quits"))
            .child(kv("Credential", mono(format!("dblocal:{}", inst.name), cx)))
            .child(
                div().pt_2().border_t_1().border_color(tokens::border_subtle()).child(
                    Checkbox::new("autostart")
                        .label("Run automatically at login")
                        .checked(inst.autostart)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.instances[idx].autostart = *checked;
                            cx.notify();
                        })),
                ),
            );

        let recent = card(None)
            .child(
                h_flex()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("Recent logs"),
                    )
                    .child(
                        Button::new("goto-logs")
                            .link()
                            .small()
                            .label("Open the Logs tab")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.section = crate::ui::workspace::Section::Logs;
                                cx.notify();
                            })),
                    ),
            )
            .child(self.render_log_lines(5, cx));

        page()
            .child(connection)
            .child(h_flex().gap_4().items_start().flex_wrap()
                .child(div().flex_1().min_w(px(320.)).child(details))
                .child(div().flex_1().min_w(px(320.)).child(process)))
            .child(recent)
            .into_any_element()
    }

    fn render_provisioning(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let inst = self.current();
        let pct = inst.progress;
        let total_mb = 34.0_f32;
        let steps = [
            ("Downloading the runtime", pct < 100.),
            ("Verifikasi SHA-256", false),
            ("Ekstrak ke ~/.dblocal/runtimes", false),
            ("Inisialisasi data (initdb)", false),
        ];
        card(None)
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("Menyiapkan runtime"))
                    .child(mono(
                        format!(
                            "{}-{}-{}.tar.gz",
                            inst.engine.name().to_lowercase(),
                            inst.version,
                            std::env::consts::ARCH
                        ),
                        cx,
                    )
                    .text_xs()
                    .text_color(tokens::text_muted())),
            )
            .child(Progress::new("dl").value(pct))
            .child(
                h_flex()
                    .gap_4()
                    .child(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(format!("{pct:.0}%")))
                    .child(mono(format!("{:.1} MB / {total_mb:.1} MB", total_mb * pct / 100.), cx))
                    .child(div().text_color(tokens::text_muted()).child("2,4 MB/dtk"))
                    .child(div().flex_1())
                    .child(Button::new("pause").label("Jeda").small())
                    .child(Button::new("cancel-dl").label("Batal").small()),
            )
            .child(v_flex().gap_2().children(steps.iter().enumerate().map(|(i, (label, active))| {
                h_flex()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.))
                            .rounded_full()
                            .text_xs()
                            .when(*active, |s| s.bg(tokens::accent()).text_color(tokens::on_accent()))
                            .when(!*active, |s| s.border_1().border_color(tokens::border()).text_color(tokens::text_muted()))
                            .child((i + 1).to_string()),
                    )
                    .child(div().when(!*active, |s| s.text_color(tokens::text_muted())).child(*label))
            })))
    }

    pub(crate) fn render_log_lines(&self, last: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let inst = self.current();
        let start = inst.logs.len().saturating_sub(last);
        let mono_font = cx.theme().mono_font_family.clone();
        v_flex()
            .font_family(mono_font)
            .text_xs()
            .children(inst.logs[start..].iter().enumerate().map(|(i, line)| {
                let (tag, color) = match line.level {
                    LogLevel::Log => ("LOG", tokens::info()),
                    LogLevel::Dbl => ("DBL", tokens::ok()),
                    LogLevel::Error => ("ERROR", tokens::danger()),
                    LogLevel::Separator => ("", tokens::text_muted()),
                };
                if line.level == LogLevel::Separator {
                    return h_flex()
                        .py_0p5()
                        .text_color(tokens::text_muted())
                        .child(format!("── {} ──────────────", line.text));
                }
                h_flex()
                    .py_0p5()
                    .px_1()
                    .gap_3()
                    .rounded(px(3.))
                    .when(line.level == LogLevel::Error, |s| s.bg(tokens::tint(tokens::danger(), 0.08)))
                    .child(div().w(px(36.)).text_right().text_color(tokens::text_muted()).child((start + i + 1).to_string()))
                    .child(div().text_color(tokens::text_muted()).child(line.time.clone()))
                    .child(div().w(px(46.)).text_color(color).child(tag))
                    .child(div().flex_1().min_w_0().child(line.text.clone()))
            }))
    }

    // ---------------------------------------------------------------- Logs
    pub(crate) fn render_logs(&self, cx: &mut Context<Self>) -> AnyElement {
        let inst = self.current();
        let diagnosis = match &inst.failure {
            Some(Failure::PortInUse { port, suggested }) => Some(
                banner(tokens::danger())
                    .child(svg().path("icons/triangle-alert.svg").flex_none().size(px(20.)).text_color(tokens::danger()))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(280.))
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(div().text_sm().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(format!("Port {port} is already in use")))
                                    .child(mono("DBL-1302", cx).text_xs().text_color(tokens::text_muted())),
                            )
                            .child(div().text_color(tokens::text_muted()).child(format!(
                                "Binding 127.0.0.1:{port} failed before the engine was started. A process outside DBLocal holds the port. The nearest free port is {suggested}."
                            ))),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_2()
                            .child(
                                Button::new("move-port")
                                    .primary()
                                    .label(format!("Move to {suggested} and start"))
                                    .on_click(cx.listener(|this, _, window, cx| this.move_port_and_start(window, cx))),
                            )
                            .child(
                                Button::new("report")
                                    .label("Report an issue")
                                    .on_click(cx.listener(|_, _, window, cx| {
                                        window.push_notification(
                                            Notification::info("Template issue disiapkan tanpa password & path home"),
                                            cx,
                                        )
                                    })),
                            ),
                    ),
            ),
            None => None,
        };

        let viewer = v_flex()
            .rounded(px(8.))
            .border_1()
            .border_color(tokens::border())
            .bg(tokens::bg_panel())
            .child(
                h_flex()
                    .gap_2()
                    .p(px(8.))
                    .border_b_1()
                    .border_color(tokens::border_subtle())
                    .child(div().text_sm().font_weight(gpui_kit::FontWeight::SEMIBOLD).px_1().child("engine.log"))
                    .child(div().flex_1())
                    .child(
                        Button::new("copy-logs")
                            .small()
                            .icon(IconName::Copy)
                            .label("Copy 500 lines")
                            .on_click(cx.listener(|this, _, window, cx| this.copy_logs(window, cx))),
                    )
                    .child(Button::new("open-folder").small().icon(IconName::FolderOpen).label("Open folder")),
            )
            .child(div().p(px(8.)).child(self.render_log_lines(1000, cx)))
            .child(
                h_flex()
                    .gap_4()
                    .px(px(14.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(tokens::border_subtle())
                    .text_xs()
                    .text_color(tokens::text_muted())
                    .child(mono(format!("~/.dblocal/instances/{}/logs/engine.log", inst.name), cx))
                    .child("rotasi maks 3 × 10 MB")
                    .child(div().flex_1())
                    .child(format!("{} baris", inst.logs.len())),
            );

        page().children(diagnosis).child(viewer).into_any_element()
    }

    // ----------------------------------------------------------- Databases
    pub(crate) fn render_databases(&self, cx: &mut Context<Self>) -> AnyElement {
        let inst = self.current();
        if inst.engine == EngineKind::Valkey {
            return page()
                .child(card(None).child(div().text_color(tokens::text_muted()).child(
                    "Valkey has no named databases or SQL users. Use indexes 0–15 directly from the URI.",
                )))
                .into_any_element();
        }
        if inst.status != Status::Running {
            return page()
                .child(card(None).child(div().text_color(tokens::text_muted()).child(
                    "Start the instance to view and manage databases.",
                )))
                .into_any_element();
        }

        let mono_font = cx.theme().mono_font_family.clone();
        let header = h_flex()
            .gap_2()
            .child(div().flex_1().text_sm().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(format!("Database · {}", inst.databases.len())))
            .child(div().w(px(200.)).child(Input::new(&self.new_db_name).small()))
            .child(
                Button::new("create-db")
                    .primary()
                    .small()
                    .icon(IconName::Plus)
                    .label("Create database")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let name = this.new_db_name.read(cx).value().to_string();
                        let valid = !name.is_empty()
                            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                        if !valid {
                            window.push_notification(
                                Notification::error("The name may contain only letters, digits and underscores"),
                                cx,
                            );
                            return;
                        }
                        let idx = this.selected;
                        this.instances[idx].databases.push(crate::model::DbRow {
                            name: name.clone().into(),
                            size: "7,3 MB".into(),
                        });
                        this.new_db_name.update(cx, |s, cx| s.set_value("", window, cx));
                        window.push_notification(Notification::success(format!("Database {name} created (UTF-8)")), cx);
                        cx.notify();
                    })),
            );

        let rows = inst.databases.iter().enumerate().map(|(i, db)| {
            h_flex()
                .h(px(42.))
                .px(px(14.))
                .border_t_1()
                .border_color(tokens::border_subtle())
                .child(div().flex_1().font_family(mono_font.clone()).child(db.name.clone()))
                .child(div().w(px(140.)).text_color(tokens::text_muted()).child("UTF8 · C.UTF-8"))
                .child(div().w(px(100.)).text_right().child(db.size.clone()))
                .child(
                    h_flex().w(px(80.)).justify_end().child(
                        Button::new(("del-db", i))
                            .ghost()
                            .small()
                            .icon(IconName::Delete)
                            .tooltip("Delete database (type the name to confirm)")
                            .disabled(db.name.as_ref() == "postgres"),
                    ),
                )
        });

        let table = v_flex()
            .rounded(px(8.))
            .border_1()
            .border_color(tokens::border())
            .bg(tokens::bg_panel())
            .child(div().p(px(14.)).child(header))
            .child(
                h_flex()
                    .h(px(30.))
                    .px(px(14.))
                    .text_xs()
                    .text_color(tokens::text_muted())
                    .border_t_1()
                    .border_color(tokens::border())
                    .child(div().flex_1().child("Name"))
                    .child(div().w(px(140.)).child("Encoding"))
                    .child(div().w(px(100.)).text_right().child("Size"))
                    .child(div().w(px(80.))),
            )
            .children(rows);

        let users = card(Some("Users and privileges"))
            .children(
                [
                    (inst.engine.superuser(), "all", "superuser", tokens::purple()),
                    ("app_rw", "app_dev", "read-write", tokens::accent()),
                    ("report_ro", "app_dev", "read-only", tokens::text_muted()),
                ]
                .into_iter()
                .map(|(user, db, role, color)| {
                    h_flex()
                        .gap_3()
                        .h(px(34.))
                        .child(div().flex_1().font_family(mono_font.clone()).child(user))
                        .child(div().w(px(100.)).text_color(tokens::text_muted()).child(db))
                        .child(
                            div()
                                .px_2()
                                .rounded_full()
                                .bg(tokens::tint(color, 0.14))
                                .text_color(color)
                                .text_xs()
                                .child(role),
                        )
                }),
            )
            .child(h_flex().child(Button::new("new-user").small().icon(IconName::Plus).label("New user")));

        let reset = card(Some("Superuser password"))
            .child(div().text_color(tokens::text_muted()).child(
                "Generates a new password and updates the URI. If it fails, the old password stays in effect.",
            ))
            .child(h_flex().child(
                Button::new("reset-pass")
                    .label("Reset password")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let idx = this.selected;
                        this.instances[idx].password = crate::model::demo_password(idx + 97);
                        window.push_notification(Notification::success("New password saved in the keychain"), cx);
                        cx.notify();
                    })),
            ));

        page()
            .child(table)
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .flex_wrap()
                    .child(div().flex_1().min_w(px(360.)).child(users))
                    .child(div().flex_1().min_w(px(300.)).child(reset)),
            )
            .child(
                banner(tokens::text_muted())
                    .items_center()
                    .child(div().flex_1().text_color(tokens::text_muted()).child(
                        "DBLocal manages the server, not the data. To browse tables, open the instance in your client.",
                    ))
                    .child(Button::new("open-client").icon(IconName::ExternalLink).label("Open in…")),
            )
            .into_any_element()
    }

    // -------------------------------------------------------------- Config
    pub(crate) fn render_config(&self, cx: &mut Context<Self>) -> AnyElement {
        let inst = self.current();
        let mono_font = cx.theme().mono_font_family.clone();
        let params: Vec<(&str, String, &str, &str)> = match inst.engine {
            EngineKind::Postgres => vec![
                ("port", inst.port.to_string(), "5432", "Diuji bind sebelum disimpan"),
                ("max_connections", "200".into(), "100", "Butuh restart"),
                ("shared_buffers", "512MB".into(), "128MB", "Butuh restart"),
                ("work_mem", "4MB".into(), "4MB", "Berlaku setelah reload"),
                ("timezone", "Asia/Makassar".into(), "Etc/UTC", "Default session time zone"),
                ("password_encryption", "scram-sha-256".into(), "scram-sha-256", "Hanya 127.0.0.1 dan ::1"),
            ],
            EngineKind::Valkey => vec![
                ("port", inst.port.to_string(), "6379", "Diuji bind sebelum disimpan"),
                ("maxmemory", "256mb".into(), "0", "0 = tanpa batas"),
                ("appendonly", "no".into(), "no", "Persistensi AOF"),
                ("protected-mode", "yes".into(), "yes", "Selalu aktif"),
            ],
            EngineKind::MySql | EngineKind::MariaDb => vec![
                ("port", inst.port.to_string(), "3306", "Diuji bind sebelum disimpan"),
                ("max_connections", "151".into(), "151", "Butuh restart"),
                ("innodb_buffer_pool_size", "256M".into(), "128M", "Butuh restart"),
                ("character_set_server", "utf8mb4".into(), "utf8mb4", "Collation utf8mb4_unicode_ci"),
            ],
        };

        let rows = params.into_iter().map(|(name, value, default, hint)| {
            let changed = value != default && name != "port";
            h_flex()
                .gap_4()
                .px(px(18.))
                .py(px(10.))
                .border_t_1()
                .border_color(tokens::border_subtle())
                .when(changed, |s| s.bg(tokens::tint(tokens::accent(), 0.05)))
                .child(
                    v_flex()
                        .flex_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().font_family(mono_font.clone()).child(name))
                                .when(changed, |s| {
                                    s.child(
                                        div()
                                            .px_1()
                                            .rounded(px(4.))
                                            .border_1()
                                            .border_color(tokens::tint(tokens::accent(), 0.5))
                                            .text_color(tokens::accent())
                                            .text_size(px(11.))
                                            .child("diubah"),
                                    )
                                }),
                        )
                        .child(div().text_xs().text_color(tokens::text_muted()).child(hint)),
                )
                .child(
                    div()
                        .w(px(220.))
                        .h(px(32.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(if changed { tokens::accent() } else { tokens::border() })
                        .bg(tokens::bg_editor())
                        .font_family(mono_font.clone())
                        .child(value),
                )
                .child(div().w(px(120.)).font_family(mono_font.clone()).text_color(tokens::text_muted()).child(default))
        });

        page()
            .child(
                banner(tokens::warn())
                    .items_center()
                    .child(svg().path("icons/refresh-cw.svg").flex_none().size(px(18.)).text_color(tokens::warn()))
                    .child(div().flex_1().child("Saved. A restart is required for the changes to take effect."))
                    .child(Button::new("later").label("Nanti"))
                    .child(
                        Button::new("restart-now")
                            .primary()
                            .label("Restart sekarang")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let idx = this.selected;
                                if this.instances[idx].status == Status::Running {
                                    this.toggle_run(idx, window, cx);
                                }
                            })),
                    ),
            )
            .child(
                v_flex()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(tokens::border())
                    .bg(tokens::bg_panel())
                    .child(
                        h_flex()
                            .px(px(18.))
                            .py(px(12.))
                            .child(div().flex_1().text_sm().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("Setting penting"))
                            .child(div().text_xs().text_color(tokens::text_muted()).child("Current value · Default")),
                    )
                    .children(rows)
                    .child(
                        h_flex()
                            .gap_2()
                            .px(px(18.))
                            .py(px(12.))
                            .border_t_1()
                            .border_color(tokens::border())
                            .child(Button::new("open-conf").icon(IconName::ExternalLink).label("Open config file"))
                            .child(Button::new("revert").label("Restore the previous configuration"))
                            .child(div().flex_1())
                            .child(Button::new("discard").label("Buang perubahan"))
                            .child(Button::new("save").primary().label("Save")),
                    ),
            )
            .into_any_element()
    }
}
