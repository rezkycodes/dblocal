//! Preview constructors for headless renders (demo/headless builds only).
//!
//! These build the same `Workspace` the app builds, seeded from the demo
//! service, so the render tests can exercise every screen. The demo flag also
//! turns on the sample content in the placeholder tabs, because the product
//! build must never invent rows (`D-25`).

use std::sync::Arc;

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::DblService;
use gpui_kit::{AppContext as _, Context, Window};

use super::{InnerTab, Workspace};

impl Workspace {
    /// Builds a preview populated from a seeded demo service.
    pub fn preview(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::new_preview(service, window, cx);
        workspace.demo_content = true;
        // Previews never start a tray; do not claim it is missing either.
        workspace.tray_note = None;
        workspace.tray_note_shown = true;
        // The prototype's status bar carries a live download row.
        workspace.state.on_download(
            dblocal_core::service::OperationId::new("demo-download-pg-15"),
            dblocal_core::model::DownloadProgress::new(
                220 * 1024 * 1024,
                980 * 1024 * 1024,
                24 * 1024 * 1024,
                Some(std::time::Duration::from_secs(32)),
            ),
            dblocal_core::strings::UI_DOWNLOADS.to_owned(),
        );
        if let Some(first) = workspace
            .state
            .instances
            .iter()
            .find(|view| view.status == InstanceStatus::Running)
            .map(|view| view.id.clone())
        {
            workspace.open_tab_sync(first);
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

    /// Preview of a failed instance's Logs tab, exercising the diagnosis banner.
    pub fn preview_failure(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        if let Some(id) = workspace
            .state
            .instances
            .iter()
            .find(|view| view.status == InstanceStatus::Failed)
            .map(|view| view.id.clone())
        {
            let index = workspace.tabs.open_or_select(id.clone());
            workspace.load_logs_sync(&id);
            workspace.tabs.set_inner(index, InnerTab::Logs);
        }
        workspace
    }

    /// Preview of a failed instance's Overview, which shows the diagnosis banner.
    pub fn preview_failed_overview(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        if let Some(id) = workspace
            .state
            .instances
            .iter()
            .find(|view| view.status == InstanceStatus::Failed)
            .map(|view| view.id.clone())
        {
            let index = workspace.tabs.open_or_select(id.clone());
            workspace.load_logs_sync(&id);
            workspace.tabs.set_inner(index, InnerTab::Overview);
        }
        workspace
    }

    /// Preview on the Logs tab of the running instance.
    pub fn preview_logs(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        if let Some(id) = workspace.tabs.selected_instance().cloned() {
            workspace.load_logs_sync(&id);
            workspace
                .tabs
                .set_inner(workspace.tabs.selected_index(), InnerTab::Logs);
        }
        workspace
    }

    /// Preview of the Logs tab with a 200-line buffer and follow on, so the
    /// virtualised list must paint the newest lines at the bottom.
    pub fn preview_logs_long(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        // Switch to Logs first: `set_inner_tab` reloads the demo tail, which
        // would overwrite our custom buffer.
        workspace
            .tabs
            .set_inner(workspace.tabs.selected_index(), InnerTab::Logs);
        if let Some(id) = workspace.tabs.selected_instance().cloned() {
            let lines = (0..200)
                .map(|index| format!("baris log panjang {index:03} — payload stabil"))
                .collect::<Vec<_>>();
            workspace.logs.load(&id, lines);
            if let Some(buffer) = workspace.logs.get_mut(&id) {
                buffer.set_following(true);
            }
            workspace.log_scroll.scroll_to_bottom();
        }
        workspace
    }

    /// Preview of the Logs tab with only a short buffer.
    pub fn preview_logs_short(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace
            .tabs
            .set_inner(workspace.tabs.selected_index(), InnerTab::Logs);
        if let Some(id) = workspace.tabs.selected_instance().cloned() {
            workspace
                .logs
                .load(&id, vec!["satu baris pendek".to_owned()]);
        }
        workspace
    }

    /// Preview of the Config tab placeholder in the product build (`Fase 3`).
    pub fn preview_product_config(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.demo_content = false;
        workspace.set_inner_tab(InnerTab::Config, cx);
        workspace
    }

    /// Preview with the Databases tab open (placeholder or demo content).
    pub fn preview_databases(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.set_inner_tab(InnerTab::Databases, cx);
        workspace
    }

    /// Preview of the product build: no demo content in the placeholder tabs.
    pub fn preview_product_databases(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.demo_content = false;
        workspace.set_inner_tab(InnerTab::Databases, cx);
        workspace
    }

    /// Preview with the create modal open on a version that needs a download.
    ///
    /// PostgreSQL 17.4 is installed in the demo, so the preview selects 16.6
    /// (not installed) and the footer must promise the SHA-256-verified
    /// download (`F-INS-01` AC2).
    pub fn preview_create(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.open_create_dialog(window, cx);
        workspace.choose_version("16.6".to_owned(), window, cx);
        workspace
    }

    /// Preview of the create modal with an engine unavailable on this platform.
    pub fn preview_create_unavailable(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.open_create_dialog(window, cx);
        // Force the MariaDB card to read as unavailable, exactly as the catalog
        // read model would on macOS or Linux aarch64 (F-CAT-02 AC2).
        if let Some(entry) = workspace
            .catalog
            .engines
            .iter_mut()
            .find(|entry| entry.engine == dblocal_core::model::EngineKey::Mariadb)
        {
            entry.available = false;
            entry.unavailable_reason =
                Some(dblocal_core::strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM.to_owned());
        }
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
            let name_input = cx.new(|cx| {
                gpui_kit::component::input::InputState::new(window, cx)
                    .placeholder(dblocal_core::strings::UI_DELETE_NAME_PLACEHOLDER.to_owned())
            });
            workspace.delete_target = Some(id.clone());
            workspace.delete_mode = dblocal_core::model::DeleteMode::ArchiveData;
            workspace.delete_name_input = Some(name_input);
            workspace.delete_size = workspace
                .service
                .data_size(&id)
                .unwrap_or(0);
        }
        workspace
    }

    /// Preview with the quit dialog open on a running instance (F-TRY-02 AC2).
    pub fn preview_quit(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        let running = crate::quit::running_instance_ids(&workspace.state.instances);
        let action = workspace
            .quit
            .apply(crate::quit::QuitEvent::QuitRequested { running });
        debug_assert!(matches!(action, crate::quit::QuitAction::ShowDialog));
        workspace
    }

    /// Preview of the quit dialog reporting a failed stop.
    pub fn preview_quit_stop_failed(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        let running = crate::quit::running_instance_ids(&workspace.state.instances);
        workspace
            .quit
            .apply(crate::quit::QuitEvent::QuitRequested { running });
        workspace.quit.apply(crate::quit::QuitEvent::StopAll);
        if let Some(id) = workspace
            .state
            .instances
            .first()
            .map(|view| view.id.clone())
        {
            workspace
                .quit
                .apply(crate::quit::QuitEvent::StopFinished { id, ok: false });
        }
        workspace
    }

    /// Preview with a background service call in flight (neutral pending note).
    pub fn preview_pending(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.pending_preview = true;
        workspace
    }

    /// Preview of a build with no tray host: the status bar says so once.
    pub fn preview_no_tray(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.tray_available = false;
        workspace.tray_note_shown = true;
        workspace.tray_note = Some(gpui_kit::SharedString::from(
            dblocal_core::strings::UI_TRAY_UNAVAILABLE_NOTE,
        ));
        workspace
    }

    /// Preview with the dock filter applied, so only matching rows survive.
    pub fn preview_filtered(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.set_filter_text_for_preview("pg-", window, cx);
        workspace
    }

    /// Preview with a stale catalog banner over the centre pane.
    pub fn preview_stale_banner(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.catalog.status = dblocal_core::model::CatalogStatus::Stale;
        workspace.catalog.banner = Some(dblocal_core::strings::CATALOG_STALE.to_owned());
        workspace
    }

    /// Preview with every connection URI revealed.
    pub fn preview_revealed(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        if let Some(id) = workspace.tabs.selected_instance().cloned() {
            workspace.toggle_reveal_sync(&id);
        }
        workspace
    }

    /// Preview after the first instance was removed, as a finished delete does.
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

    /// Sets the dock filter text through the preview's input entity.
    fn set_filter_text_for_preview(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(input) = self.filter_input.clone() {
            input.update(cx, |state, cx| state.set_value(text.to_owned(), window, cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use dblocal_core::model::{
        AuthMode, EngineKey, InstanceId, InstanceName, InstanceSpec, InstanceStatus, Port,
    };
    use dblocal_core::service::{DblService, mock::MockService};

    use crate::logs::LogStore;

    fn seeded() -> Arc<dyn DblService> {
        let service = MockService::demo();
        Arc::new(service)
    }

    #[test]
    fn the_demo_seeds_a_running_and_a_failed_instance() {
        let service = seeded();
        let instances = service.list_instances().expect("list");
        assert!(
            instances
                .iter()
                .any(|view| view.status == InstanceStatus::Running)
        );
        assert!(
            instances
                .iter()
                .any(|view| view.status == InstanceStatus::Failed)
        );
    }

    #[test]
    fn a_deleted_instance_leaves_the_dock_and_its_tab() {
        let service = seeded();
        let instances = service.list_instances().expect("list");
        let target = instances
            .iter()
            .find(|view| view.name == "pg-legacy")
            .expect("seeded instance")
            .clone();

        let mut state = crate::app::ShellState {
            instances: instances.clone(),
            downloads: Vec::new(),
            last_failure: None,
            uris: std::collections::HashMap::new(),
        };
        let mut tabs = crate::app::Tabs::default();
        let mut logs = LogStore::new();
        tabs.open_or_select(target.id.clone());
        logs.load(&target.id, vec!["line".to_owned()]);

        let before: Vec<String> = crate::format::group_by_engine(&state.instances)
            .into_iter()
            .flat_map(|group| group.instances.into_iter().map(|v| v.name.clone()))
            .collect();
        assert!(before.contains(&target.name), "instance starts in the dock");

        state.apply_removed(&target.id, &mut tabs, &mut logs);

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
            uris: std::collections::HashMap::new(),
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
    }

    #[test]
    fn the_demo_dock_groups_every_engine_but_valkey_present() {
        let service = seeded();
        let instances = service.list_instances().expect("list");
        let groups = crate::format::group_by_engine(&instances);
        assert_eq!(groups.len(), 4, "all four engines have an instance");
        let total: usize = groups.iter().map(|group| group.instances.len()).sum();
        assert_eq!(total, 6, "the prototype cast has six instances");
    }

    #[test]
    fn the_download_preview_picks_a_version_that_is_not_installed() {
        // Defect 7: the default PostgreSQL version in the demo (17.4) is
        // installed, so the create render never showed a download. The preview
        // selects 16.6 instead and the footer must promise a download.
        let service = seeded();
        let catalog = service.catalog().expect("catalog");
        let entry = catalog
            .engines
            .iter()
            .find(|entry| entry.engine == EngineKey::Postgresql)
            .expect("postgresql entry");
        let selected = entry
            .versions
            .iter()
            .find(|version| version.version == "16.6")
            .expect("16.6 in the demo catalog");
        assert!(!selected.installed, "16.6 must need a download");
        assert_eq!(
            crate::format::create_footer_note(selected.installed),
            dblocal_core::strings::UI_CREATE_DOWNLOAD_NOTE
        );
    }

    #[test]
    fn the_demo_env_block_never_shows_a_demo_password() {
        // Reproduces the coordinator's round-2 finding on `overview-dark.png`:
        // the MariaDB `.env` printed `DATABASE_URL=mysql://root:pwd-…@host/`.
        let service = seeded();
        for view in service.list_instances().expect("list") {
            if view.auth != AuthMode::Password {
                continue;
            }
            let uri = service
                .connection_uri(&view.id, dblocal_core::service::UriFormat::Uri)
                .expect("uri");
            let dotenv = service
                .connection_uri(&view.id, dblocal_core::service::UriFormat::DotEnv)
                .expect("dotenv");
            let password = uri
                .expose()
                .split_once("://")
                .and_then(|(_, rest)| rest.split_once('@').map(|(creds, _)| creds.to_owned()))
                .and_then(|creds| creds.split_once(':').map(|(_, p)| p.to_owned()))
                .expect("demo uri carries a password");
            assert!(!password.is_empty());

            let rows = crate::app::masked_env_rows(dotenv.expose(), false, Some(&uri));
            let text: String = rows
                .iter()
                .map(|row| match &row.key {
                    Some(key) => format!("{key}={}", row.value),
                    None => row.value.clone(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !text.contains(&password),
                "{} leaked its demo password: {text}",
                view.name
            );
            assert!(
                text.contains("••••••"),
                "{} hidden env block has no mask: {text}",
                view.name
            );
        }
    }

    #[test]
    fn an_instance_view_can_be_built_for_any_engine() {
        // Guards the fixture helper the render tests use.
        for engine in EngineKey::ALL {
            let spec = InstanceSpec::new(
                InstanceName::new("probe").expect("name"),
                engine,
                "17.4",
                Some(Port::new(5600).expect("port")),
                AuthMode::Password,
            )
            .expect("spec");
            let instance = dblocal_core::model::Instance::new(
                &spec,
                InstanceId::new(),
                InstanceStatus::Stopped,
                std::time::SystemTime::UNIX_EPOCH,
            );
            let view = dblocal_core::service::InstanceView::from_instance(&instance);
            assert_eq!(view.engine, engine);
        }
    }
}
