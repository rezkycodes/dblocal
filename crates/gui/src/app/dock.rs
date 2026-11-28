//! The instances dock: engine groups with counts, collapsible rows, the
//! runtime list and the primary "New instance" button (D-25 prototype).
//!
//! Rendering only. The groups, the filter and the runtime rows all come from
//! [`crate::format`], so the decisions are unit-tested without a window.

use dblocal_core::strings;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, Theme, h_flex, input::Input, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Div, IntoElement, ParentElement, SharedString, Styled, Window, div, px,
};

use crate::format::{self, StatusGlyph};
use crate::theme::tokens;

use super::Workspace;
use super::components::{
    engine_icon, icon, row_destructive_variant, section_label, status_glyph, toolchain_icon,
};
use super::design;

/// Width of the dock in logical pixels; the render tests sample this region.
pub const DOCK_WIDTH: f32 = 272.0;

impl Workspace {
    pub(crate) fn render_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let query = self.filter_text(cx);
        let visible = format::filter_instances(&self.state.instances, &query);
        let groups = format::group_by_engine_refs(&visible);

        let mut tree = v_flex()
            .id("instance-tree")
            .gap_1()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if groups.is_empty() {
            tree = tree.child(
                Label::new(strings::UI_DOCK_NO_MATCHES)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .px_2()
                    .py_3(),
            );
        }
        for group in groups {
            tree = tree.child(self.render_group(&group, &theme, window, cx));
        }

