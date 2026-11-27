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

    /// Preview with no tab open: the centre pane shows the launcher.
    pub fn preview_launcher(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::new_preview(service, window, cx);
        workspace.demo_content = true;
        workspace.tray_note = None;
        workspace.tray_note_shown = true;
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

    /// Preview with the palette open and its query filtered to a **single** row
    /// ("New instance"), so a render test can prove the panel's height follows
    /// its content instead of stretching to the window bottom (W4-I D2).
    pub fn preview_palette_one_row(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let workspace = Self::preview_palette(service, window, cx);
        let state = workspace.command_state().clone();
        state.update(cx, |state, cx| {
            // The model installs on the first `Command` render; the query is
            // already in the field, so the first install filters to one row.
            state.set_query(dblocal_core::strings::UI_PALETTE_NEW, window, cx);
        });
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
                .map(|index| format!("long log line {index:03} — stable payload"))
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
            workspace.logs.load(&id, vec!["one short line".to_owned()]);
        }
        workspace
    }

    /// Preview of the Logs tab with a short line and, beside it, a 300-character
    /// engine log line (W4-I D3). `UniformList` rows are one line tall, so a
    /// wrapped long line would overprint its neighbour; a render test measures
    /// that each row keeps its height and that no two rows share pixels.
    pub fn preview_logs_long_line(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace
            .tabs
            .set_inner(workspace.tabs.selected_index(), InnerTab::Logs);
        if let Some(id) = workspace.tabs.selected_instance().cloned() {
            // A real PostgreSQL startup shape: a long path makes the
            // `listening on IPv4 address` line cross 300 characters (the prefix
            // is ~110, plus an id and 240 filter bytes).
            let long = format!(
                "LOG:  listening on IPv4 address \"127.0.0.1\", port 5432  \
                 startup-packet /var/lib/dblocal/instances/{id}/data/pg_stat_tmp/{}",
                "x".repeat(240)
            );
            workspace.logs.load(
                &id,
                vec![
                    "short line before".to_owned(),
                    long,
                    "short line after".to_owned(),
                ],
            );
            if let Some(buffer) = workspace.logs.get_mut(&id) {
                buffer.set_following(true);
            }
            workspace.log_scroll.scroll_to_bottom();
        }
        workspace
    }

    /// Preview of the Config tab on the demo service (`F-CFG-01`, `F-CFG-02`).
    pub fn preview_product_config(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.demo_content = false;
        // Set the tab without the background entry read; the preview reads it
        // synchronously below and must start no background work.
        workspace
            .tabs
            .set_inner(workspace.tabs.selected_index(), InnerTab::Config);
        workspace.load_config_sync(cx);
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

    /// Preview with the demo build's Config table open (one row per parameter).
    ///
    /// The render tests draw the live Config tab; this reaches it with the
    /// selected demo instance's settings read synchronously.
    pub fn preview_config_demo(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace
            .tabs
            .set_inner(workspace.tabs.selected_index(), InnerTab::Config);
        workspace.load_config_sync(cx);
        workspace
    }

    /// Two instances each with their Config tab opened; the second is selected.
    ///
    /// Every Config row id is derived from the instance and the key, so two
    /// instances that share `max_connections` cannot collide in the a11y tree
    /// (the W2-B2 trap) even in one frame that draws the tab strip plus a card.
    pub fn preview_two_configs(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        for view in workspace.state.instances.clone() {
            workspace.open_tab_sync(view.id.clone());
            workspace
                .tabs
                .set_inner(workspace.tabs.selected_index(), InnerTab::Config);
            workspace.load_config_sync(cx);
        }
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

    /// Two running PostgreSQL instances that each report a database named `app`.
    ///
    /// Each tab's Databases screen derives its row ids from the instance id and
    /// the database name, so two instances with the same database name cannot
    /// collide in the a11y tree — the trap `F-DBU` names and W2-B2 fixed. Both
    /// tabs open on Databases; the second is selected.
    pub fn preview_two_instances_same_db(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        for view in workspace.state.instances.clone() {
            workspace.open_tab_sync(view.id.clone());
            workspace.set_inner_tab(InnerTab::Databases, cx);
        }
        // Leave the last tab selected; its Databases rows use its own id.
        workspace
    }

    /// Preview with the Databases tab open and its rows already read, so a
    /// render shows the real table rather than the loading line.
    pub fn preview_databases_loaded(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.set_inner_tab(InnerTab::Databases, cx);
        if let Some(id) = workspace.tabs.selected_instance().cloned() {
            workspace.reload_admin_bg(&id, cx);
        }
        workspace
    }

    /// Preview with the type-the-name drop dialog open.
    pub fn preview_drop_dialog(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.set_inner_tab(InnerTab::Databases, cx);
        if let Some(id) = workspace.tabs.selected_instance().cloned() {
            workspace.open_drop_dialog(&id, "postgres".to_owned(), window, cx);
        }
        workspace
    }

    /// Preview with the reset-password confirmation open.
    pub fn preview_reset_dialog(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.set_inner_tab(InnerTab::Databases, cx);
        if let Some(id) = workspace.tabs.selected_instance().cloned() {
            workspace.open_reset_confirm(&id, cx);
        }
        workspace
    }

    /// Preview with the Settings dialog open, on the Settings tab.
    pub fn preview_settings(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        // Open without the login-item read so the preview never touches the
        // host's real login items.
        workspace.settings.open();
        workspace
    }

    /// Preview with the Settings dialog open on an available update.
    pub fn preview_settings_update(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.settings.open();
        workspace.update.auto_check = Some(true);
        workspace
            .update
            .apply_check(Some(&sample_update_status(true)));
        workspace
    }

    /// Preview with a newer release but no update running (icon in the title bar).
    pub fn preview_update_available(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace
            .update
            .apply_check(Some(&sample_update_status(true)));
        workspace
    }

    /// Preview with the update downloading at 41%.
    pub fn preview_update_downloading(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview_update_available(service, window, cx);
        workspace.update.begin_operation(sample_operation());
        workspace.update.apply_progress(
            &sample_operation(),
            &dblocal_core::update::UpdateProgress::Downloading {
                received_bytes: 17 * 1024 * 1024,
                total_bytes: Some(42 * 1024 * 1024),
            },
        );
        workspace
    }

    /// Preview with the update verifying.
    pub fn preview_update_verifying(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview_update_available(service, window, cx);
        workspace.update.begin_operation(sample_operation());
        workspace.update.apply_progress(
            &sample_operation(),
            &dblocal_core::update::UpdateProgress::Verifying,
        );
        workspace
    }

    /// Preview with the update installing.
    pub fn preview_update_installing(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview_update_available(service, window, cx);
        workspace.update.begin_operation(sample_operation());
        workspace
            .apply_update_progress_for_preview(dblocal_core::update::UpdateProgress::Installing);
        workspace
    }

    /// Preview with the update ready to restart.
    pub fn preview_update_ready(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview_update_available(service, window, cx);
        workspace.update.begin_operation(sample_operation());
        workspace.apply_update_progress_for_preview(
            dblocal_core::update::UpdateProgress::ReadyToRestart {
                version: "0.9.0".to_owned(),
                restart_target: "/home/u/.local/bin/dblocal".into(),
            },
        );
        workspace
    }

    /// Preview with a failed update.
    pub fn preview_update_failed(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview_update_available(service, window, cx);
        workspace.update.begin_operation(sample_operation());
        workspace.apply_update_progress_for_preview(dblocal_core::update::UpdateProgress::Failed {
            code: "DBL-8005".to_owned(),
            message:
                "The download is corrupt or does not match its checksum. Try downloading again."
                    .to_owned(),
        });
        workspace
    }

    /// Preview of a failure the previous run persisted (the detached helper's
    /// note), with no newer release known.
    pub fn preview_update_previous_failure(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.update.record_previous_failure(
            "The previous DBLocal process (PID 48211) did not exit within 60s; update aborted.",
        );
        workspace
    }

    /// Preview of a long previous-run failure containing a long Windows path,
    /// to verify that the message wraps inside the dialog padding.
    pub fn preview_update_previous_failure_long(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.update.record_previous_failure(
            "Could not replace C:\\Users\\Administrator\\AppData\\Local\\Programs\\DBLocal\\staging\\dblocal-0.9.0-windows-x86_64\\dblocal.exe: access is denied because another process holds the file open.",
        );
        workspace
    }

    /// Preview of a release build with no signing key: self-update is refused
    /// even on a per-user install, and the reason names the missing key.
    pub fn preview_update_no_signing_key(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace
            .update
            .apply_check(Some(&sample_update_status_no_signing_key()));
        workspace.update.dialog_open = true;
        workspace
    }

    /// Preview of a release build with no signing key, dialog closed: only the
    /// title-bar icon, so a render can prove it is still there.
    pub fn preview_update_no_signing_key_icon(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace
            .update
            .apply_check(Some(&sample_update_status_no_signing_key()));
        workspace
    }

    /// Preview with a newer release this install cannot apply (a distro package).
    pub fn preview_update_cannot(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace
            .update
            .apply_check(Some(&sample_update_status(false)));
        workspace.update.dialog_open = true;
        workspace
    }

    /// Folds a progress sample into the update state, as the event pump does.
    fn apply_update_progress_for_preview(
        &mut self,
        progress: dblocal_core::update::UpdateProgress,
    ) {
        self.update.apply_progress(&sample_operation(), &progress);
    }

    /// Preview with the one-time secret dialog open (a created user password).
    pub fn preview_secret_dialog(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.set_inner_tab(InnerTab::Databases, cx);
        if let Some(id) = workspace.tabs.selected_instance().cloned() {
            workspace.secret_dialog = Some(super::admin_dialogs::SecretDialog {
                instance: id,
                kind: super::admin_dialogs::SecretKind::UserPassword {
                    user: "app_ro".to_owned(),
                },
                value: dblocal_core::secret::SecretString::new("pwd-user-0001"),
            });
        }
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

    /// Preview of the create modal with "Only show latest versions" unchecked
    /// and an older patch present on the 17.x line so the extra row is visible.
    pub fn preview_create_all_versions(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview_create(service, window, cx);
        if let Some(entry) = workspace
            .catalog
            .engines
            .iter_mut()
            .find(|entry| entry.engine == dblocal_core::model::EngineKey::Postgresql)
        {
            entry.versions.insert(
                1,
                dblocal_core::service::VersionEntry {
                    version: "17.3".to_owned(),
                    line: "17".to_owned(),
                    eol: false,
                    status: None,
                    installed: false,
                    size_bytes: 54_000_000,
                },
            );
        }
        workspace.set_latest_only(false, window, cx);
        workspace
    }

    /// Preview of the create modal with Redis selected (Cache & Queue tab).
    pub fn preview_create_redis(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview_create(service, window, cx);
        workspace.choose_engine_action(dblocal_core::model::EngineKey::Redis, window, cx);
        workspace
    }

    /// Preview of the create modal on the Storage tab (RustFS with console checkbox).
    pub fn preview_create_storage(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview_create(service, window, cx);
        workspace.choose_category_action(crate::format::EngineCategory::Storage, window, cx);
        workspace.set_console_enabled_action(true);
        workspace
    }

    /// Preview of a running RustFS instance overview with console enabled,
    /// plus sidebar icons and RUNTIMES list.
    pub fn preview_rustfs_overview(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        let Ok(name) = dblocal_core::model::InstanceName::new("s3-local") else {
            return workspace;
        };
        let Ok(port) = dblocal_core::model::Port::new(9000) else {
            return workspace;
        };
        let Ok(console_port) = dblocal_core::model::Port::new(9001) else {
            return workspace;
        };
        let Ok(spec) = dblocal_core::model::InstanceSpec::new(
            name,
            dblocal_core::model::EngineKey::Rustfs,
            "1.0.7",
            Some(port),
            dblocal_core::model::AuthMode::Password,
        ) else {
            return workspace;
        };
        let spec = spec.with_extra_port(Some(console_port));
        let inst = dblocal_core::model::Instance::new(
            &spec,
            dblocal_core::model::InstanceId::new(),
            dblocal_core::model::InstanceStatus::Running,
            std::time::SystemTime::UNIX_EPOCH,
        )
        .with_superuser("DBLPREVIEWACCESSKEY1");
        let id = inst.id().clone();
        let mut view = dblocal_core::service::InstanceView::from_instance(&inst);
        view = view.with_credential_backend(dblocal_core::model::CredentialBackend::Keychain);
        workspace.state.instances.push(view);
        workspace.runtimes.push(dblocal_core::model::Runtime::new(
            "rustfs-1.0.7-linux-x86_64",
            dblocal_core::model::EngineKey::Rustfs,
            "1.0.7",
            dblocal_core::platform::Arch::X86_64,
            std::path::PathBuf::from("/runtimes/rustfs/1.0.7"),
            112_388_130,
            std::time::SystemTime::UNIX_EPOCH,
            dblocal_core::model::RuntimeState::Installed,
        ));
        let endpoint = dblocal_core::secret::SecretString::new("http://127.0.0.1:9000");
        let dotenv = dblocal_core::secret::SecretString::new(
            "AWS_ENDPOINT_URL=http://127.0.0.1:9000\nAWS_ACCESS_KEY_ID=DBLPREVIEWACCESSKEY1\nAWS_SECRET_ACCESS_KEY=preview-secret-key-24-chars\nRUSTFS_CONSOLE_URL=http://127.0.0.1:9001\n",
        );
        workspace.state.set_uri(&id, &endpoint);
        workspace.dotenv_cache.insert(id.clone(), dotenv);
        workspace.tabs.open_or_select(id);
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

    /// Preview of the create modal with the **MySQL** engine card focused (not
    /// the default-selected PostgreSQL), so a render test can prove the ring is
    /// drawn: the selected card's border is already the primary colour, so only a
    /// focused *unselected* card shows the ring as new pixels (`F3`).
    pub fn preview_create_engine_focused(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let workspace = Self::preview_create(service, window, cx);
        if let Some(handle) = workspace.create_form().and_then(|form| {
            form.engine_focus
                .iter()
                .find(|(key, _)| *key == dblocal_core::model::EngineKey::Mysql)
                .map(|(_, h)| h.clone())
        }) {
            handle.focus(window, cx);
        }
        workspace
    }

    /// Preview of the create modal with the version row **17.4** focused but not
    /// selected (the dialog selects 16.6), so a render test can prove the
    /// version-row ring is a full outline on all four edges (`G2`).
    pub fn preview_version_row_focused(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let workspace = Self::preview_create(service, window, cx);
        if let Some(handle) = workspace.create_form().and_then(|form| {
            form.version_focus
                .iter()
                .find(|(tag, _)| tag == "17.4")
                .map(|(_, h)| h.clone())
        }) {
            handle.focus(window, cx);
        }
        workspace
    }

    /// Preview with the PostgreSQL engine-group header focused, so a render test
    /// can prove the group-header ring is a full outline on all four edges (`G2`).
    pub fn preview_dock_group_focused(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let workspace = Self::preview(service, window, cx);
        if let Some(handle) = workspace.dock_group_focus(dblocal_core::model::EngineKey::Postgresql)
        {
            handle.focus(window, cx);
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
            workspace.delete_size = workspace.service.data_size(&id).unwrap_or(0);
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

/// The operation id the update previews use, so their progress samples land.
fn sample_operation() -> dblocal_core::service::OperationId {
    dblocal_core::service::OperationId::new("preview-update")
}

/// The sample newer release the update previews render (`0.9.0`).
fn sample_update_status(can_self_update: bool) -> dblocal_core::service::UpdateStatus {
    let install_kind = if can_self_update {
        dblocal_core::update::InstallKind::PortableTarball {
            exe_path: "/home/u/.local/bin/dblocal".into(),
        }
    } else {
        dblocal_core::update::InstallKind::DistPackage {
            exe_path: "/usr/bin/dblocal".into(),
            hint: dblocal_core::update::PackageManagerHint::Rpm,
        }
    };
    let release_url = "https://github.com/rezkycodes/dblocal/releases/tag/v0.9.0".to_owned();
    let asset_name = if can_self_update {
        "dblocal-0.9.0-linux-x86_64.tar.gz".to_owned()
    } else {
        "dblocal-0.9.0-1.x86_64.rpm".to_owned()
    };
    dblocal_core::service::UpdateStatus {
        current: "0.8.0".to_owned(),
        available: Some(dblocal_core::service::AvailableUpdateView {
            version: "0.9.0".to_owned(),
            tag: "v0.9.0".to_owned(),
            release_notes: "Sample release notes.".to_owned(),
            release_url: release_url.clone(),
            asset_name: asset_name.clone(),
            size_bytes: 42 * 1024 * 1024,
            can_self_update,
        }),
        cannot_update_reason: install_kind
            .cannot_update_reason_with(Some(&release_url), Some(&asset_name)),
        install_kind,
        package_manager_hint: None,
    }
}

/// A sample check result for a release build with no signing key configured:
/// the install kind is fine, but the signing gate refuses the update.
fn sample_update_status_no_signing_key() -> dblocal_core::service::UpdateStatus {
    let kind = dblocal_core::update::InstallKind::PortableTarball {
        exe_path: "/home/u/.local/bin/dblocal".into(),
    };
    let reason = "Release signing public key is not configured in this build; automatic self-update is disabled.";
    dblocal_core::service::UpdateStatus {
        current: "0.8.0".to_owned(),
        available: Some(dblocal_core::service::AvailableUpdateView {
            version: "0.9.0".to_owned(),
            tag: "v0.9.0".to_owned(),
            release_notes: "Sample release notes.".to_owned(),
            release_url: "https://github.com/rezkycodes/dblocal/releases/tag/v0.9.0".to_owned(),
            asset_name: "dblocal-0.9.0-linux-x86_64.tar.gz".to_owned(),
            size_bytes: 42 * 1024 * 1024,
            can_self_update: false,
        }),
        install_kind: kind,
        package_manager_hint: None,
        cannot_update_reason: Some(reason.to_owned()),
    }
}
