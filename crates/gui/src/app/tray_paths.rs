//! Window-less tray handling (F-TRY-01/02, R1/R5).
//!
//! When no `Workspace` exists the app-level pump routes tray actions here: Open
//! opens/focuses the window, Quit loads the instance list off the GPUI thread
//! and then asks in the new window, and Start/Stop call the service directly,
//! logging every failure with its `DBL-NNNN` code and instance id (never a
//! secret).

use std::sync::Arc;

use dblocal_core::service::DblService;
use gpui_kit::{App, AppContext as _};

use super::bootstrap::{AppRuntime, clear_main_window, open_main_window, request_app_quit};

/// Route a tray action. With a live window and workspace open, forward to it;
/// otherwise clear any stale handles and handle at app level (R1/R4/R5).
pub fn handle_tray_action(action: crate::tray::TrayAction, cx: &mut App) {
    let (ws, main_window) = cx
        .try_global::<AppRuntime>()
        .map(|r| (r.workspace.clone(), r.main_window))
        .unwrap_or((None, None));
    let window_live = main_window.is_some_and(|handle| {
        cx.windows()
            .iter()
            .any(|w| w.window_id() == handle.window_id())
    });
    if let Some(ref ws) = ws
        && window_live
    {
        ws.update(cx, |this, cx| this.apply_tray_action(action, cx));
        return;
    }
    if ws.is_some() || main_window.is_some() {
        tracing::warn!(
            ?action,
            "tray action saw stale workspace/window handle; clearing and handling at app level"
        );
        clear_main_window(cx);
    }
    handle_app_tray_action(action, cx);
}

/// App-level tray handling when no window is open.
pub fn handle_app_tray_action(action: crate::tray::TrayAction, cx: &mut App) {
    let Some(service) = cx.try_global::<AppRuntime>().map(|r| r.service.clone()) else {
        tracing::warn!(?action, "tray action dropped: AppRuntime is not installed");
        return;
    };
    match action {
        crate::tray::TrayAction::OpenWindow => {
            if let Err(err) = open_main_window(cx) {
                tracing::warn!("tray open window failed: {err:#}");
            }
        }
        crate::tray::TrayAction::Quit => handle_tray_quit_no_window(service, cx),
        crate::tray::TrayAction::Start(id) => handle_tray_start_no_window(service, id, cx),
        crate::tray::TrayAction::Stop(id) => handle_tray_stop_no_window(service, id, cx),
    }
}

fn handle_tray_quit_no_window(service: Arc<dyn DblService>, cx: &mut App) {
    cx.spawn(async move |cx: &mut gpui_kit::AsyncApp| {
        let svc = service.clone();
        let listed = cx
            .background_spawn(async move { svc.list_instances() })
            .await;
        cx.update(|cx| match listed {
            Ok(views) => {
                let running = crate::quit::running_instance_ids(&views);
                if running.is_empty() {
                    request_app_quit(cx);
                    return;
                }
                match open_main_window(cx) {
                    Ok(_) => {
                        if let Some(ws) = cx
                            .try_global::<AppRuntime>()
                            .and_then(|r| r.workspace.clone())
                        {
                            ws.update(cx, |this, cx| this.request_quit_with(running, cx));
                        } else {
                            tracing::warn!(
                                "tray quit opened window without workspace; cannot show quit dialog"
                            );
                        }
                    }
                    Err(err) => {
                        tracing::warn!("tray quit could not open window: {err:#}");
                    }
                }
            }
            Err(err) => {
                tracing::warn!("tray quit could not list instances: {err}");
                if let Err(open_err) = open_main_window(cx) {
                    tracing::warn!("tray quit could not open error window: {open_err:#}");
                    return;
                }
                let facing = dblocal_core::service::UserFacingError::from(&err);
                if let Some(ws) = cx
                    .try_global::<AppRuntime>()
                    .and_then(|r| r.workspace.clone())
                {
                    ws.update(cx, |this, cx| {
                        this.report_user_facing_error(&facing);
                        cx.notify();
                    });
                } else {
                    tracing::warn!(
                        "tray quit opened error window without workspace; cannot display error"
                    );
                }
            }
        });
    })
    .detach();
}

fn handle_tray_start_no_window(service: Arc<dyn DblService>, id: String, cx: &mut App) {
    let Ok(id) = id.parse::<dblocal_core::model::InstanceId>() else {
        tracing::warn!("tray start: invalid instance id");
        return;
    };
    cx.background_spawn(async move {
        if let Err(err) = service.start_instance(&id) {
            tracing::warn!(
                code = err.code(),
                instance = %id,
                "tray start failed: {err}"
            );
        }
    })
    .detach();
}

fn handle_tray_stop_no_window(service: Arc<dyn DblService>, id: String, cx: &mut App) {
    let Ok(id) = id.parse::<dblocal_core::model::InstanceId>() else {
        tracing::warn!("tray stop: invalid instance id");
        return;
    };
    cx.background_spawn(async move {
        if let Err(err) = service.stop_instance(&id) {
            tracing::warn!(
                code = err.code(),
                instance = %id,
                "tray stop failed: {err}"
            );
        }
    })
    .detach();
}
