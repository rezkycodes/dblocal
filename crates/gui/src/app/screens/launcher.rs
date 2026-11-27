//! The launcher: what the centre pane shows when no tab is open.
//!
//! Modelled on DBDelve's "Connect a database" card. One centred column: the app
//! mark, a heading, then a grouped card with three sections:
//!
//! 1. **Quick create**: one tile per engine the catalog offers on this
//!    platform. A click opens the New instance dialog with that engine chosen.
//! 2. **Open an instance** (only when instances exist): the first few
//!    instances with their status, version and port; a click opens the tab.
//! 3. **Actions**: New instance, Command palette and Settings, each with its
//!    shortcut drawn as key caps.
//!
//! The welcome heading (no instances yet) and the select-instance heading
//! (instances exist, no tab open) share this layout, so the empty window never
//! reads as a blank page.

use dblocal_core::model::EngineKey;
use dblocal_core::strings;
use gpui_kit::assets::IconName;
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, Theme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{Context, IntoElement, ParentElement, SharedString, Styled, div, px};

use crate::format;
use crate::theme::tokens;

use super::Workspace;
use crate::app::actions::modifier_label;
use crate::app::components::{engine_badge, engine_icon, icon, status_glyph};
use crate::app::design;

/// Width of the launcher card.
const LAUNCHER_W: f32 = 560.0;
/// How many instances the launcher lists before deferring to the dock.
const LAUNCHER_INSTANCES: usize = 4;

/// One launcher action row.
#[derive(Clone, Copy)]
enum LaunchAction {
    NewInstance,
    Palette,
    Settings,
}

impl Workspace {
    /// Quick create: opens New instance with `engine` already chosen (its
    /// category tab follows the engine).
    pub(crate) fn quick_create(
        &mut self,
        engine: EngineKey,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        self.open_new_instance(window, cx);
        self.choose_engine_action(engine, window, cx);
        cx.notify();
    }

    /// The launcher for an empty centre pane (see the module docs).
    pub(crate) fn render_launcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let empty = self.instances().is_empty();
        let (title, subtitle) = if empty {
            (strings::UI_WELCOME_TITLE, strings::UI_WELCOME_SUBTITLE)
        } else {
            (
                strings::UI_SELECT_INSTANCE_TITLE,
                strings::UI_SELECT_INSTANCE_SUBTITLE,
            )
        };

        let mut card = design::group_card("launcher", &theme)
            .w(px(LAUNCHER_W))
            .child(self.render_quick_create(&theme, cx));
        if !empty {
            card = card.child(self.render_launcher_instances(&theme, cx));
        }
        card = card.child(self.render_launcher_actions(empty, &theme, cx));

