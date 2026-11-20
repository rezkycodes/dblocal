//! Preview constructors and demo data for headless renders.
//!
//! These build the same `Workspace` the app builds, seeded from a mock service,
//! so reviewers and the render tests can exercise every shell state.

use std::sync::Arc;

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::{DblService, FailureInfo};
use dblocal_core::strings;
use gpui::{Context, Window};

use super::{InnerTab, OpenTab, Workspace};

impl Workspace {
    /// Builds a preview populated from a seeded mock service.
    ///
    /// Preview constructors drive the headless render tests and let reviewers
    /// exercise every shell state without a live session.
    pub fn preview(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::new(service, window, cx);
        if let Some(first) = workspace.instances.first().map(|view| view.id.clone()) {
            workspace.open_tab(first, cx);
        }
        let failure_id = workspace
            .instances
            .iter()
            .find(|view| view.status == InstanceStatus::Failed)
            .map(|view| view.id.clone());
        if let Some(id) = failure_id {
            if let Some(position) = workspace
                .open_tabs
                .iter()
                .position(|tab| tab.instance == id)
            {
                workspace.selected_tab = position;
            } else {
                workspace.open_tabs.push(OpenTab {
                    instance: id.clone(),
                    inner: InnerTab::Overview,
                });
                workspace.selected_tab = workspace.open_tabs.len() - 1;
            }
            workspace.selected_instance = Some(id);
        }
        workspace
    }

    /// Preview with the command palette open.
    pub fn preview_palette(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.palette_open = true;
        workspace
    }

    /// Preview on the Logs tab, with lines already in the buffer.
    pub fn preview_logs(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        if let Some(tab) = workspace.open_tabs.first_mut() {
            tab.inner = InnerTab::Logs;
        }
        workspace.logs.replace_with(demo_log_lines());
        workspace
    }

    /// Preview with the create modal open.
    pub fn preview_create(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.open_create_dialog(window, cx);
        workspace
    }

    /// Preview with the delete modal open on the first instance.
    pub fn preview_delete(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        if let Some(id) = workspace.instances.first().map(|view| view.id.clone()) {
            workspace.open_delete_dialog(id, window, cx);
        }
        workspace
    }

    /// Preview focused on a failed instance, exercising the diagnosis panel.
    pub fn preview_failure(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        let failure = FailureInfo {
            code: "DBL-1005".to_owned(),
            message: strings::ERR_PORT_IN_USE.to_owned(),
            diagnosis: dblocal_core::model::StartFailure::PortInUse,
            log_tail: vec![
                "[error] bind failed: port 5434 in use".to_owned(),
                "[error] could not start listener".to_owned(),
            ],
        };
        if let Some(view) = workspace
            .instances
            .iter_mut()
            .find(|view| view.status == InstanceStatus::Failed)
        {
            view.failure = Some(failure);
            let id = view.id.clone();
            if let Some(tab) = workspace.open_tabs.iter_mut().find(|t| t.instance == id) {
                tab.inner = InnerTab::Overview;
            } else {
                workspace.open_tabs.push(OpenTab {
                    instance: id.clone(),
                    inner: InnerTab::Overview,
                });
                workspace.selected_tab = workspace.open_tabs.len() - 1;
            }
            workspace.selected_instance = Some(id);
        }
        workspace
    }
}

/// Deterministic log lines for preview renders.
pub fn demo_log_lines() -> Vec<String> {
    (0..64)
        .map(|index| match index % 4 {
            0 => format!("[info] baris log contoh {index}"),
            1 => format!("[debug] health check ok ({index}ms)"),
            2 => "[info] listening on 127.0.0.1 only".to_string(),
            _ => format!("[debug] checkpoint complete: wrote {index} buffers"),
        })
        .collect()
}
