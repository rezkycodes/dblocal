//! Tray actions, the quit flow and the window-close rule (F-TRY-01/02).
//!
//! The decision logic lives in `crate::quit` as a pure state machine; this
//! module is the thin GPUI glue that turns its actions into service calls and
//! window operations.

#[cfg(test)]
use std::time::Duration;

use dblocal_core::model::InstanceId;
use dblocal_core::service::OperationId;
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
                self.open_or_focus_main_window(cx);
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

    fn open_or_focus_main_window(&mut self, cx: &mut Context<Self>) {
        cx.defer(|cx| {
            if let Err(err) = crate::app::bootstrap::open_main_window(cx) {
                tracing::error!("could not open main window from tray: {err:#}");
            }
        });
    }

    pub(crate) fn request_quit(&mut self, cx: &mut Context<Self>) {
        let running = running_instance_ids(&self.state.instances);
        self.request_quit_with(running, cx);
    }

    /// Request quit with a precomputed running list (R1: tray quit without
    /// window loads instances from service before opening the window).
    pub(crate) fn request_quit_with(&mut self, running: Vec<InstanceId>, cx: &mut Context<Self>) {
        let action = self.quit.apply(QuitEvent::QuitRequested { running });
        self.handle_quit_action(action, cx);
    }

    fn handle_quit_action(&mut self, action: QuitAction, cx: &mut Context<Self>) {
        match action {
            QuitAction::None | QuitAction::ShowDialog => {
                self.maybe_clear_quit_stops();
                cx.notify();
            }
            QuitAction::StopInstances(ids) => {
                self.quit_stops.clear();
                self.arm_quit_timeout(cx);
                for id in ids {
                    let service = self.service.clone();
                    let iid = id.clone();
                    self.bg(cx, move || {
                        let result = service.stop_instance(&iid);
                        move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                            match result {
                                Ok(op) => this.register_quit_stop(op, iid, cx),
                                Err(err) => {
                                    this.report_error(&err);
                                    let action = this
                                        .quit
                                        .apply(QuitEvent::StopFinished { id: iid, ok: false });
                                    this.handle_quit_action(action, cx);
                                }
                            }
                            cx.notify();
                        }
                    });
                }
                cx.notify();
            }
            QuitAction::Exit => {
                self.quit_stops.clear();
                self.quit_timeout_task = None;
                crate::app::bootstrap::request_app_quit(cx);
            }
        }
    }

    /// Arm the bounded wait for all quit-initiated stops to finish.
    fn arm_quit_timeout(&mut self, cx: &mut Context<Self>) {
        let duration = self.quit_stop_timeout;
        self.quit_timeout_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            let _ = this.update(cx, |this, cx| {
                if matches!(this.quit.state(), QuitState::StoppingAll { .. }) {
                    let action = this.quit.apply(QuitEvent::Timeout);
                    this.handle_quit_action(action, cx);
                }
            });
        }));
    }

    /// Registers a quit-issued stop, honouring an outcome that arrived first
    /// (`OperationFinished` is not ordered against the request continuation).
    pub(crate) fn register_quit_stop(
        &mut self,
        op: OperationId,
        id: InstanceId,
        cx: &mut Context<Self>,
    ) {
        if let Some((id, ok)) = self.quit_stops.register(op, id) {
            let action = self.quit.apply(QuitEvent::StopFinished { id, ok });
            self.handle_quit_action(action, cx);
        }
    }

    fn maybe_clear_quit_stops(&mut self) {
        if !matches!(self.quit.state(), QuitState::StoppingAll { .. }) {
            self.quit_stops.clear();
            self.quit_timeout_task = None;
        }
    }

    /// A quit-initiated stop operation finished (or failed). Only a real
    /// `OperationFinished` advances the quit machine; request acceptance does not.
    pub(crate) fn on_quit_stop_finished(
        &mut self,
        op: &OperationId,
        ok: bool,
        cx: &mut Context<Self>,
    ) {
        let Some((id, ok)) = self.quit_stops.finished(op, ok) else {
            return;
        };
        if !matches!(self.quit.state(), QuitState::StoppingAll { .. }) {
            self.maybe_clear_quit_stops();
            return;
        }
        let action = self.quit.apply(QuitEvent::StopFinished { id, ok });
        self.handle_quit_action(action, cx);
    }

    /// Inject a shorter quit-stop timeout (tests only).
    #[cfg(test)]
    pub(crate) fn set_quit_stop_timeout(&mut self, duration: Duration) {
        self.quit_stop_timeout = duration;
    }

    /// Number of quit-stop operations still waiting for `OperationFinished`.
    #[cfg(test)]
    pub(crate) fn quit_stop_pending_count(&self) -> usize {
        self.quit_stops.pending_len()
    }

    /// Quit-stop operation ids still pending completion (tests only).
    #[cfg(test)]
    pub(crate) fn quit_stop_pending_ops(&self) -> Vec<OperationId> {
        self.quit_stops.pending_ops()
    }

    #[cfg(test)]
    pub(crate) fn quit_stop_buffer_len(&self) -> usize {
        self.quit_stops.early_len()
    }

    pub(crate) fn quit_keep_running(&mut self, cx: &mut Context<Self>) {
        let action = self.quit.apply(QuitEvent::KeepRunning);
        self.handle_quit_action(action, cx);
        self.restore_focus_if_quit_closed();
    }

    pub(crate) fn quit_stop_all(&mut self, cx: &mut Context<Self>) {
        let action = self.quit.apply(QuitEvent::StopAll);
        self.handle_quit_action(action, cx);
    }

    pub(crate) fn quit_cancel(&mut self, cx: &mut Context<Self>) {
        let action = self.quit.apply(QuitEvent::Cancel);
        self.handle_quit_action(action, cx);
        self.restore_focus_if_quit_closed();
    }

    /// `F1`: when the quit dialog is no longer confirming, the keyboard returns to
    /// the opener (or the root fallback).
    pub(crate) fn restore_focus_if_quit_closed(&mut self) {
        if !self.quit.is_confirming() {
            self.restore_dialog_focus_after_close();
        }
    }

    pub(crate) fn quit_force_exit(&mut self, cx: &mut Context<Self>) {
        let action = self.quit.apply(QuitEvent::ForceExit);
        self.handle_quit_action(action, cx);
    }

    /// Called by the window-close hook. With a tray the window is actually
    /// closed and the process stays alive (Linux: cx.hide() is not implemented).
    /// Without a tray the close goes through the quit flow.
    pub(crate) fn handle_window_close_request(&mut self, cx: &mut Context<Self>) {
        let has_tray = cx
            .try_global::<crate::app::bootstrap::AppRuntime>()
            .is_some_and(|r| r.tray_available());
        if has_tray {
            self.detach_from_app_runtime(cx);
        } else {
            self.request_quit(cx);
        }
    }

    /// Detach this workspace when its window closes. The app-level tray pump
    /// keeps handling tray actions.
    pub(crate) fn detach_from_app_runtime(&mut self, cx: &mut Context<Self>) {
        self._event_task = None;
        cx.defer(move |cx| {
            crate::app::bootstrap::clear_main_window(cx);
        });
    }

    /// The quit dialog: default "Leave them running", alternative
    /// "Stop all, then quit", a stop-in-flight state, and a failure state.
    pub(crate) fn render_quit_dialog(
        &self,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::component::FocusTrapElement as _;
        let theme = cx.theme().clone();
        // `F2`: a focus trap keeps Tab inside the quit dialog's buttons.
        let trap = window
            .use_keyed_state(
                gpui_kit::SharedString::from("modal-focus-quit"),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();
        // `F1`/`F2`: the quit dialog opens without an open path that has a
        // window, so take the keyboard on this frame when nothing inside it is
        // focused (the first frame it draws). After that the trap keeps focus in.
        if !trap.contains_focused(window, cx) {
            trap.focus(window, cx);
        }
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
                    .child(self.quit_dialog_buttons(cx, stopping, failed))
                    .focus_trap(gpui_kit::SharedString::from("trap-quit"), &trap),
            )
    }

    /// The dialog's button row: cancel/stop-all/keep while confirming, a force
    /// button while stopping, and cancel/exit-anyway after a failed stop.
    fn quit_dialog_buttons(
        &self,
        cx: &mut Context<Self>,
        stopping: bool,
        failed: bool,
    ) -> impl IntoElement {
        h_flex()
            .gap_2()
            .justify_end()
            .when(!stopping && !failed, |this| {
                this.child(
                    gpui_kit::component::button::Button::new("quit-cancel")
                        .ghost()
                        .label(strings::UI_CANCEL)
                        .on_click(cx.listener(|this, _, _, cx| this.quit_cancel(cx))),
                )
                .child(
                    gpui_kit::component::button::Button::new("quit-stop-all")
                        .ghost()
                        .label(strings::UI_QUIT_STOP_ALL)
                        .on_click(cx.listener(|this, _, _, cx| this.quit_stop_all(cx))),
                )
                .child(
                    gpui_kit::component::button::Button::new("quit-keep")
                        .primary()
                        .label(strings::UI_QUIT_KEEP_RUNNING)
                        .on_click(cx.listener(|this, _, _, cx| this.quit_keep_running(cx))),
                )
            })
            .when(stopping, |this| {
                this.child(
                    gpui_kit::component::button::Button::new("quit-force")
                        .ghost()
                        .label(strings::UI_QUIT_FORCE_CONFIRM)
                        .on_click(cx.listener(|this, _, _, cx| this.quit_force_exit(cx))),
                )
            })
            .when(failed, |this| {
                this.child(
                    gpui_kit::component::button::Button::new("quit-fail-cancel")
                        .ghost()
                        .label(strings::UI_CANCEL)
                        .on_click(cx.listener(|this, _, _, cx| this.quit_cancel(cx))),
                )
                .child(
                    gpui_kit::component::button::Button::new("quit-fail-exit")
                        .primary()
                        .label(strings::UI_QUIT_STOP_ALL)
                        .on_click(cx.listener(|this, _, _, cx| this.quit_force_exit(cx))),
                )
            })
    }
}
