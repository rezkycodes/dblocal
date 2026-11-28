//! Background action handlers for the Toolchains GUI (v0.3.0).
//!
//! Every `ToolchainService` call runs off the GPUI thread so the UI never
//! blocks on disk scans, file locks, downloads, or shell rc edits. Progress
//! samples stream back over a `flume` channel (parked with zero idle CPU; no
//! polling timers).

use std::path::PathBuf;
use std::sync::Arc;

use dblocal_core::config::{OpenCommand, OpenLauncher as _, SystemOpenLauncher};
use dblocal_core::download::{CancellationToken, DownloadEvent, ProgressSink};
use dblocal_core::paths::DblPaths;
use dblocal_core::platform::{Arch, Os};
use dblocal_core::toolchain::catalog::{CatalogState, Profile, ToolId, VersionSpec};
use dblocal_core::toolchain::fsops::ToolchainPaths;
use dblocal_core::toolchain::ops::ToolchainService;
use dblocal_core::toolchain::php_ini::SettingKey;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{AppContext as _, Context, Entity, SharedString, Task, Window};

use super::Workspace;
use super::toolchain_state::{
    InstallProgress, PHP_SETTINGS_META, PhpSettingsForm, ToolchainSnapshot, VersionKey,
    tool_display_name,
};

/// Configuration for building a [`ToolchainService`] off-thread.
///
/// Production builds resolve paths from `DblPaths::from_env()` and the real
/// `PATH`. Tests and previews inject a temp directory, a fixture catalog, and
/// a synthetic `PATH` so they never touch `~/.dblocal` or `$HOME`.
#[derive(Clone)]
pub(crate) struct ToolchainBackend {
    pub paths: ToolchainPaths,
    pub home: PathBuf,
    pub os: Os,
    pub arch: Arch,
    pub catalog_override: Option<CatalogState>,
    pub path_var_override: Option<String>,
    #[cfg(any(test, debug_assertions))]
    pub loopback_http: bool,
}

impl ToolchainBackend {
    /// Production backend derived from `DblPaths::from_env()`.
    pub fn from_env() -> Self {
        let dbl_paths = DblPaths::from_env().ok();
        let home = super::workspace_api::home_dir_from_env().unwrap_or_else(|| {
            dbl_paths
                .as_ref()
                .and_then(|p| p.root().parent().map(std::path::Path::to_path_buf))
                .unwrap_or_else(|| PathBuf::from("."))
        });
        let paths = dbl_paths
            .as_ref()
            .map(ToolchainPaths::from_paths)
            .unwrap_or_else(|| ToolchainPaths::with_root(home.join(".dblocal")));
        Self {
            paths,
            home,
            os: Os::current(),
            arch: Arch::current(),
            catalog_override: None,
            path_var_override: None,
            #[cfg(any(test, debug_assertions))]
            loopback_http: false,
        }
    }

    /// Builds a [`ToolchainService`] with the configured paths, platform and
    /// optional overrides.
    pub fn build_service(&self) -> ToolchainService {
        let mut svc = ToolchainService::new(self.paths.clone(), self.home.clone())
            .with_platform(self.os, self.arch);
        if let Some(catalog) = &self.catalog_override {
            svc = svc.with_catalog(catalog.clone());
        }
        #[cfg(any(test, debug_assertions))]
        if self.loopback_http {
            svc = svc.with_loopback_http();
        }
        svc
    }

    /// Reads the effective `PATH` string (override or host environment).
    pub fn path_var(&self) -> String {
        self.path_var_override
            .clone()
            .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
    }
}

impl Workspace {
    /// Selects a toolchain page in the centre pane and refreshes its state off-thread.
    pub(crate) fn select_toolchain(&mut self, tool: ToolId, cx: &mut Context<Self>) {
        self.toolchain.selected_tool = Some(tool);
        self.refresh_toolchains_bg(cx);
        cx.notify();
    }

    /// Leaves the toolchain page (e.g. when selecting an instance tab).
    pub(crate) fn clear_toolchain_selection(&mut self) {
        self.toolchain.selected_tool = None;
    }

