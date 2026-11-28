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

mod config;
mod databases;
mod launcher;
mod logs;
mod overview;
mod toolchain;
mod toolchain_panels;

use dblocal_core::model::InstanceStatus;
use dblocal_core::strings;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_kit::component::label::Label;
use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable, Theme, h_flex,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{Context, IntoElement, ParentElement, SharedString, Styled, Window, div, px};

use crate::format;
use crate::theme::tokens;

use super::Workspace;
use super::components::{engine_badge, engine_icon, icon, status_glyph, status_pill};
use super::state::InnerTab;

impl Workspace {
    /// The centre pane: welcome screen, select-instance prompt, or a tab.
    pub(crate) fn render_center(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if let Some(tool) = self.toolchain.selected_tool {
            return self.render_toolchain_page(tool, window, cx);
        }
        let theme = cx.theme().clone();
        if self.tabs.is_empty() {
            return self.render_launcher(window, cx).into_any_element();
        }

        v_flex()
            .id("center-pane")
            .flex_1()
            .h_full()
            .min_w_0()
            .bg(theme.background)
            .child(self.render_instance_tabs(&theme, window, cx))
            .when_some(self.catalog_banner_el(cx), |this, b| this.child(b))
            .child(self.render_selected(cx))
            .into_any_element()
    }