        div()
            .id(if empty { "welcome" } else { "select-instance" })
            .flex_1()
            .h_full()
            .min_w_0()
            .bg(theme.background)
            .overflow_y_scroll()
            .child(
                v_flex()
                    .w_full()
                    .min_h_full()
                    .items_center()
                    .justify_center()
                    .gap_5()
                    .py(px(32.))
                    .child(launcher_mark(&theme))
                    .child(
                        v_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Label::new(title)
                                    .text_size(px(20.))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                            )
                            .child(
                                Label::new(subtitle)
                                    .text_sm()
                                    .text_color(theme.muted_foreground),
                            ),
                    )
                    .child(card)
                    .child(
                        h_flex()
                            .gap_1p5()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(icon(IconName::ShieldCheck, 13.0, theme.muted_foreground))
                            .child(Label::new(strings::UI_LAUNCHER_FOOTER)),
                    ),
            )
    }

    /// The engine tiles, three per row.
    fn render_quick_create(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let engines: Vec<EngineKey> = self
            .catalog()
            .engines
            .iter()
            .filter(|entry| entry.available)
            .map(|entry| entry.engine)
            .collect();
        let mut grid = h_flex().flex_wrap().gap_1();
        for engine in engines {
            grid = grid.child(
                h_flex()
                    .id(SharedString::from(format!(
                        "launch-engine-{}",
                        engine.as_str()
                    )))
                    .w(px((LAUNCHER_W - 32.0 - 8.0) / 3.0))
                    .h(px(40.))
                    .px(px(8.))
                    .gap_2p5()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(tokens::hover(theme)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.quick_create(engine, window, cx);
                    }))
                    .child(engine_badge(engine, 28.0, theme))
                    .child(
                        Label::new(engine.display_name())
                            .id(SharedString::from(format!(
                                "launch-engine-name-{}",
                                engine.as_str()
                            )))
                            .truncate(),
                    ),
            );
        }
        v_flex()
            .id("launcher-quick-create")
            .gap_2()
            .p(px(12.))
            .child(
                Label::new(strings::UI_LAUNCHER_QUICK_CREATE.to_uppercase())
                    .id("launcher-quick-create-heading")
                    .pl(px(4.))
                    .text_size(px(11.))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground),
            )
            .child(grid)
    }

    /// The first few instances, one row each, like the dock.
    fn render_launcher_instances(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = v_flex()
            .id("launcher-instances")
            .gap_0p5()
            .p(px(12.))
            .border_t_1()
            .border_color(tokens::hairline(theme))
            .child(
                Label::new(strings::UI_LAUNCHER_INSTANCES.to_uppercase())
                    .id("launcher-instances-heading")
                    .pl(px(4.))
                    .pb_1()
                    .text_size(px(11.))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground),
            );
        for view in self.instances().iter().take(LAUNCHER_INSTANCES) {
            let id = view.id.clone();
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!("launch-instance-{}", view.id)))
                    .h(px(34.))
                    .px(px(8.))
                    .gap_2p5()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(tokens::hover(theme)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_tab(id.clone(), cx);
                        cx.notify();
                    }))
                    .child(engine_icon(view.engine, 16.0, theme))
                    .child(
                        Label::new(view.name.clone())
                            .id(SharedString::from(format!("launch-name-{}", view.id)))
                            .flex_1()
                            .truncate(),
                    )
                    .child(status_glyph(view.status, 7.0, theme))
                    .child(
                        Label::new(format::status_label(view.status))
                            .id(SharedString::from(format!("launch-status-{}", view.id)))
                            .w(px(64.))
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        Label::new(format!("{} · :{}", format::dock_version(view), view.port))
                            .id(SharedString::from(format!("launch-port-{}", view.id)))
                            .w(px(96.))
                            .text_right()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground),
                    ),
            );
        }
        list
    }

    /// New instance, Command palette, Settings.
    fn render_launcher_actions(
        &self,
        empty: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = [
            (
                LaunchAction::NewInstance,
                IconName::Plus,
                if empty {
                    strings::UI_CREATE_FIRST
                } else {
                    strings::UI_NEW_INSTANCE
                },
                "N",
            ),
            (
                LaunchAction::Palette,
                IconName::Command,
                strings::UI_LAUNCHER_PALETTE,
                "K",
            ),
            (
                LaunchAction::Settings,
                IconName::Settings,
                strings::UI_SETTINGS,
                ",",
            ),
        ];
        let mut list = v_flex()
            .id("launcher-actions")
            .gap_0p5()
            .p(px(12.))
            .border_t_1()
            .border_color(tokens::hairline(theme));
        for (index, (action, glyph, label, key)) in rows.into_iter().enumerate() {
            let element_id = if index == 0 && empty {
                SharedString::from("welcome-create")
            } else {
                SharedString::from(format!("launch-action-{index}"))
            };
            list = list.child(
                h_flex()
                    .id(element_id)
                    .h(px(38.))
                    .px(px(8.))
                    .gap_2p5()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(tokens::hover(theme)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        match action {
                            LaunchAction::NewInstance => this.open_new_instance(window, cx),
                            LaunchAction::Palette => this.toggle_palette_in(window, cx),
                            LaunchAction::Settings => this.open_settings(window, cx),
                        }
                        cx.notify();
                    }))
                    .child(design::icon_tile(glyph, 28.0, theme.foreground, theme))
                    .child(
                        Label::new(label)
                            .id(SharedString::from(format!("launch-action-label-{index}")))
                            .flex_1(),
                    )
                    .child(design::kbd(
                        &format!("launch-{index}"),
                        &[modifier_label(), key],
                        theme,
                    )),
            );
        }
        list
    }
}

/// The large app mark above the heading.
fn launcher_mark(theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(52.))
        .rounded(px(14.))
        .bg(theme.primary)
        .child(icon(IconName::Database, 26.0, theme.primary_foreground))
}
