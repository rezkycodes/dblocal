//! Dialog "Instance baru" (F-INS-01, F-CAT-02, F-PRT-01, F-CRD-03).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{WindowExt, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, SharedString, WeakEntity, Window, div, px, svg};

use crate::model::EngineKind;
use crate::theme::tokens;
use crate::ui::components::{engine_badge, section_label};
use crate::ui::workspace::Workspace;

pub struct NewInstanceSpec {
    pub engine: EngineKind,
    pub version: SharedString,
    pub name: SharedString,
    pub port: Option<u16>,
    pub no_auth: bool,
    pub installed: bool,
}

pub struct NewInstanceForm {
    workspace: WeakEntity<Workspace>,
    taken_ports: Vec<(EngineKind, u16)>,
    engine: EngineKind,
    version_ix: usize,
    no_auth: bool,
    name: Entity<InputState>,
    port: Entity<InputState>,
    error: Option<SharedString>,
}

impl NewInstanceForm {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        taken_ports: Vec<(EngineKind, u16)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx));
        let port = cx.new(|cx| InputState::new(window, cx));
        let mut this = Self {
            workspace,
            taken_ports,
            engine: EngineKind::Postgres,
            version_ix: 1,
            no_auth: false,
            name,
            port,
            error: None,
        };
        this.sync_defaults(window, cx);
        this
    }

    fn suggested_port(&self) -> u16 {
        let mut port = self.engine.canonical_port();
        while self.taken_ports.iter().any(|(_, p)| *p == port) {
            port += 1;
        }
        port
    }

    fn sync_defaults(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (version, _, _) = self.engine.catalog()[self.version_ix];
        let major = version.split('.').next().unwrap_or(version);
        let prefix = match self.engine {
            EngineKind::Postgres => "postgres",
            EngineKind::Valkey => "valkey",
            EngineKind::MySql => "mysql",
            EngineKind::MariaDb => "mariadb",
        };
        let name = format!("{prefix}-{major}");
        let port = self.suggested_port().to_string();
        self.name.update(cx, |s, cx| s.set_value(name, window, cx));
        self.port.update(cx, |s, cx| s.set_value(port, window, cx));
        self.error = None;
    }

    fn pick_engine(&mut self, engine: EngineKind, window: &mut Window, cx: &mut Context<Self>) {
        self.engine = engine;
        self.version_ix = 0;
        self.sync_defaults(window, cx);
        cx.notify();
    }

    fn pick_version(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.version_ix = ix;
        self.sync_defaults(window, cx);
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().to_string();
        let valid_name = name.len() >= 2
            && name.len() <= 31
            && name.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !valid_name {
            self.error = Some("Nama: huruf kecil, angka, tanda hubung (2–31 karakter).".into());
            cx.notify();
            return;
        }
        let port: u16 = match self.port.read(cx).value().trim().parse() {
            Ok(p) if p >= 1024 => p,
            Ok(_) => {
                self.error = Some("Port < 1024 butuh privilese; DBLocal tidak pernah meminta sudo.".into());
                cx.notify();
                return;
            }
            Err(_) => {
                self.error = Some("Port harus berupa angka.".into());
                cx.notify();
                return;
            }
        };
        if self.taken_ports.iter().any(|(_, p)| *p == port) {
            self.error = Some(format!("Port {port} sudah dialokasikan instance DBLocal lain.").into());
            cx.notify();
            return;
        }
        let (version, _, installed) = self.engine.catalog()[self.version_ix];
        let spec = NewInstanceSpec {
            engine: self.engine,
            version: version.into(),
            name: name.into(),
            port: Some(port),
            no_auth: self.no_auth,
            installed,
        };
        self.workspace
            .update(cx, |ws, cx| ws.create_instance(spec, cx))
            .ok();
        window.close_dialog(cx);
    }
}