        v_flex()
            .id("sidebar")
            .w(px(DOCK_WIDTH))
            .h_full()
            .flex_none()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .p(px(8.))
            .gap_2p5()
            .child(
                h_flex()
                    .pl(px(8.))
                    .gap_2()
                    .items_center()
                    .child(
                        section_label(strings::UI_DOCK_INSTANCES)
                            .text_color(theme.muted_foreground),
                    )
                    .child(count_badge("instances", self.state.instances.len(), &theme))
                    .child(div().flex_1())
                    .child(
                        Button::new("dock-new")
                            .ghost()
                            .xsmall()
                            .compact()
                            .icon(IconName::Plus)
                            .tooltip(strings::UI_NEW_INSTANCE)
                            .accessibility_label(strings::UI_NEW_INSTANCE)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_new_instance(window, cx);
                            })),
                    ),
            )
            .child(match self.filter_input() {
                Some(input) => Input::new(input)
                    .small()
                    .cleanable(true)
                    .prefix(icon(IconName::Search, 14.0, theme.muted_foreground))
                    .into_any_element(),
                None => div().into_any_element(),
            })
            .child(tree)
            .child(self.render_toolchains(&theme, window, cx))
            .child(self.render_runtimes(&theme, cx))
            .child(
                Button::new("dock-create")
                    .outline()
                    .small()
                    .w_full()
                    .icon(IconName::Plus)
                    .label(strings::UI_NEW_INSTANCE_FOOTER)
                    .child(div().flex_1())
                    .child(design::kbd(
                        "dock-create",
                        &[super::actions::modifier_label(), "N"],
                        &theme,
                    ))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_new_instance(window, cx);
                    })),
            )
    }

    /// One engine group: a collapsible header with its instance count.
    ///
    /// `F4`: the header is a keyboard tab stop (Enter/Space expands/collapses)
    /// and it draws the ring like every other control.
    fn render_group(
        &self,
        group: &format::EngineGroup<'_>,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let engine = group.engine;
        let collapsed = self.is_collapsed(engine);
        let count = group.instances.len();
        let header_icon = if collapsed {
            IconName::ChevronRight
        } else {
            IconName::ChevronDown
        };
        // The header's focus handle lives on the Workspace (one per engine), so a
        // render test can focus it to check the ring (`G2`).
        let header_focus = self.dock_group_focus(engine);
        let header = h_flex()
            .id(SharedString::from(format!("group-{}", engine.as_str())))
            .h(px(28.))
            .px(px(6.))
            .gap_2()
            .rounded(px(6.))
            .cursor_pointer()
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .hover(|s| s.bg(tokens::hover(theme)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_collapsed(engine);
                cx.notify();
            }))
            .child(icon(header_icon, 12.0, theme.muted_foreground))
            .child(engine_icon(engine, 16.0, theme))
            .child(div().flex_1().text_sm().child(group.label))
            .child(count_badge(engine.as_str(), count, theme));
        // `G2`: an inset full outline (all four edges), not the outside ring the
        // dock's clipping can reduce to a single line.
        let header = match &header_focus {
            Some(focus) => super::components::apply_inset_focus_ring(header, focus, window, cx),
            None => header,
        };

        let mut group_box = v_flex().gap_0p5().child(header);
        if !collapsed {
            for view in &group.instances {
                group_box = group_box.child(self.render_row(view, theme, window, cx));
            }
        }
        group_box
    }

    /// One instance row: glyph, name, status, short version and `:port`.
    ///
    /// `F4`: a keyboard tab stop (Enter/Space opens the instance) that draws the
    /// ring like the rest of the shell.
    fn render_row(
        &self,
        view: &dblocal_core::service::InstanceView,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.toolchain.selected_tool.is_none()
            && self.tabs.selected_instance() == Some(&view.id);
        let id = view.id.clone();
        let glyph = format::status_glyph(view.status);
        // A failed row paints its port in the danger colour (task requirement 3).
        let port_colour = if glyph == StatusGlyph::Failed {
            theme.danger
        } else {
            theme.muted_foreground
        };
        let mono = theme.mono_font_family.clone();
        let accessible = format!("{} — {}", view.name, format::status_label(view.status));
        let tooltip_label = accessible.clone();
        let row_focus = super::components::keyed_focus(
            SharedString::from(format!("row-focus-{}", view.id)),
            window,
            cx,
        );
        let row = h_flex()
            .id(SharedString::from(format!("row-{}", view.id)))
            .relative()
            .h(px(30.))
            .pl(px(30.))
            .pr(px(8.))
            .gap_2()
            .rounded(px(6.))
            .cursor_pointer()
            .aria_label(accessible)
            .tooltip(move |_window, cx| {
                cx.new(|_| gpui_kit::component::tooltip::Tooltip::new(tooltip_label.clone()))
                    .into()
            })
            .when(selected, |s| {
                s.bg(tokens::active(theme))
                    .text_color(theme.sidebar_accent_foreground)
                    // The accent bar marks the open instance at a glance.
                    .child(
                        div()
                            .absolute()
                            .left(px(2.))
                            .top(px(8.))
                            .w(px(3.))
                            .h(px(14.))
                            .rounded_full()
                            .bg(theme.primary),
                    )
            })
            .when(!selected, |s| s.hover(|s| s.bg(tokens::hover(theme))))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_tab(id.clone(), cx);
            }))
            .child(status_glyph(view.status, 8.0, theme))
            .child(engine_icon(view.engine, 14.0, theme))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(selected, |s| s.font_weight(gpui_kit::FontWeight::MEDIUM))
                    .child(view.name.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format::dock_version(view)),
            )
            .child(
                div()
                    .w(px(44.))
                    .text_right()
                    .text_xs()
                    .font_family(mono)
                    .text_color(port_colour)
                    .child(format!(":{}", view.port)),
            );
        super::components::apply_focus_ring(row, &row_focus, window, cx)
    }

    /// The TOOLCHAINS section between INSTANCES and RUNTIMES: PHP, Node.js,
    /// Composer with Simple Icons marks and a compact state hint.
    fn render_toolchains(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let summaries = self.toolchain.sidebar_summaries();
        let active_count = summaries.iter().filter(|s| s.has_active).count();
        let mut section = v_flex()
            .id("toolchain-list")
            .debug_selector(|| "sidebar-toolchains".to_owned())
            .gap_0p5()
            .pt(px(10.))
            .border_t_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .px(px(8.))
                    .pb_1()
                    .gap_2()
                    .items_center()
                    .child(
                        section_label(strings::UI_DOCK_TOOLCHAINS)
                            .text_color(theme.muted_foreground),
                    )
                    .child(count_badge("toolchains", active_count, theme)),
            );

        for summary in summaries {
            let tool = summary.tool;
            let selected = self.toolchain.selected_tool == Some(tool);
            let row_focus = super::components::keyed_focus(
                SharedString::from(format!("tc-nav-focus-{}", tool.as_str())),
                window,
                cx,
            );
            let accessible = format!("{} — {}", summary.label, summary.hint);
            let row = h_flex()
                .id(SharedString::from(format!("tc-nav-{}", tool.as_str())))
                .debug_selector(move || format!("tc-nav-{}", tool.as_str()))
                .relative()
                .h(px(28.))
                .pl(px(12.))
                .pr(px(8.))
                .gap_2()
                .rounded(px(6.))
                .cursor_pointer()
                .aria_label(accessible)
                .when(selected, |s| {
                    s.bg(tokens::active(theme))
                        .text_color(theme.sidebar_accent_foreground)
                        .child(
                            div()
                                .absolute()
                                .left(px(2.))
                                .top(px(7.))
                                .w(px(3.))
                                .h(px(14.))
                                .rounded_full()
                                .bg(theme.primary),
                        )
                })
                .when(!selected, |s| s.hover(|s| s.bg(tokens::hover(theme))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_toolchain(tool, cx);
                }))
                .child(toolchain_icon(tool, 14.0, theme))
                .child(
                    Label::new(summary.label)
                        .id(SharedString::from(format!("tc-nav-lbl-{}", tool.as_str())))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .when(selected, |s| s.font_weight(gpui_kit::FontWeight::MEDIUM)),
                )
                .child(
                    Label::new(summary.hint)
                        .id(SharedString::from(format!("tc-nav-hint-{}", tool.as_str())))
                        .flex_none()
                        .text_xs()
                        .text_color(if summary.has_active {
                            theme.foreground
                        } else {
                            theme.muted_foreground
                        }),
                );
            section = section.child(super::components::apply_focus_ring(
                row, &row_focus, window, cx,
            ));
        }

        section
    }

    /// The runtime list: title, instance count, an "unused" chip when unused, and a
    /// remove control on each row. An unused runtime gets a live control
    /// (`F-RTM-04` AC1); a used one gets a disabled control whose tooltip names
    /// what it needs (AC2).
    fn render_runtimes(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = format::runtime_rows(self.runtimes(), &self.state.instances);
        let mut list = v_flex()
            .id("runtime-list")
            .gap_0p5()
            .pt(px(10.))
            .border_t_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .px(px(8.))
                    .pb_1()
                    .gap_2()
                    .items_center()
                    .child(
                        section_label(strings::UI_DOCK_RUNTIMES).text_color(theme.muted_foreground),
                    )
                    .child(count_badge("runtimes", rows.len(), theme)),
            );
        if rows.is_empty() {
            list = list.child(
                Label::new(strings::UI_RUNTIME_UNUSED)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .px(px(10.)),
            );
        }
        for (index, row) in rows.iter().enumerate() {
            let usage = if row.unused {
                design::chip(
                    &format!("runtime-usage-{index}"),
                    row.usage.clone(),
                    theme.warning,
                )
                .into_any_element()
            } else {
                Label::new(row.usage.clone())
                    .id(SharedString::from(format!("runtime-usage-{index}")))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .into_any_element()
            };
            let remove = self.render_runtime_remove_control(index, row, theme, cx);
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!("runtime-row-{index}")))
                    .h(px(28.))
                    .px(px(8.))
                    .gap_2()
                    .rounded(px(6.))
                    .hover(|s| s.bg(tokens::hover(theme)))
                    .child(engine_icon(row.engine, 14.0, theme))
                    .child(
                        Label::new(row.title.clone())
                            .id(SharedString::from(format!("runtime-title-{index}")))
                            .flex_1()
                            .text_xs(),
                    )
                    .child(usage)
                    .child(remove),
            );
        }
        list
    }

    /// One row's remove control. Unused: enabled, opens the confirm dialog.
    /// Used: disabled, with a tooltip naming the reason (`F-RTM-04` AC2).
    fn render_runtime_remove_control(
        &self,
        index: usize,
        row: &format::RuntimeRow,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = format::runtime_remove_label(&row.title);
        let button = Button::new(SharedString::from(format!("runtime-remove-{index}")))
            .custom(row_destructive_variant(theme, cx))
            .small()
            .compact()
            .icon(IconName::Trash)
            .accessibility_label(label);
        if row.unused {
            let id = row.id.clone();
            button
                .tooltip(strings::UI_RUNTIME_REMOVE)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_runtime_remove_dialog(id.clone(), window, cx);
                }))
                .into_any_element()
        } else {
            button
                .disabled(true)
                .tooltip(strings::UI_RUNTIME_IN_USE)
                .into_any_element()
        }
    }
}

/// A small rounded count beside a heading or a group name.
fn count_badge(key: &str, count: usize, theme: &Theme) -> impl IntoElement {
    div()
        .flex_none()
        .min_w(px(18.))
        .h(px(16.))
        .px(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(tokens::active(theme))
        .child(
            Label::new(count.to_string())
                .id(SharedString::from(format!("count-{key}")))
                .text_size(px(10.5))
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(theme.muted_foreground),
        )
}
