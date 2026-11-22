//! Tray actions, the quit flow and the window-close rule (F-TRY-01/02).
//!
//! The decision logic lives in `crate::quit` as a pure state machine; this
//! module is the thin GPUI glue that turns its actions into service calls and
//! window operations.

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
                                Ok(op) => {
                                    this.quit_stops.insert(op, iid);
                                }
                                Err(err) => {
                                    this.report_error(&err);
                                    let action =
                                        this.quit.apply(QuitEvent::StopFinished { id: iid, ok: false });
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
                cx.quit();
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
        let Some(id) = self.quit_stops.remove(op) else {
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
        self.quit_stops.len()
    }

    /// Quit-stop operation ids still pending completion (tests only).
    #[cfg(test)]
    pub(crate) fn quit_stop_pending_ops(&self) -> Vec<OperationId> {
        self.quit_stops.keys().cloned().collect()
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

    /// Release tray pump receiver back to app runtime so a reopened window can
    /// receive tray actions. The service bridge stays alive at app level.
    pub(crate) fn detach_from_app_runtime(&mut self, cx: &mut Context<Self>) {
        self._tray_task = None;
        let actions = self.tray_actions.clone();
        cx.defer(move |cx| {
            if cx.has_global::<crate::app::bootstrap::AppRuntime>() {
                cx.update_global::<crate::app::bootstrap::AppRuntime, _>(|runtime, _| {
                    runtime.restore_tray_actions(actions);
                });
                crate::app::bootstrap::clear_main_window(cx);
            }
        });
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
                    .child(self.quit_dialog_buttons(cx, stopping, failed)),
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

#[cfg(test)]
mod glue_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::sync::Arc;

    use dblocal_core::model::InstanceStatus;
    use dblocal_core::service::DblService;
    use gpui_kit::TestAppContext;

    use super::Workspace;
    use crate::quit::QuitState;
    use crate::tray::TrayAction;

    /// A workspace on the emit-free service, with the tray forced unavailable so
    /// window close is a quit rather than a hide.
    fn fixture(
        cx: &mut TestAppContext,
    ) -> (
        gpui_kit::Entity<Workspace>,
        crate::app::test_service::QuietService,
        &mut gpui_kit::VisualTestContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(crate::theme::init);
        let service = crate::app::test_service::QuietService::three();
        let svc: Arc<dyn DblService> = Arc::new(service.clone());
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::new_preview(svc, window, cx);
            workspace.tray = crate::tray::TrayHandle::unavailable();
            workspace
        });
        cx.run_until_parked();
        (view, service, cx)
    }

    /// A quit with a running instance asks first: the dialog, not the exit.
    #[gpui_kit::test]
    fn quit_with_a_running_instance_opens_the_dialog(cx: &mut TestAppContext) {
        let (view, _service, cx) = fixture(cx);
        view.update(cx, |this, cx| this.request_quit(cx));
        cx.run_until_parked();
        assert!(matches!(
            view.read_with(cx, |this, _| this.quit_machine().state().clone()),
            QuitState::Confirming { .. }
        ));
    }

    /// "Biarkan tetap berjalan" marks the machine done without stopping anything.
    #[gpui_kit::test]
    fn keep_running_does_not_call_stop(cx: &mut TestAppContext) {
        let (view, service, cx) = fixture(cx);
        view.update(cx, |this, cx| this.request_quit(cx));
        cx.run_until_parked();
        let before = service.calls();
        view.update(cx, |this, cx| this.quit_keep_running(cx));
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |this, _| this.quit_machine().state().clone()),
            QuitState::Done
        );
        assert_eq!(
            service.calls(),
            before,
            "keep-running must not touch engines"
        );
    }

    /// Cancel returns to Idle without a dialog and without a stop.
    #[gpui_kit::test]
    fn cancel_returns_to_idle(cx: &mut TestAppContext) {
        let (view, service, cx) = fixture(cx);
        view.update(cx, |this, cx| this.request_quit(cx));
        cx.run_until_parked();
        let before = service.calls();
        view.update(cx, |this, cx| this.quit_cancel(cx));
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |this, _| this.quit_machine().state().clone()),
            QuitState::Idle
        );
        assert_eq!(service.calls(), before);
    }

    /// Q1: request acceptance must not quit; only OperationFinished does.
    #[gpui_kit::test]
    fn stop_all_does_not_quit_before_operation_finishes(cx: &mut TestAppContext) {
        let (view, service, cx) = fixture(cx);
        view.update(cx, |this, cx| this.request_quit(cx));
        cx.run_until_parked();
        let before = service.calls();
        view.update(cx, |this, cx| this.quit_stop_all(cx));
        for _ in 0..40 {
            cx.run_until_parked();
            if view.read_with(cx, |this, _| this.quit_stop_pending_count()) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(service.calls() > before, "stop request must be issued");
        assert!(
            matches!(
                view.read_with(cx, |this, _| this.quit_machine().state().clone()),
                QuitState::StoppingAll { .. }
            ),
            "request acceptance must not transition to Done"
        );
        // Simulate time passing after acceptance, but no OperationFinished.
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
        cx.run_until_parked();
        assert!(
            matches!(
                view.read_with(cx, |this, _| this.quit_machine().state().clone()),
                QuitState::StoppingAll { .. }
            ),
            "still stopping before OperationFinished"
        );
        let ops = view.read_with(cx, |this, _| this.quit_stop_pending_ops());
        for op in ops {
            view.update(cx, |this, cx| {
                this.apply_event_with_cx(
                    crate::service_bridge::UiEvent::OperationFinished { id: op, result: Ok(()) },
                    cx,
                )
            });
            cx.run_until_parked();
        }
        assert_eq!(
            view.read_with(cx, |this, _| this.quit_machine().state().clone()),
            QuitState::Done
        );
    }

    /// Q1: an operation finishing with an error leaves quit in StopFailed.
    #[gpui_kit::test]
    fn stop_all_operation_error_becomes_stop_failed(cx: &mut TestAppContext) {
        let (view, _service, cx) = fixture(cx);
        view.update(cx, |this, cx| this.request_quit(cx));
        cx.run_until_parked();
        view.update(cx, |this, cx| this.quit_stop_all(cx));
        for _ in 0..40 {
            cx.run_until_parked();
            if view.read_with(cx, |this, _| this.quit_stop_pending_count()) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let ops = view.read_with(cx, |this, _| this.quit_stop_pending_ops());
        assert!(!ops.is_empty());
        let err = dblocal_core::service::UserFacingError::new("DBL-9999", "boom");
        for op in ops {
            view.update(cx, |this, cx| {
                this.apply_event_with_cx(
                    crate::service_bridge::UiEvent::OperationFinished {
                        id: op,
                        result: Err(err.clone()),
                    },
                    cx,
                )
            });
            cx.run_until_parked();
        }
        assert!(matches!(
            view.read_with(cx, |this, _| this.quit_machine().state().clone()),
            QuitState::StopFailed { .. }
        ));
    }

    /// Q1: if an operation never finishes, the bounded wait moves to StopFailed.
    #[gpui_kit::test]
    fn stop_all_timeout_moves_to_stop_failed(cx: &mut TestAppContext) {
        let (view, _service, cx) = fixture(cx);
        view.update(cx, |this, _cx| this.set_quit_stop_timeout(std::time::Duration::from_millis(20)));
        view.update(cx, |this, cx| this.request_quit(cx));
        cx.run_until_parked();
        view.update(cx, |this, cx| this.quit_stop_all(cx));
        for _ in 0..40 {
            cx.run_until_parked();
            if view.read_with(cx, |this, _| this.quit_stop_pending_count()) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(matches!(
            view.read_with(cx, |this, _| this.quit_machine().state().clone()),
            QuitState::StoppingAll { .. }
        ));
        cx.executor().advance_clock(std::time::Duration::from_millis(30));
        cx.run_until_parked();
        assert!(matches!(
            view.read_with(cx, |this, _| this.quit_machine().state().clone()),
            QuitState::StopFailed { .. }
        ));
    }

    /// A tray Quit is the same entry point the window-close hook uses.
    #[gpui_kit::test]
    fn tray_quit_opens_the_same_dialog(cx: &mut TestAppContext) {
        let (view, _service, cx) = fixture(cx);
        view.update(cx, |this, cx| this.apply_tray_action(TrayAction::Quit, cx));
        cx.run_until_parked();
        assert!(matches!(
            view.read_with(cx, |this, _| this.quit_machine().state().clone()),
            QuitState::Confirming { .. }
        ));
    }

    /// Tray start/stop route to the same handlers as the window.
    #[gpui_kit::test]
    fn tray_start_routes_to_start_instance(cx: &mut TestAppContext) {
        let (view, service, cx) = fixture(cx);
        let id = service.id_with(InstanceStatus::Stopped);
        let before = service.calls();
        view.update(cx, |this, cx| {
            this.apply_tray_action(TrayAction::Start(id.to_string()), cx)
        });
        cx.run_until_parked();
        assert!(
            service.calls() > before,
            "a tray Start must call the service"
        );
    }

    /// With no tray, closing the window is a quit; with one, it hides.
    #[gpui_kit::test]
    fn close_without_a_tray_asks_to_quit(cx: &mut TestAppContext) {
        let (view, _service, cx) = fixture(cx);
        view.update(cx, |this, cx| this.handle_window_close_request(cx));
        cx.run_until_parked();
        assert!(matches!(
            view.read_with(cx, |this, _| this.quit_machine().state().clone()),
            QuitState::Confirming { .. }
        ));
    }
}