impl Render for NewInstanceForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let engines = EngineKind::ALL.iter().map(|&engine| {
            let selected = engine == self.engine;
            v_flex()
                .id(("engine", engine as usize))
                .flex_1()
                .gap_2()
                .p(px(12.))
                .rounded(px(8.))
                .border_1()
                .cursor_pointer()
                .border_color(if selected { tokens::accent() } else { tokens::border() })
                .bg(if selected { tokens::tint(tokens::accent(), 0.08) } else { tokens::bg_editor().into() })
                .hover(|s| s.border_color(tokens::accent()))
                .on_click(cx.listener(move |this, _, window, cx| this.pick_engine(engine, window, cx)))
                .child(
                    h_flex()
                        .child(engine_badge(engine, 28.))
                        .child(div().flex_1())
                        .when(selected, |s| {
                            s.child(svg().path("icons/check.svg").size(px(16.)).text_color(tokens::accent()))
                        }),
                )
                .child(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(engine.name()))
                .child(
                    div()
                        .text_xs()
                        .text_color(tokens::text_muted())
                        .child(format!("Port kanonik {}", engine.canonical_port())),
                )
        });

        let versions = self.engine.catalog().iter().enumerate().map(|(ix, (version, note, installed))| {
            let selected = ix == self.version_ix;
            h_flex()
                .id(("version", ix))
                .h(px(40.))
                .px(px(14.))
                .gap_3()
                .cursor_pointer()
                .border_t_1()
                .border_color(tokens::border_subtle())
                .bg(if selected { tokens::tint(tokens::accent(), 0.10) } else { tokens::bg_editor().into() })
                .hover(|s| s.bg(tokens::bg_hover()))
                .on_click(cx.listener(move |this, _, window, cx| this.pick_version(ix, window, cx)))
                .child(
                    div()
                        .size(px(16.))
                        .rounded_full()
                        .border_1()
                        .border_color(if selected { tokens::accent() } else { tokens::text_muted() })
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(selected, |s| s.child(div().size(px(8.)).rounded_full().bg(tokens::accent()))),
                )
                .child(div().w(px(90.)).font_weight(gpui_kit::FontWeight::MEDIUM).child(*version))
                .child(div().flex_1().text_color(tokens::text_muted()).child(*note))
                .child(
                    div()
                        .px_2()
                        .rounded_full()
                        .text_xs()
                        .when(*installed, |s| s.bg(tokens::tint(tokens::ok(), 0.14)).text_color(tokens::ok()).child("Terpasang"))
                        .when(!*installed, |s| s.bg(tokens::bg_elevated()).text_color(tokens::text_muted()).child("Unduh")),
                )
        });

        let (_, _, installed) = self.engine.catalog()[self.version_ix];
        let summary = if installed {
            "Runtime sudah terpasang — siap dalam < 10 detik"
        } else {
            "Akan mengunduh runtime lalu verifikasi SHA-256"
        };

        v_flex()
            .gap_5()
            .text_size(px(13.))
            .child(
                div()
                    .text_color(tokens::text_muted())
                    .child("Runtime diunduh dari upstream dan diverifikasi SHA-256 sebelum dipasang."),
            )
            .child(v_flex().gap_2().child(section_label("Engine")).child(h_flex().gap_2p5().children(engines)))
            .child(
                v_flex()
                    .gap_2()
                    .child(section_label(format!("Versi · {}", self.engine.name())))
                    .child(
                        v_flex()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(tokens::border())
                            .overflow_hidden()
                            .children(versions),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1p5()
                            .child(div().font_weight(gpui_kit::FontWeight::MEDIUM).child("Nama instance"))
                            .child(Input::new(&self.name))
                            .child(div().text_xs().text_color(tokens::text_muted()).child("huruf kecil, angka, tanda hubung")),
                    )
                    .child(
                        v_flex()
                            .w(px(160.))
                            .gap_1p5()
                            .child(div().font_weight(gpui_kit::FontWeight::MEDIUM).child("Port"))
                            .child(Input::new(&self.port))
                            .child(div().text_xs().text_color(tokens::text_muted()).child("port bebas berikutnya")),
                    )
                    .child(
                        v_flex()
                            .w(px(140.))
                            .gap_1p5()
                            .child(div().font_weight(gpui_kit::FontWeight::MEDIUM).child("Superuser"))
                            .child(
                                div()
                                    .h(px(32.))
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(tokens::border_subtle())
                                    .text_color(tokens::text_muted())
                                    .child(self.engine.superuser()),
                            )
                            .child(div().text_xs().text_color(tokens::text_muted()).child("password → Keychain")),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .p(px(12.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(if self.no_auth { tokens::tint(tokens::danger(), 0.5) } else { tokens::border().into() })
                    .bg(if self.no_auth { tokens::tint(tokens::danger(), 0.08) } else { tokens::tint(tokens::danger(), 0.) })
                    .child(
                        Checkbox::new("no-auth")
                            .label("Izinkan koneksi tanpa password")
                            .checked(self.no_auth)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.no_auth = *checked;
                                cx.notify();
                            })),
                    )
                    .child(div().pl(px(24.)).text_xs().text_color(tokens::text_muted()).child(
                        "Proses dan user lain di mesin ini bisa terhubung tanpa kredensial. Tidak bisa diubah setelah dibuat.",
                    )),
            )
            .when_some(self.error.clone(), |s, err| {
                s.child(div().text_color(tokens::danger()).child(err))
            })
            .child(
                h_flex()
                    .gap_3()
                    .pt_2()
                    .border_t_1()
                    .border_color(tokens::border())
                    .child(
                        div()
                            .flex_1()
                            .text_color(if installed { tokens::ok() } else { tokens::text_muted() })
                            .child(summary),
                    )
                    .child(
                        Button::new("ni-cancel")
                            .label("Batal")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("ni-create")
                            .primary()
                            .label("Create")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
