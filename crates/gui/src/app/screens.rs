//! The centre pane: tab strip, instance header, sub-tabs and the four screens.
//!
//! Rendering only. Every branch reads view-model values from
//! [`crate::format`], [`crate::logs`] and `ShellState`; no business logic lives
//! here.
//!
//! Honest placeholders (`D-25`): the Databases tab (`F-DBU`, Phase 2), the
//! Config tab (`F-CFG`, Phase 3) and "Open in…" (`F-OPN`, Phase 2) have no
//! backend in the frozen service. In the product build they say so and disable
//! their controls; only the demo build renders sample rows, so the layout stays
//! reviewable. `shell.rs` decides which of the two runs.

mod databases;
mod logs;
mod overview;

use dblocal_core::model::InstanceStatus;
use dblocal_core::strings;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::label::Label;
use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable, Theme, h_flex,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{Context, IntoElement, ParentElement, SharedString, Styled, div, px};

use crate::format;

use super::Workspace;
use super::components::{engine_badge, icon, status_glyph, status_pill};
use super::state::InnerTab;

impl Workspace {
    /// The centre pane: welcome screen, or the selected instance's tab.
    pub(crate) fn render_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if self.tabs.is_empty() {
            return self.render_welcome(&theme, cx).into_any_element();
        }