    /// One tab per opened instance, with its status glyph and a close button.
    ///
    /// `F4`: the tab itself is a keyboard tab stop (Enter/Space selects it) with
    /// the ring; the close button is its own stop.
    fn render_instance_tabs(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id("instance-tabs")
            .h(px(36.))
            .flex_none()
            .items_center()
            .gap_1()
            .px(px(8.))
            .overflow_x_hidden()
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
                let engine = view.map(|view| view.engine);
                let id = tab.instance.clone();
                let tab_focus = super::components::keyed_focus(
                    SharedString::from(format!("itab-focus-{index}")),
                    window,
                    cx,
                );
                // The selected tab is a soft pill (the active wash); the others
                // only take the hover wash.
                let tab_row = h_flex()
                    .id(SharedString::from(format!("itab-{index}")))
                    .h(px(26.))
                    .gap_2()
                    .pl(px(10.))
                    .pr(px(4.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(active, |s| {
                        s.bg(theme.tab_active)
                            .text_color(theme.tab_active_foreground)
                    })
                    .when(!active, |s| {
                        s.text_color(theme.tab_foreground).hover(|s| {
                            s.bg(crate::theme::tokens::hover(theme))
                                .text_color(theme.foreground)
                        })
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_tab(index, cx);
                        cx.notify();
                    }))
                    .when_some(engine, |row, engine| {
                        row.child(engine_icon(engine, 14.0, theme))
                    })
                    .child(Label::new(name))
                    .child(status_glyph(status, 7.0, theme))
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
                    );
                super::components::apply_focus_ring(tab_row, &tab_focus, window, cx)
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
        let id_delete = view.id.clone();
        let label = if running {
            strings::UI_STOP
        } else {
            strings::UI_START
        };
        h_flex()
            .id("instance-header")
            .flex_wrap()
            .gap_4()
            .px(px(24.))
            .pt(px(18.))
            .pb(px(6.))
            .items_center()
            .child(
                div()
                    .p(px(3.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(theme.border)
                    .child(engine_badge(view.engine, 40.0, &theme)),
            )
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
                                    .text_size(px(20.))
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
                            .when(!running, |b| b.primary())
                            .when(running, |b| b.outline())
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
                            .outline()
                            .icon(IconName::RotateCw)
                            .label(strings::UI_ACTION_RESTART)
                            .disabled(!format::can_restart(view.status) || busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.stop_instance(id_restart.clone(), cx);
                                cx.notify();
                            })),
                    )
                    .child(self.render_open_in(view, "header", &theme, cx))
                    // `F-INS-05`: the delete entry point beside the other header
                    // actions. Icon-only trash like the other ghost controls, in
                    // the danger colour so it reads as destructive, with a
                    // danger-tinted wash on hover and press and no fill at rest. The dialog itself explains the
                    // consequence, so the button is always enabled. A normal Tab
                    // stop with the ring and Enter/Space activation.
                    .child(
                        Button::new("header-delete")
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .foreground(theme.danger)
                                    .hover(tokens::destructive_hover(&theme))
                                    .active(tokens::destructive_active(&theme)),
                            )
                            .icon(IconName::Trash)
                            .tooltip(strings::UI_DELETE)
                            .accessibility_label(strings::UI_DELETE)
                            .track_focus(&self.delete_focus.clone().tab_stop(true))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_delete_dialog(id_delete.clone(), window, cx);
                            })),
                    ),
            )
    }

    /// The "Open in…" menu, live over `detect_clients` / `open_in_client`
    /// (`F-OPN-01..02`).
    ///
    /// Entries come from the cached detection, grouped GUI then CLI, with a
    /// refresh item. A client that cannot open this engine is listed disabled.
    /// When nothing was detected the menu offers "Copy URI" instead of an empty
    /// list (`F-OPN-01` AC2). A launch failure is reported as a typed toast.
    ///
    /// `slot` keeps the element ids unique when two of these menus are on screen
    /// at once — the instance header and the Overview Connection card both draw
    /// one (the W2-B2 duplicate-node trap).
    fn render_open_in(
        &self,
        view: &dblocal_core::service::InstanceView,
        slot: &'static str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::component::button::DropdownButton;
        use gpui_kit::component::menu::PopupMenuItem;
        let menu = self.open_in_menu(view.engine);
        let opening = self.opening_instance() == Some(&view.id);
        // "Copy URI" is the fallback when nothing was detected (`F-OPN-01` AC2)
        // and also when every detected client is for another engine, so the menu
        // is never a list of dead rows.
        let copy_uri = menu.copy_uri_fallback || !menu.has_launchable();
        let groups = menu.groups.clone();
        let entity = cx.entity();
        let id_copy = view.id.clone();
        let detection_failed = self.clients.as_ref().is_some_and(|result| result.is_err());

        DropdownButton::new(SharedString::from(format!("{slot}-open-in")))
            .outline()
            .button(
                Button::new(SharedString::from(format!("{slot}-open-in-main")))
                    .icon(IconName::ExternalLink)
                    .label(strings::UI_ACTION_OPEN_IN)
                    .loading(opening)
                    .disabled(opening),
            )
            .dropdown_menu(move |menu, _, _| {
                let mut popup = menu.min_w(px(260.));
                if detection_failed {
                    popup = popup
                        .item(PopupMenuItem::new(strings::UI_OPEN_IN_FAILED).disabled(true))
                        .separator();
                }
                if copy_uri {
                    popup = popup.item(PopupMenuItem::new(strings::UI_COPY_URI).on_click({
                        let entity = entity.clone();
                        let id = id_copy.clone();
                        move |_, window, cx| {
                            entity.update(cx, |this, cx| {
                                this.copy_uri(
                                    id.clone(),
                                    dblocal_core::service::UriFormat::Uri,
                                    window,
                                    cx,
                                );
                            });
                        }
                    }));
                }
                for group in &groups {
                    popup = popup.item(PopupMenuItem::label(group.heading));
                    for entry in &group.entries {
                        let kind = entry.kind;
                        let enabled = entry.enabled();
                        let entity = entity.clone();
                        let instance = id_copy.clone();
                        popup = popup.item(
                            PopupMenuItem::new(entry.label).disabled(!enabled).on_click(
                                move |_, _, cx| {
                                    entity.update(cx, |this, cx| {
                                        this.open_instance_in(instance.clone(), kind, cx);
                                    });
                                },
                            ),
                        );
                    }
                }
                let refresh = entity.clone();
                popup.separator().item(
                    PopupMenuItem::new(strings::UI_OPEN_IN_REFRESH)
                        .icon(gpui_kit::assets::IconName::RefreshCw)
                        .on_click(move |_, _, cx| {
                            refresh.update(cx, |this, cx| {
                                this.detect_clients_refresh(cx);
                            });
                        }),
                )
            })
            .text_color(theme.foreground)
    }

    /// Overview · Logs · Databases · Config.
    fn render_subtabs(&self, active: InnerTab, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .id("subtabs")
            .px(px(24.))
            .pt(px(8.))
            .pb(px(2.))
            .bg(theme.background)
            .child(
                // Segmented control hugs its labels instead of spanning the page.
                h_flex().child(
                    div().flex_none().child(
                        TabBar::new("inner-tabs")
                            .segmented()
                            .selected_index(active.index())
                            .on_click(cx.listener(|this, index: &usize, window, cx| {
                                this.set_inner_tab_with_window(
                                    InnerTab::from_index(*index),
                                    window,
                                    cx,
                                );
                                cx.notify();
                            }))
                            .children(InnerTab::ALL.iter().map(|tab| {
                                Tab::new().aria_label(tab.label()).child(
                                    h_flex()
                                        .gap_1p5()
                                        .items_center()
                                        .child(icon(
                                            subtab_icon(*tab),
                                            14.0,
                                            theme.muted_foreground,
                                        ))
                                        .child(Label::new(tab.label()).id(SharedString::from(
                                            format!("subtab-{}", tab.label()),
                                        ))),
                                )
                            })),
                    ),
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

/// The icon drawn in front of each sub-tab label.
fn subtab_icon(tab: InnerTab) -> IconName {
    match tab {
        InnerTab::Overview => IconName::LayoutDashboard,
        InnerTab::Logs => IconName::ScrollText,
        InnerTab::Databases => IconName::Database,
        InnerTab::Config => IconName::SlidersHorizontal,
    }
}

/// A page container: the 32px gutters and capped width the prototype uses.
pub(crate) fn page() -> gpui_kit::Div {
    v_flex()
        .gap_3()
        .px(px(24.))
        .pt(px(10.))
        .pb(px(24.))
        .max_w(px(1080.))
}

/// A page that fills its parent instead of scrolling, for the Logs tab: its
/// virtualised list needs a definite height to lay out at all.
pub(crate) fn page_fill() -> gpui_kit::Div {
    v_flex()
        .gap_3()
        .px(px(24.))
        .pt(px(10.))
        .pb(px(12.))
        .max_w(px(1080.))
        .flex_1()
        .min_h_0()
}

/// An icon plus a muted label, the pattern most hints use.
pub(crate) fn hint_row(name: IconName, text: String, theme: &Theme) -> impl IntoElement {
    // A11y: one page can draw two hints from this call site (the stopped-URI
    // note and the loopback note), so the row and its label take the text as id.
    let id = SharedString::from(format!("hint-{text}"));
    h_flex()
        .id(id.clone())
        .gap_1p5()
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(icon(name, 14.0, theme.muted_foreground))
        .child(Label::new(text).id(id))
}