    /// Re-scans installed toolchains, active registry, PATH doctor, and PHP
    /// settings off the GPUI thread. Called on startup, on selecting a toolchain
    /// page, and on window activation (no polling timers).
    pub(crate) fn refresh_toolchains_bg(&mut self, cx: &mut Context<Self>) {
        let backend = self.toolchain_backend.clone();
        self.bg(cx, move || {
            let svc = backend.build_service();
            let path_var = backend.path_var();
            let snap = ToolchainSnapshot::capture(&svc, backend.arch, &path_var);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.toolchain.apply_snapshot(snap);
                this.sync_php_setting_fields(cx);
                cx.notify();
            }
        });
    }

    /// Re-scans on window activation if a toolchain page is currently visible.
    pub(crate) fn refresh_toolchains_on_activation(&mut self, cx: &mut Context<Self>) {
        if self.toolchain.selected_tool.is_some() {
            self.refresh_toolchains_bg(cx);
        }
    }

    /// Switches the Linux PHP extension profile selector (`Common` / `Bulk`).
    pub(crate) fn set_php_profile(&mut self, profile: Profile, cx: &mut Context<Self>) {
        self.toolchain.php_profile = profile;
        cx.notify();
    }

    /// Toggles the expandable extension list on one PHP version row.
    pub(crate) fn toggle_php_extensions(&mut self, key: VersionKey, cx: &mut Context<Self>) {
        self.toolchain.toggle_extensions(&key);
        cx.notify();
    }

    /// Starts installing `(tool, version, profile)` off-thread with live
    /// progress and cooperative cancellation. Refuses if already busy.
    pub(crate) fn install_toolchain_version(
        &mut self,
        tool: ToolId,
        version: String,
        profile: Option<Profile>,
        cx: &mut Context<Self>,
    ) {
        let key = VersionKey::new(tool, version.clone(), profile);
        if self.toolchain.installs.contains_key(&key)
            || self.toolchain.externally_busy.contains(&key)
        {
            return;
        }
        let Ok(spec) = VersionSpec::parse(&version) else {
            self.toolchain
                .row_errors
                .insert(key, format!("Invalid version tag: {version}"));
            cx.notify();
            return;
        };

        self.toolchain.row_errors.remove(&key);
        self.toolchain.installs.insert(
            key.clone(),
            InstallProgress {
                bytes_done: 0,
                bytes_total: 0,
                cancelling: false,
            },
        );
        let cancel = CancellationToken::new();
        self.toolchain_cancels.insert(key.clone(), cancel.clone());

        let (tx, rx) = flume::unbounded::<(u64, u64)>();
        let sink: ProgressSink = Arc::new(move |ev| {
            if let DownloadEvent::Progress(p) = ev {
                let _ = tx.send((p.bytes_done, p.bytes_total));
            }
        });

        let backend = self.toolchain_backend.clone();
        let key_for_bg = key.clone();

        // Pump progress samples to the UI while the install runs; parks on the
        // channel and exits as soon as the sender drops when the install finishes.
        let key_for_pump = key.clone();
        let pump: Task<()> = cx.spawn(async move |this, cx| {
            while let Ok((done, total)) = rx.recv_async().await {
                let updated = this.update(cx, |this, cx| {
                    if let Some(prog) = this.toolchain.installs.get_mut(&key_for_pump) {
                        prog.bytes_done = done;
                        prog.bytes_total = total;
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        });
        self.bg_tasks.push(pump);

        self.bg(cx, move || {
            let svc = backend
                .build_service()
                .with_progress(sink)
                .with_cancel(cancel);
            let result = run_async(svc.install(tool, &spec, profile, true));
            let path_var = backend.path_var();
            let snap = ToolchainSnapshot::capture(&svc, backend.arch, &path_var);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.toolchain.installs.remove(&key_for_bg);
                this.toolchain_cancels.remove(&key_for_bg);
                this.toolchain.apply_snapshot(snap);
                this.sync_php_setting_fields(cx);
                match result {
                    Ok(report) => {
                        this.toolchain.row_errors.remove(&key_for_bg);
                        let msg = if report.activated {
                            format!(
                                "Installed and activated {} {}",
                                tool_display_name(report.tool),
                                report.version
                            )
                        } else {
                            format!(
                                "Installed {} {}",
                                tool_display_name(report.tool),
                                report.version
                            )
                        };
                        this.set_toast(SharedString::from(msg));
                        this.refresh_home_size_bg(cx);
                    }
                    Err(err) => {
                        if matches!(err, dblocal_core::DblError::Cancelled { .. }) {
                            this.set_toast(SharedString::from("Install cancelled"));
                        } else {
                            let message = err.user_message();
                            this.toolchain.row_errors.insert(key_for_bg, message);
                            this.report_error(&err);
                        }
                    }
                }
                cx.notify();
            }
        });
        cx.notify();
    }

    /// Requests cancellation of an in-flight install.
    pub(crate) fn cancel_toolchain_install(&mut self, key: &VersionKey, cx: &mut Context<Self>) {
        if let Some(token) = self.toolchain_cancels.get(key) {
            token.cancel();
        }
        if let Some(prog) = self.toolchain.installs.get_mut(key) {
            prog.cancelling = true;
        }
        cx.notify();
    }

    /// Switches the active version of `tool` off-thread.
    pub(crate) fn activate_toolchain_version(
        &mut self,
        tool: ToolId,
        version: String,
        cx: &mut Context<Self>,
    ) {
        let Ok(spec) = VersionSpec::parse(&version) else {
            return;
        };
        let backend = self.toolchain_backend.clone();
        self.bg(cx, move || {
            let svc = backend.build_service();
            let result = svc.activate(tool, &spec);
            let path_var = backend.path_var();
            let snap = ToolchainSnapshot::capture(&svc, backend.arch, &path_var);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.toolchain.apply_snapshot(snap);
                this.sync_php_setting_fields(cx);
                match result {
                    Ok(act) => {
                        let msg = if let Some(warning) = act.warning {
                            format!(
                                "Active {} set to {} ({warning})",
                                tool_display_name(tool),
                                act.active.version()
                            )
                        } else {
                            format!(
                                "Active {} set to {}",
                                tool_display_name(tool),
                                act.active.version()
                            )
                        };
                        this.set_toast(SharedString::from(msg));
                    }
                    Err(err) => this.report_error(&err),
                }
                cx.notify();
            }
        });
    }

    /// Opens the remove-version confirmation dialog.
    pub(crate) fn open_toolchain_remove_dialog(
        &mut self,
        tool: ToolId,
        version: String,
        profile: Option<Profile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dlg) = self.toolchain.build_remove_dialog(tool, &version, profile) else {
            return;
        };
        self.toolchain.remove_dialog = Some(dlg);
        self.begin_modal(window, cx);
        self.focus_modal_root();
        cx.notify();
    }

    /// Runs the confirmed removal off-thread.
    pub(crate) fn confirm_toolchain_remove(&mut self, cx: &mut Context<Self>) {
        let Some(dlg) = self.toolchain.remove_dialog.take() else {
            return;
        };
        self.restore_dialog_focus_after_close();
        let Ok(spec) = VersionSpec::parse(&dlg.version) else {
            return;
        };
        let key = VersionKey::new(dlg.tool, dlg.version.clone(), dlg.profile);
        let backend = self.toolchain_backend.clone();
        self.bg(cx, move || {
            let svc = backend.build_service();
            let result = run_async(svc.remove(dlg.tool, &spec));
            let path_var = backend.path_var();
            let snap = ToolchainSnapshot::capture(&svc, backend.arch, &path_var);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.toolchain.apply_snapshot(snap);
                this.sync_php_setting_fields(cx);
                match result {
                    Ok(report) => {
                        this.toolchain.row_errors.remove(&key);
                        let msg = if report.cleared.is_some() && report.others_remain {
                            format!(
                                "Removed {} {} (no active version selected — choose another below)",
                                tool_display_name(report.tool),
                                report.version
                            )
                        } else {
                            format!(
                                "Removed {} {}",
                                tool_display_name(report.tool),
                                report.version
                            )
                        };
                        this.set_toast(SharedString::from(msg));
                        this.refresh_home_size_bg(cx);
                    }
                    Err(err) => {
                        this.toolchain.row_errors.insert(key, err.user_message());
                        this.report_error(&err);
                    }
                }
                cx.notify();
            }
        });
    }

    /// Opens the Add-to-PATH consent dialog (never edits PATH silently).
    pub(crate) fn open_path_consent_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toolchain.path_consent_open = true;
        self.begin_modal(window, cx);
        self.focus_modal_root();
        cx.notify();
    }

    /// Applies the managed PATH block after explicit consent.
    pub(crate) fn confirm_apply_path(&mut self, cx: &mut Context<Self>) {
        self.toolchain.path_consent_open = false;
        self.restore_dialog_focus_after_close();
        if self.toolchain.path.busy {
            return;
        }
        self.toolchain.path.busy = true;
        self.toolchain.path.last_error = None;
        let backend = self.toolchain_backend.clone();
        self.bg(cx, move || {
            let svc = backend.build_service();
            let result = svc.apply_path();
            let path_var = backend.path_var();
            let snap = ToolchainSnapshot::capture(&svc, backend.arch, &path_var);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.toolchain.path.busy = false;
                this.toolchain.apply_snapshot(snap);
                match result {
                    Ok(report) => {
                        let fail_count = report.failures.len();
                        let changed = report.changed.len();
                        this.toolchain.path.last_report = Some(report);
                        let msg = if fail_count > 0 {
                            format!("Updated {changed} file(s); {fail_count} failed (see details)")
                        } else if changed > 0 {
                            format!("Added DBLocal shims to PATH ({changed} file(s) updated; open a new terminal)")
                        } else {
                            "DBLocal shims PATH block is already up to date".to_owned()
                        };
                        this.set_toast(SharedString::from(msg));
                    }
                    Err(err) => {
                        this.toolchain.path.last_error = Some(err.user_message());
                        this.report_error(&err);
                    }
                }
                cx.notify();
            }
        });
        cx.notify();
    }

    /// Removes the managed PATH block off-thread.
    pub(crate) fn remove_toolchain_path(&mut self, cx: &mut Context<Self>) {
        if self.toolchain.path.busy {
            return;
        }
        self.toolchain.path.busy = true;
        self.toolchain.path.last_error = None;
        let backend = self.toolchain_backend.clone();
        self.bg(cx, move || {
            let svc = backend.build_service();
            let result = svc.remove_path();
            let path_var = backend.path_var();
            let snap = ToolchainSnapshot::capture(&svc, backend.arch, &path_var);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.toolchain.path.busy = false;
                this.toolchain.apply_snapshot(snap);
                match result {
                    Ok(report) => {
                        let fail_count = report.failures.len();
                        let changed = report.changed.len();
                        this.toolchain.path.last_report = Some(report);
                        let msg = if fail_count > 0 {
                            format!("Removed from {changed} file(s); {fail_count} failed")
                        } else {
                            format!("Removed DBLocal shims from PATH ({changed} file(s) restored)")
                        };
                        this.set_toast(SharedString::from(msg));
                    }
                    Err(err) => {
                        this.toolchain.path.last_error = Some(err.user_message());
                        this.report_error(&err);
                    }
                }
                cx.notify();
            }
        });
        cx.notify();
    }

    /// Lazily creates `InputState` entities for the five text PHP settings.
    pub(crate) fn ensure_php_setting_fields(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for meta in PHP_SETTINGS_META {
            if meta.key == SettingKey::DisplayErrors {
                continue;
            }
            if self.php_setting_fields.contains_key(&meta.key) {
                continue;
            }
            let initial = self.toolchain.php_settings.field_text(meta.key);
            let placeholder = meta.placeholder.to_owned();
            let field = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(initial)
            });
            let key = meta.key;
            cx.subscribe(
                &field,
                move |this, input, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        this.toolchain.php_settings.set_field(key, text);
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        let active = this.toolchain.php_settings_enabled();
                        if this.toolchain.php_settings.can_save(active) {
                            this.save_php_settings(cx);
                        }
                    }
                    _ => {}
                },
            )
            .detach();
            self.php_setting_fields.insert(meta.key, field);
        }
    }

    /// Syncs existing PHP setting `InputState` entities from `self.toolchain.php_settings`.
    pub(crate) fn sync_php_setting_fields(&mut self, cx: &mut Context<Self>) {
        let entity_id = cx.entity().entity_id();
        let mut updates: Vec<(Entity<InputState>, String)> = Vec::new();
        for meta in PHP_SETTINGS_META {
            if meta.key == SettingKey::DisplayErrors {
                continue;
            }
            let Some(field) = self.php_setting_fields.get(&meta.key).cloned() else {
                continue;
            };
            let target = self.toolchain.php_settings.field_text(meta.key);
            if field.read(cx).value().as_ref() != target {
                updates.push((field, target));
            }
        }
        if updates.is_empty() {
            return;
        }
        cx.with_window(entity_id, |window, cx| {
            for (field, text) in updates {
                field.update(cx, |state, cx| state.set_value(text, window, cx));
            }
        });
    }

    /// Toggles `display_errors` in the PHP settings form.
    pub(crate) fn toggle_php_display_errors(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.toolchain.php_settings.set_display_errors(enabled);
        cx.notify();
    }

    /// Validates and saves PHP settings off-thread.
    pub(crate) fn save_php_settings(&mut self, cx: &mut Context<Self>) {
        if !self.toolchain.php_settings_enabled() || self.toolchain.php_settings.busy {
            return;
        }
        let Ok(updates) = self.toolchain.php_settings.validated_updates() else {
            cx.notify();
            return;
        };
        self.toolchain.php_settings.busy = true;
        self.toolchain.php_settings.error = None;
        self.toolchain.php_settings.notice = None;
        let backend = self.toolchain_backend.clone();
        self.bg(cx, move || {
            let svc = backend.build_service();
            let result = svc.set_php_settings(&updates);
            let saved = svc.php_settings();
            let dir = svc.php_settings_dir();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.toolchain.php_settings.busy = false;
                match result {
                    Ok(_) => {
                        this.toolchain.php_settings = PhpSettingsForm::from_settings(saved, dir);
                        this.toolchain.php_settings.notice =
                            Some("Saved to 99-dblocal.ini.".to_owned());
                        this.sync_php_setting_fields(cx);
                        this.set_toast(SharedString::from("PHP settings saved"));
                    }
                    Err(err) => {
                        this.toolchain.php_settings.error = Some(err.user_message());
                        this.report_error(&err);
                    }
                }
                cx.notify();
            }
        });
        cx.notify();
    }

    /// Resets PHP settings to defaults off-thread.
    pub(crate) fn reset_php_settings(&mut self, cx: &mut Context<Self>) {
        if !self.toolchain.php_settings_enabled() || self.toolchain.php_settings.busy {
            return;
        }
        self.toolchain.php_settings.busy = true;
        self.toolchain.php_settings.error = None;
        self.toolchain.php_settings.notice = None;
        let backend = self.toolchain_backend.clone();
        self.bg(cx, move || {
            let svc = backend.build_service();
            let result = svc.reset_php_settings();
            let saved = svc.php_settings();
            let dir = svc.php_settings_dir();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.toolchain.php_settings.busy = false;
                match result {
                    Ok(_) => {
                        this.toolchain.php_settings = PhpSettingsForm::from_settings(saved, dir);
                        this.toolchain.php_settings.notice = Some("Reset to defaults.".to_owned());
                        this.sync_php_setting_fields(cx);
                        this.set_toast(SharedString::from("PHP settings reset to defaults"));
                    }
                    Err(err) => {
                        this.toolchain.php_settings.error = Some(err.user_message());
                        this.report_error(&err);
                    }
                }
                cx.notify();
            }
        });
        cx.notify();
    }

    /// Opens the managed PHP `conf.d` folder using the OS default handler.
    pub(crate) fn open_php_config_folder(&mut self, cx: &mut Context<Self>) {
        let dir = self.toolchain.php_settings.config_dir.clone();
        if dir.as_os_str().is_empty() {
            return;
        }
        let os = self.toolchain.os;
        self.bg(cx, move || {
            let _ = std::fs::create_dir_all(&dir);
            let cmd = OpenCommand::for_os(os, &dir);
            let result = SystemOpenLauncher.launch(&cmd);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                match result {
                    Ok(()) => {
                        this.set_toast(SharedString::from("Opened PHP config folder"));
                    }
                    Err(err) => {
                        this.toolchain.php_settings.error =
                            Some(format!("Could not open {}: {err}", dir.display()));
                    }
                }
                cx.notify();
            }
        });
    }
}

/// Runs a toolchain future on a small current-thread tokio runtime inside the
/// background executor thread.
fn run_async<F, T>(fut: F) -> dblocal_core::Result<T>
where
    F: std::future::Future<Output = dblocal_core::Result<T>>,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|source| dblocal_core::DblError::Io { source })?;
    rt.block_on(fut)
}