        v_flex()
            .id("center-pane")
            .flex_1()
            .h_full()
            .min_w_0()
            .bg(theme.background)
            .child(self.render_instance_tabs(&theme, cx))
            .when_some(self.catalog_banner_el(cx), |this, b| this.child(b))
            .child(self.render_selected(cx))
            .into_any_element()
    }

    /// The empty state: a welcome heading and the first-instance call to action.
    fn render_welcome(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("welcome")
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
                Label::new(strings::UI_WELCOME_TITLE)
                    .text_xl()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD),
            )
            .child(
                Label::new(strings::UI_WELCOME_SUBTITLE)
                    .text_sm()
                    .text_color(theme.muted_foreground),
            )
            .child(
                Button::new("welcome-create")
                    .label(strings::UI_CREATE_FIRST)
                    .primary()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_new_instance(window, cx);
                    })),
            )
    }

    /// One tab per opened instance, with its status glyph and a close button.
    fn render_instance_tabs(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .id("instance-tabs")
            .h(px(36.))
            .flex_none()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .children(self.tabs.open().iter().enumerate().map(|(index, tab)| {
                let active = index == self.tabs.selected_index();
                let view = self.instance(&tab.instance);
                let name = view
                    .map(|view| view.name.clone())
                    .unwrap_or_else(|| strings::UI_INSTANCES.to_owned());
                let status = view
                    .map(|view| view.status)
                    .unwrap_or(InstanceStatus::Stopped);
                let id = tab.instance.clone();
                h_flex()
                    .id(SharedString::from(format!("itab-{index}")))
                    .h_full()
                    .gap_2()
                    .pl(px(14.))
                    .pr(px(8.))
                    .border_r_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .when(active, |s| {
                        s.bg(theme.tab_active)
                            .text_color(theme.tab_active_foreground)
                    })
                    .when(!active, |s| {
                        s.text_color(theme.tab_foreground)
                            .hover(|s| s.text_color(theme.foreground))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_tab(index, cx);
                        cx.notify();
                    }))
                    .child(status_glyph(status, 8.0, theme))
                    .child(Label::new(name))
                    .child(
                        Button::new(SharedString::from(format!("close-tab-{index}")))
                            .ghost()
                            .xsmall()
                            .compact()
                            .icon(IconName::Close)
                            .tooltip(strings::UI_ACTION_CLOSE_TAB)
                            .accessibility_label(strings::UI_ACTION_CLOSE_TAB)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let _ = id;
                                this.close_tab(index, cx);
                            })),
                    )
            }))
    }

    /// The selected tab: header, sub-tab bar and the active screen.
    fn render_selected(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(tab) = self.tabs.selected_tab() else {
            return div().into_any_element();
        };
        let Some(view) = self.instance(&tab.instance).cloned() else {
            return div().into_any_element();
        };
        let inner = tab.inner;
        // Logs keeps its own bounded scroll area so the virtualised list gets a
        // definite height; the other screens scroll the whole page as before.
        let body = if inner == InnerTab::Logs {
            v_flex()
                .id("screen-fill")
                .flex_1()
                .min_h_0()
                .child(self.render_screen(&view, inner, cx))
                .into_any_element()
        } else {
            div()
                .id("screen-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(self.render_screen(&view, inner, cx))
                .into_any_element()
        };
        v_flex()
            .id("selected-instance")
            .flex_1()
            .min_h_0()
            .w_full()
            .child(self.render_header(&view, cx))
            .child(self.render_subtabs(inner, cx))
            .child(body)
            .into_any_element()
    }

    /// Engine badge, name, status chip, subtitle and the header actions.
    fn render_header(
        &self,
        view: &dblocal_core::service::InstanceView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let busy = format::is_busy(view.status);
        let running = view.status == InstanceStatus::Running;
        let id_run = view.id.clone();
        let id_restart = view.id.clone();
        let label = if running {
            strings::UI_STOP
        } else {
            strings::UI_START
        };
        h_flex()
            .id("instance-header")
            .flex_wrap()
            .gap_4()
            .px(px(32.))
            .pt(px(24.))
            .pb(px(6.))
            .items_start()
            .child(engine_badge(view.engine, 44.0, &theme))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(240.))
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2p5()
                            .items_center()
                            .child(
                                Label::new(view.name.clone())
                                    .text_size(px(22.))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                            )
                            .child(status_pill(view.status, &theme)),
                    )
                    .child(
                        Label::new(format::header_subtitle(view))
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("header-run")
                            .icon(if running {
                                IconName::Square
                            } else {
                                IconName::Play
                            })
                            .label(label)
                            .loading(busy)
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if running {
                                    this.stop_instance(id_run.clone(), cx);
                                } else {
                                    this.start_instance(id_run.clone(), cx);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("header-restart")
                            .icon(IconName::RefreshCw)
                            .label(strings::UI_ACTION_RESTART)
                            .disabled(!format::can_restart(view.status) || busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.stop_instance(id_restart.clone(), cx);
                                cx.notify();
                            })),
                    )
                    .child(self.render_open_in(view, &theme, cx)),
            )
    }

    /// The "Open in…" dropdown (placeholder: `F-OPN` is Phase 2).
    ///
    /// In the product build the menu says the backend is not available yet and
    /// every entry is disabled; the demo build lists the prototype's clients
    /// (`F-OPN-01` AC1) so the layout is reviewable.
    fn render_open_in(
        &self,
        _view: &dblocal_core::service::InstanceView,
        theme: &Theme,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::component::button::DropdownButton;
        use gpui_kit::component::menu::PopupMenuItem;
        let placeholder = !self.demo_content;
        DropdownButton::new("header-open-in")
            .button(
                Button::new("header-open-in-main")
                    .icon(IconName::ExternalLink)
                    .label(strings::UI_ACTION_OPEN_IN)
                    .tooltip(strings::UI_PLACEHOLDER_OPEN_IN),
            )
            .dropdown_menu(move |menu, _, _| {
                let menu = menu.min_w(px(240.));
                if placeholder {
                    menu.item(PopupMenuItem::new(strings::UI_PLACEHOLDER_OPEN_IN).disabled(true))
                } else {
                    menu.item(PopupMenuItem::new("psql").disabled(true))
                        .item(PopupMenuItem::new("TablePlus").disabled(true))
                        .item(PopupMenuItem::new(strings::UI_COPY_URI).disabled(true))
                }
            })
            .when(placeholder, |this| this)
            .text_color(theme.muted_foreground)
    }

    /// Overview · Logs · Databases · Config.
    fn render_subtabs(&self, active: InnerTab, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .id("subtabs")
            .px(px(20.))
            .pt(px(6.))
            .bg(theme.background)
            .child(
                TabBar::new("inner-tabs")
                    .underline()
                    .selected_index(active.index())
                    .on_click(cx.listener(|this, index: &usize, _window, cx| {
                        this.set_inner_tab(InnerTab::from_index(*index), cx);
                        cx.notify();
                    }))
                    .children(
                        InnerTab::ALL
                            .iter()
                            .map(|tab| Tab::new().label(tab.label())),
                    ),
            )
    }

    /// The active sub-tab's body.
    fn render_screen(
        &self,
        view: &dblocal_core::service::InstanceView,
        inner: InnerTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        match inner {
            InnerTab::Overview => self.render_overview(view, cx).into_any_element(),
            InnerTab::Logs => self.render_logs(view, cx).into_any_element(),
            InnerTab::Databases => self.render_databases(view, cx).into_any_element(),
            InnerTab::Config => self.render_config(view, cx).into_any_element(),
        }
    }
}

/// A page container: the 32px gutters and capped width the prototype uses.
pub(crate) fn page() -> gpui_kit::Div {
    v_flex()
        .gap_4()
        .px(px(32.))
        .pt(px(10.))
        .pb(px(32.))
        .max_w(px(1080.))
}

/// A page that fills its parent instead of scrolling, for the Logs tab: its
/// virtualised list needs a definite height to lay out at all.
pub(crate) fn page_fill() -> gpui_kit::Div {
    v_flex()
        .gap_3()
        .px(px(32.))
        .pt(px(10.))
        .pb(px(12.))
        .max_w(px(1080.))
        .flex_1()
        .min_h_0()
}

/// An icon plus a muted label, the pattern most hints use.
pub(crate) fn hint_row(name: IconName, text: String, theme: &Theme) -> impl IntoElement {
    h_flex()
        .gap_1p5()
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(icon(name, 14.0, theme.muted_foreground))
        .child(Label::new(text))
}
