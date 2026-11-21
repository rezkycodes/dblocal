//! Preview constructors and demo data for headless renders.
//!
//! These build the same `Workspace` the app builds, seeded from a mock service,
//! so reviewers and the render tests can exercise every shell state.

use std::sync::Arc;

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::{DblService, FailureInfo};
use dblocal_core::strings;
use gpui_kit::{Context, Window};

use super::{InnerTab, Workspace};

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
            workspace.tabs.open_or_select(id);
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
            .tabs
            .selected_index()
            .min(workspace.tabs.len().saturating_sub(1));
        if let Some(tab) = workspace.tabs.open().get(index) {
            let id = tab.instance.clone();
            workspace.tabs.set_inner(index, InnerTab::Logs);
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
            let tab_index = workspace.tabs.open_or_select(id.clone());
            workspace.tabs.set_inner(tab_index, InnerTab::Logs);
        }
        let wanted = index.min(workspace.tabs.len().saturating_sub(1));
        workspace.tabs.select(wanted);
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
            let tab_index = workspace.tabs.open_or_select(id.clone());
            workspace.tabs.set_inner(tab_index, InnerTab::Overview);
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

impl Workspace {
    /// Preview after the first instance was removed, as a finished delete does.
    ///
    /// Exercises the real `InstanceRemoved` path (`ShellState::apply_removed`)
    /// so a render can prove the dock no longer lists the instance.
    pub fn preview_after_delete(
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
            workspace
                .state
                .apply_removed(&id, &mut workspace.tabs, &mut workspace.logs);
        }
        workspace
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use dblocal_core::model::{
        AuthMode, EngineKey, InstanceName, InstanceSpec, InstanceStatus, Port,
    };
    use dblocal_core::service::{DblService, mock::MockService};

    use crate::logs::LogStore;

    fn seeded() -> Arc<dyn DblService> {
        let service = MockService::deterministic();
        for (name, engine, version, port, status) in [
            (
                "pg-main",
                EngineKey::Postgresql,
                "17.4",
                5432,
                InstanceStatus::Running,
            ),
            (
                "pg-report",
                EngineKey::Postgresql,
                "16.6",
                5433,
                InstanceStatus::Stopped,
            ),
            (
                "valkey-cache",
                EngineKey::Valkey,
                "8.1.0",
                6379,
                InstanceStatus::Running,
            ),
        ] {
            let spec = InstanceSpec::new(
                InstanceName::new(name).expect("name"),
                engine,
                version,
                Some(Port::new(port).expect("port")),
                AuthMode::Password,
            )
            .expect("spec");
            service.seed_instance(spec, status);
        }
        Arc::new(service)
    }

    #[test]
    fn a_deleted_instance_leaves_the_dock_and_its_tab() {
        let service = seeded();
        let instances = service.list_instances().expect("list");
        let target = instances
            .iter()
            .find(|view| view.name == "pg-report")
            .expect("seeded instance")
            .clone();

        // Build the same collections the workspace holds.
        let mut state = crate::app::ShellState {
            instances: instances.clone(),
            downloads: Vec::new(),
            last_failure: None,
        };
        let mut tabs = crate::app::Tabs::default();
        let mut logs = LogStore::new();
        tabs.open_or_select(target.id.clone());
        logs.load(&target.id, vec!["line".to_owned()]);

        // What the dock listed before the delete.
        let before: Vec<String> = crate::format::group_by_engine(&state.instances)
            .into_iter()
            .flat_map(|group| group.instances.into_iter().map(|v| v.name.clone()))
            .collect();
        assert!(before.contains(&target.name), "instance starts in the dock");

        state.apply_removed(&target.id, &mut tabs, &mut logs);

        // What the dock lists after it: the row is gone from the model the
        // sidebar renders, its tab is closed and the selection no longer points
        // at it.
        let after: Vec<String> = crate::format::group_by_engine(&state.instances)
            .into_iter()
            .flat_map(|group| group.instances.into_iter().map(|v| v.name.clone()))
            .collect();
        assert!(
            !after.contains(&target.name),
            "deleted instance still in the dock: {after:?}"
        );
        assert_eq!(after.len(), before.len() - 1);
        assert!(tabs.open().iter().all(|tab| tab.instance != target.id));
        assert!(tabs.selected_instance().is_none());
        assert!(logs.get(&target.id).is_none());
    }

    #[test]
    fn deleting_the_selected_instance_keeps_the_other_tabs() {
        let service = seeded();
        let instances = service.list_instances().expect("list");
        assert!(instances.len() >= 2);
        let mut state = crate::app::ShellState {
            instances,
            downloads: Vec::new(),
            last_failure: None,
        };
        let mut tabs = crate::app::Tabs::default();
        let mut logs = LogStore::new();
        let first = state.instances[0].id.clone();
        let second = state.instances[1].id.clone();
        tabs.open_or_select(first.clone());
        tabs.open_or_select(second.clone());

        state.apply_removed(&second, &mut tabs, &mut logs);

        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.open()[0].instance, first);
        assert!(tabs.selected_instance().is_none());
    }
}
