//! Tray actions, the quit flow and the window-close rule (F-TRY-01/02).
//!
//! The decision logic lives in `crate::quit` as a pure state machine; this
//! module is the thin GPUI glue that turns its actions into service calls and
//! window operations.

use dblocal_core::model::InstanceId;
use dblocal_core::strings;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::{Context, IntoElement};

use crate::quit::{QuitAction, QuitEvent, QuitState, running_instance_ids};
use crate::tray::TrayAction;

use super::Workspace;

impl Workspace {
    pub(crate) fn apply_tray_action(&mut self, action: TrayAction, cx: &mut Context<Self>) {
        match action {
            TrayAction::OpenWindow => {
                cx.activate(true);
            }
            TrayAction::Quit => self.request_quit(cx),
            TrayAction::Start(id) => {
                if let Ok(id) = id.parse::<InstanceId>() {
                    self.start_instance(id, cx);
                }
            }
            TrayAction::Stop(id) => {
                if let Ok(id) = id.parse::<InstanceId>() {
                    self.stop_instance(id, cx);
                }
            }
        }
    }

    pub(crate) fn request_quit(&mut self, cx: &mut Context<Self>) {
        let running = running_instance_ids(&self.state.instances);
        let action = self.quit.apply(QuitEvent::QuitRequested { running });
        self.handle_quit_action(action, cx);
    }

    fn handle_quit_action(&mut self, action: QuitAction, cx: &mut Context<Self>) {
        match action {
            QuitAction::None | QuitAction::ShowDialog => cx.notify(),
            QuitAction::StopInstances(ids) => {
                for id in ids {
                    let service = self.service.clone();
                    let iid = id.clone();
                    self.bg(cx, move || {
                        let result = service.stop_instance(&iid);
                        move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                            let ok = result.is_ok();
                            if let Err(err) = result {
                                this.report_error(&err);
                            }
                            let action = this.quit.apply(QuitEvent::StopFinished { id: iid, ok });
                            this.handle_quit_action(action, cx);
                            cx.notify();
                        }
                    });
                }
                cx.notify();
            }
            QuitAction::Exit => cx.quit(),
        }
    }

    pub(crate) fn quit_keep_running(&mut self, cx: &mut Context<Self>) {
        let action = self.quit.apply(QuitEvent::KeepRunning);
        self.handle_quit_action(action, cx);
    }

    pub(crate) fn quit_stop_all(&mut self, cx: &mut Context<Self>) {
        let action = self.quit.apply(QuitEvent::StopAll);
        self.handle_quit_action(action, cx);
    }

    pub(crate) fn quit_cancel(&mut self, cx: &mut Context<Self>) {
        let action = self.quit.apply(QuitEvent::Cancel);
        self.handle_quit_action(action, cx);
    }

    pub(crate) fn quit_force_exit(&mut self, cx: &mut Context<Self>) {
        let action = self.quit.apply(QuitEvent::ForceExit);
        self.handle_quit_action(action, cx);
    }

    /// Called by the window-close hook. Hides to tray when a tray is present;
    /// otherwise treats close as quit (F-TRY-02 AC1).
    pub(crate) fn handle_window_close_request(&mut self, cx: &mut Context<Self>) {
        if self.tray.is_available() {
            cx.hide();
        } else {
            self.request_quit(cx);
        }
    }

    /// The quit dialog: default "Biarkan tetap berjalan", alternative
    /// "Hentikan semua lalu keluar", a stop-in-flight state, and a failure state.
    pub(crate) fn render_quit_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (body, stopping, failed) = match self.quit.state() {
            QuitState::Confirming { .. } => (strings::UI_QUIT_BODY.to_owned(), false, false),
            QuitState::StoppingAll { .. } => (strings::UI_QUIT_STOPPING.to_owned(), true, false),
            QuitState::StopFailed { .. } => (strings::UI_QUIT_STOP_FAILED.to_owned(), false, true),
            _ => (String::new(), false, false),
        };
        gpui_kit::div()
            .id("quit-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui_kit::hsla(0., 0., 0., 0.45))
            .child(
                gpui_kit::div()
                    .id("quit-dialog")
                    .w(gpui_kit::px(520.))
                    .rounded(gpui_kit::px(8.))
                    .bg(theme.popover)
                    .border_1()
                    .border_color(theme.border)
                    .p(gpui_kit::px(20.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        gpui_kit::div()
                            .text_lg()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(strings::UI_QUIT_TITLE),
                    )
                    .child(
                        gpui_kit::div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(body),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .when(!stopping && !failed, |this| {
                                this.child(
                                    gpui_kit::component::button::Button::new("quit-cancel")
                                        .ghost()
                                        .label(strings::UI_CANCEL)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.quit_cancel(cx)),
                                        ),
                                )
                                .child(
                                    gpui_kit::component::button::Button::new("quit-stop-all")
                                        .ghost()
                                        .label(strings::UI_QUIT_STOP_ALL)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.quit_stop_all(cx)),
                                        ),
                                )
                                .child(
                                    gpui_kit::component::button::Button::new("quit-keep")
                                        .primary()
                                        .label(strings::UI_QUIT_KEEP_RUNNING)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.quit_keep_running(cx)
                                            }),
                                        ),
                                )
                            })
                            .when(stopping, |this| {
                                this.child(
                                    gpui_kit::component::button::Button::new("quit-force")
                                        .ghost()
                                        .label(strings::UI_QUIT_FORCE_CONFIRM)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.quit_force_exit(cx)),
                                        ),
                                )
                            })
                            .when(failed, |this| {
                                this.child(
                                    gpui_kit::component::button::Button::new("quit-fail-cancel")
                                        .ghost()
                                        .label(strings::UI_CANCEL)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.quit_cancel(cx)),
                                        ),
                                )
                                .child(
                                    gpui_kit::component::button::Button::new("quit-fail-exit")
                                        .primary()
                                        .label(strings::UI_QUIT_STOP_ALL)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.quit_force_exit(cx)),
                                        ),
                                )
                            }),
                    ),
            )
    }
}
