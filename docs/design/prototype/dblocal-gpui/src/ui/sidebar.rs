//! Dock kiri: pohon instance per engine, daftar runtime, tombol Instance baru.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{Context, div, px, svg};

use crate::model::EngineKind;
use crate::theme::tokens;
use crate::ui::components::{engine_badge, section_label, status_dot};
use crate::ui::workspace::Workspace;

impl Workspace {
    pub(crate) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.filter.read(cx).value().to_lowercase();
        let mono_font = cx.theme().mono_font_family.clone();

        let groups = EngineKind::ALL.iter().filter_map(|&engine| {
            let members: Vec<usize> = self
                .instances
                .iter()
                .enumerate()
                .filter(|(_, i)| i.engine == engine)
                .filter(|(_, i)| query.is_empty() || i.name.to_lowercase().contains(&query))
                .map(|(ix, _)| ix)
                .collect();
            if members.is_empty() {
                return None;
            }
            let collapsed = self.collapsed.contains(&engine);
            let header = h_flex()
                .id(("group", engine as usize))
                .h(px(30.))
                .px(px(6.))
                .gap_2()
                .rounded(px(5.))
                .cursor_pointer()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .hover(|s| s.bg(tokens::bg_elevated()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.collapsed.remove(&engine) {
                        this.collapsed.insert(engine);
                    }
                    cx.notify();
                }))
                .child(
                    svg()
                        .path(if collapsed {
                            "icons/chevron-right.svg"
                        } else {
                            "icons/chevron-down.svg"
                        })
                        .size(px(14.))
                        .text_color(tokens::text_muted()),
                )
                .child(engine_badge(engine, 20.))
                .child(div().flex_1().child(engine.name()))
                .child(
                    div()
                        .text_xs()
                        .text_color(tokens::text_muted())
                        .child(members.len().to_string()),
                );

            let rows = members.into_iter().map(|ix| {
                let inst = &self.instances[ix];
                let selected = ix == self.selected;
                let port_color = if inst.status == crate::model::Status::Failed {
                    tokens::danger()
                } else {
                    tokens::text_muted()
                };
                h_flex()
                    .id(("inst", ix))
                    .h(px(30.))
                    .pl(px(36.))
                    .pr(px(10.))
                    .gap_2()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .when(selected, |s| s.bg(tokens::bg_selected()).text_color(tokens::text_bright()))
                    .when(!selected, |s| s.hover(|s| s.bg(tokens::bg_elevated())))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(ix, cx)))
                    .child(status_dot(inst.status, 9.))
                    .child(div().flex_1().min_w_0().truncate().child(inst.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(tokens::text_muted())
                            .child(inst.version.split('.').take(2).collect::<Vec<_>>().join(".")),
                    )
                    .child(
                        div()
                            .w(px(46.))
                            .text_right()
                            .text_xs()
                            .font_family(mono_font.clone())
                            .text_color(port_color)
                            .child(format!(":{}", inst.port)),
                    )
            });

            Some(
                v_flex()
                    .gap_0p5()
                    .child(header)
                    .when(!collapsed, |s| s.children(rows)),
            )
        });

        let runtimes = [
            ("PostgreSQL 18.7.0", "1 inst.", false),
            ("PostgreSQL 14.20", "1 inst.", false),
            ("Valkey 9.0.1", "1 inst.", false),
            ("PostgreSQL 16.11", "tak terpakai", true),
        ];

        v_flex()
            .w(px(272.))
            .flex_none()
            .h_full()
            .bg(tokens::bg_panel())
            .border_r_1()
            .border_color(tokens::border_strong())
            .p(px(8.))
            .gap_2p5()
            .child(
                h_flex()
                    .pl(px(8.))
                    .child(div().flex_1().child(section_label("Instances")))
                    .child(
                        Button::new("sidebar-new")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip("Instance baru")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_new_instance(window, cx)
                            })),
                    ),
            )
            .child(
                Input::new(&self.filter)
                    .small()
                    .prefix(
                        svg()
                            .path("icons/search.svg")
                            .size(px(14.))
                            .text_color(tokens::text_muted()),
                    )
                    .cleanable(true),
            )
            .child(
                v_flex()
                    .id("instance-tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_1()
                    .children(groups),
            )
            .child(div().h(px(1.)).bg(tokens::border_strong()))
            .child(
                v_flex()
                    .gap_0p5()
                    .child(div().px(px(8.)).pb_1().child(section_label("Runtimes")))
                    .children(runtimes.iter().map(|(name, usage, unused)| {
                        h_flex()
                            .h(px(26.))
                            .px(px(10.))
                            .gap_2()
                            .child(
                                svg()
                                    .path("icons/package.svg")
                                    .size(px(14.))
                                    .text_color(tokens::text_muted()),
                            )
                            .child(div().flex_1().child(*name))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if *unused { tokens::warn() } else { tokens::text_muted() })
                                    .child(*usage),
                            )
                    })),
            )
            .child(
                Button::new("new-instance")
                    .primary()
                    .w_full()
                    .icon(IconName::Plus)
                    .label("Instance baru")
                    .on_click(cx.listener(|this, _, window, cx| this.open_new_instance(window, cx))),
            )
    }
}
