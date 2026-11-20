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
        if let Some(first) = workspace
            .state
            .instances
            .first()
            .map(|view| view.id.clone())
        {
            workspace.open_tab(first, cx);
        }
        let failure_id = workspace
            .state
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

    /// Preview on the Logs tab of the *selected* instance, with lines loaded.
    ///
    /// The previous version set `open_tabs.first_mut().inner = Logs`, but
    /// `preview` leaves the failed instance selected, so the render showed the
    /// Overview of a different tab. This selects the tab it modifies.
    pub fn preview_logs(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        let index = workspace
            .selected_tab
            .min(workspace.open_tabs.len().saturating_sub(1));
        if let Some(tab) = workspace.open_tabs.get_mut(index) {
            tab.inner = InnerTab::Logs;
            let id = tab.instance.clone();
            workspace.logs.load(&id, demo_log_lines_for(0));
        }
        workspace
    }

    /// Preview with two instance tabs open, both on Logs, the `index`-th
    /// selected, each holding that instance's own lines.
    ///
    /// Proves the per-instance buffers: tab A's buffer never contains tab B's
    /// lines even after a live event for B.
    pub fn preview_logs_for(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
        index: usize,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        let ids: Vec<dblocal_core::model::InstanceId> = workspace
            .state
            .instances
            .iter()
            .take(2)
            .map(|view| view.id.clone())
            .collect();
        for (position, id) in ids.iter().enumerate() {
            workspace.logs.load(id, demo_log_lines_for(position));
            if let Some(tab) = workspace
                .open_tabs
                .iter_mut()
                .find(|tab| &tab.instance == id)
            {
                tab.inner = InnerTab::Logs;
            } else {
                workspace.open_tabs.push(OpenTab {
                    instance: id.clone(),
                    inner: InnerTab::Logs,
                });
            }
        }
        workspace.selected_tab = index.min(workspace.open_tabs.len().saturating_sub(1));
        if let Some(tab) = workspace.open_tabs.get(workspace.selected_tab) {
            workspace.selected_instance = Some(tab.instance.clone());
        }
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
        if let Some(id) = workspace
            .state
            .instances
            .first()
            .map(|view| view.id.clone())
        {
            workspace.open_delete_dialog(id, window, cx);
        }
        workspace
    }

    /// Preview with a stale/offline catalog banner over the centre pane.
    pub fn preview_stale_banner(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.set_catalog_status_for_preview(dblocal_core::model::CatalogStatus::Stale);
        workspace
    }

    /// Preview with every connection URI revealed.
    pub fn preview_revealed(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        if let Some(id) = workspace
            .state
            .instances
            .first()
            .map(|view| view.id.clone())
        {
            workspace.revealed.insert(id);
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
            .state
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

/// Deterministic per-instance log lines for the two-tab render.
pub fn demo_log_lines_for(index: usize) -> Vec<String> {
    (0..24)
        .map(|line| {
            if index == 0 {
                format!("[info] pg-main baris {line} · berkas instance pertama")
            } else {
                format!("[info] valkey-cache baris {line} · berkas instance kedua")
            }
        })
        .collect()
}
