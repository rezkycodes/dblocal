//! Instance and form actions: opening tabs, background reads, run controls,
//! the create form and the delete dialog.
//!
//! Every handler here routes its `DblService` call through
//! [`Workspace::bg`](super::background), so nothing blocks a frame.

use dblocal_core::model::{AuthMode, DeleteMode, EngineKey, InstanceId, Port};
use dblocal_core::service::{UriFormat, UserFacingError};
use dblocal_core::strings;
use gpui_kit::ClipboardItem;
use gpui_kit::component::input::InputState;
use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::prelude::*;
use gpui_kit::{App, Context, SharedString, Window};

use super::create_submit;
use super::{CreateDraft, CreateForm, InnerTab, UriView, Workspace};

impl Workspace {
    pub(crate) fn open_tab(&mut self, id: InstanceId, cx: &mut Context<Self>) {
        self.tabs.open_or_select(id.clone());
        self.load_logs_for(&id, cx);
        let service = self.service.clone();
        let watch_id = id.clone();
        self.bg(cx, move || {
            let _ = service.watch_logs(&watch_id, true);
            move |_: &mut Workspace, _: &mut Context<Workspace>| {}
        });
        if self.state.uri(&id).is_none() {
            let service = self.service.clone();
            let uri_id = id.clone();
            self.bg(cx, move || {
                let uri = service.connection_uri(&uri_id, UriFormat::Uri).ok();
                move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                    if let Some(uri) = uri {
                        this.state.set_uri(&uri_id, &uri);
                    }
                    cx.notify();
                }
            });
        }
        self.load_data_size(&id, cx);
        self.load_dotenv(&id, cx);
        self.load_port_suggestion(&id, cx);
        cx.notify();
    }

    fn load_dotenv(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        if self.dotenv_cache.contains_key(id) {
            return;
        }
        let service = self.service.clone();
        let did = id.clone();
        self.bg(cx, move || {
            let secret = service.connection_uri(&did, UriFormat::DotEnv).ok();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                if let Some(secret) = secret {
                    this.dotenv_cache.insert(did, secret);
                    cx.notify();
                }
            }
        });
    }

    fn load_port_suggestion(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        if self.port_suggestions.contains_key(id) {
            return;
        }
        let engine = self.instance(id).map(|v| v.engine);
        let Some(engine) = engine else {
            return;
        };
        let service = self.service.clone();
        let sid = id.clone();
        self.bg(cx, move || {
            let port = service.suggest_port(engine).ok().map(|p| p.get());
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                if let Some(port) = port {
                    this.port_suggestions.insert(sid, port);
                    cx.notify();
                }
            }
        });
    }

    /// Loads one instance's tail into its own buffer.
    pub(crate) fn load_logs_for(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let log_id = id.clone();
        self.bg(cx, move || {
            let lines = service.tail_logs(&log_id, crate::logs::capacity()).ok();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                if let Some(lines) = lines {
                    this.logs.load(&log_id, lines);
                    if this.visible_logs_following(&log_id) {
                        this.log_scroll.scroll_to_bottom();
                    }
                    cx.notify();
                }
            }
        });
    }

    /// Synchronous log load for previews; production code should use bg.
    pub(crate) fn load_logs_sync(&mut self, id: &InstanceId) {
        if let Ok(lines) = self.service.tail_logs(id, crate::logs::capacity()) {
            self.logs.load(id, lines);
        }
    }

    /// Reads the data directory size once per instance, on first open.
    pub(crate) fn load_data_size(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        if self.data_sizes.contains_key(id) {
            return;
        }
        let service = self.service.clone();
        let size_id = id.clone();
        self.bg(cx, move || {
            let size = service.data_size(&size_id).ok();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                if let Some(size) = size {
                    this.data_sizes.insert(size_id, size);
                    cx.notify();
                }
            }
        });
    }

    /// Whether the currently visible Logs tab is this instance and following.
    pub(crate) fn visible_logs_following(&self, id: &InstanceId) -> bool {
        let Some(tab) = self.tabs.selected_tab() else {
            return false;
        };
        tab.instance == *id
            && tab.inner == InnerTab::Logs
            && self
                .logs
                .get(id)
                .is_some_and(|buffer| buffer.is_following())
    }

    pub(crate) fn start_instance(&mut self, id: InstanceId, cx: &mut Context<Self>) {
        let service = self.service.clone();
        self.bg(cx, move || {
            let result = service.start_instance(&id);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                match result {
                    Ok(_) => {
                        this.toast = Some(SharedString::from(strings::UI_TOAST_START_REQUESTED))
                    }
                    Err(err) => this.report_error(&err),
                }
                cx.notify();
            }
        });
    }

    pub(crate) fn stop_instance(&mut self, id: InstanceId, cx: &mut Context<Self>) {
        let service = self.service.clone();
        self.bg(cx, move || {
            let result = service.stop_instance(&id);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                match result {
                    Ok(_) => {
                        this.toast = Some(SharedString::from(strings::UI_TOAST_STOP_REQUESTED))
                    }
                    Err(err) => this.report_error(&err),
                }
                cx.notify();
            }
        });
    }

    /// Start/stop for the visible tab, which the `Ctrl+Enter` action runs.
    pub(crate) fn toggle_run_selected(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.selected_tab() else {
            return;
        };
        let id = tab.instance.clone();
        let Some(view) = self.instance(&id) else {
            return;
        };
        if crate::format::can_start(view.status) {
            self.start_instance(id, cx);
        } else if crate::format::can_stop(view.status) {
            self.stop_instance(id, cx);
        }
    }

    pub(crate) fn report_error(&mut self, err: &dblocal_core::DblError) {
        let facing = UserFacingError::from(err);
        self.toast = Some(SharedString::from(format!(
            "{} — {}",
            facing.code, facing.message
        )));
        self.last_failure_mirror = Some(facing);
    }

    pub(crate) fn copy_uri(
        &mut self,
        id: InstanceId,
        format: UriFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let service = self.service.clone();
        self.bg(cx, move || {
            let result = service.connection_uri(&id, format);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                match result {
                    Ok(secret) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            secret.expose().to_owned(),
                        ));
                        let message = match format {
                            UriFormat::DotEnv => strings::UI_TOAST_DOTENV_COPIED,
                            _ => strings::UI_TOAST_URI_COPIED,
                        };
                        this.toast = Some(SharedString::from(message));
                    }
                    Err(err) => this.report_error(&err),
                }
                cx.notify();
            }
        });
        let _ = window;
    }

    /// Turns the plaintext on or off for one tab, fetching it only to reveal.
    pub(crate) fn toggle_reveal(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        let revealed = self.state.uri(id).is_some_and(UriView::is_revealed);
        if revealed {
            if let Some(uri) = self.state.uri_mut(id) {
                uri.hide();
            }
            cx.notify();
            return;
        }
        let service = self.service.clone();
        let rid = id.clone();
        self.bg(cx, move || {
            let result = service.connection_uri(&rid, UriFormat::Uri);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                match result {
                    Ok(secret) => {
                        if let Some(uri) = this.state.uri_mut(&rid) {
                            uri.reveal(secret);
                        } else {
                            this.state.set_uri(&rid, &secret);
                        }
                    }
                    Err(err) => this.report_error(&err),
                }
                cx.notify();
            }
        });
    }

    /// Toggles the plaintext for previews without a service call.
    ///
    /// `UriView` already keeps the full URI for masking derived blocks, so a
    /// reveal needs no keychain round-trip. Making this local state removes the
    /// last `self.service` call that could run on the GPUI thread; the
    /// production reveal is [`Self::toggle_reveal`], which uses `bg`.
    pub(crate) fn toggle_reveal_sync(&mut self, id: &InstanceId) {
        let revealed = self.state.uri(id).is_some_and(UriView::is_revealed);
        if revealed {
            if let Some(uri) = self.state.uri_mut(id) {
                uri.hide();
            }
            return;
        }
        if let Some(uri) = self.state.uri_mut(id) {
            let secret = uri.secret().clone();
            uri.reveal(secret);
        }
    }

    pub(crate) fn copy_logs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .tabs
            .selected_tab()
            .and_then(|tab| self.logs.get(&tab.instance))
            .map(|buffer| buffer.copy_text())
            .unwrap_or_default();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.toast = Some(SharedString::from(strings::UI_TOAST_LOGS_COPIED));
        window.push_notification(Notification::success(strings::UI_TOAST_LOGS_COPIED), cx);
        cx.notify();
    }

    pub(crate) fn set_autostart(&mut self, id: &InstanceId, enabled: bool, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let sid = id.clone();
        self.bg(cx, move || {
            let result = service.set_autostart(&sid, enabled);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                match result {
                    Ok(()) => {
                        this.toast = Some(SharedString::from(if enabled {
                            strings::UI_TOAST_AUTOSTART_ON
                        } else {
                            strings::UI_TOAST_AUTOSTART_OFF
                        }));
                        this.refresh_instances_bg(cx);
                    }
                    Err(err) => this.report_error(&err),
                }
                cx.notify();
            }
        });
    }

    pub(crate) fn open_create_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let engine = EngineKey::Postgresql;
        let (version, _) = self.default_engine_version(engine);
        let name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(strings::UI_FIELD_NAME.to_owned()));
        let port_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(strings::UI_FIELD_PORT.to_owned()));
        let name = crate::format::suggested_name(engine, &version);
        name_input.update(cx, |state, cx| state.set_value(name, window, cx));
        self.create = Some(CreateForm {
            engine,
            version,
            auth: AuthMode::Password,
            suggested_port: None,
            name_input: Some(name_input),
            port_input: Some(port_input.clone()),
        });
        // Port suggestion is a service call; do it in background.
        let service = self.service.clone();
        self.bg(cx, move || {
            let suggested = service.suggest_port(engine).ok();
            move |this: &mut Workspace, _cx: &mut Context<Workspace>| {
                if let Some(form) = this.create.as_mut() {
                    form.suggested_port = suggested;
                }
                // Port input is updated by the user or left as suggestion text;
                // we store it on the form. The field shows placeholder.
            }
        });
        cx.notify();
    }

    pub(crate) fn default_engine_version(&self, engine: EngineKey) -> (String, Option<Port>) {
        let version = self
            .catalog
            .engines
            .iter()
            .find(|entry| entry.engine == engine)
            .map(crate::format::default_version)
            .and_then(|version| version.map(|v| v.version.clone()))
            .unwrap_or_default();
        (version, None)
    }

    pub(crate) fn choose_engine(
        &mut self,
        engine: EngineKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (version, _) = self.default_engine_version(engine);
        if let Some(form) = self.create.as_mut() {
            form.engine = engine;
            form.version = version.clone();
            form.suggested_port = None;
        }
        let name = crate::format::suggested_name(engine, &version);
        self.set_create_field_texts(&name, "", window, cx);
        let service = self.service.clone();
        self.bg(cx, move || {
            let suggested = service.suggest_port(engine).ok();
            move |this: &mut Workspace, _cx: &mut Context<Workspace>| {
                if let Some(form) = this.create.as_mut()
                    && form.engine == engine
                {
                    form.suggested_port = suggested;
                }
            }
        });
        cx.notify();
    }

    pub(crate) fn choose_version(
        &mut self,
        version: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let engine = match self.create.as_mut() {
            Some(form) => {
                form.version = version.clone();
                form.engine
            }
            None => return,
        };
        let name = crate::format::suggested_name(engine, &version);
        self.set_create_field_texts(&name, "", window, cx);
        cx.notify();
    }

    /// Writes prefill text into the dialog's inputs without touching the entity
    /// while a borrow of `self.create` is live.
    pub(crate) fn set_create_field_texts(
        &mut self,
        name: &str,
        port: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (name_input, port_input) = match self.create.as_ref() {
            Some(form) => (form.name_input.clone(), form.port_input.clone()),
            None => return,
        };
        if let Some(input) = name_input {
            input.update(cx, |state, cx| state.set_value(name.to_owned(), window, cx));
        }
        if let Some(input) = port_input.filter(|_| !port.is_empty()) {
            input.update(cx, |state, cx| state.set_value(port.to_owned(), window, cx));
        }
    }

    pub(crate) fn choose_auth(&mut self, auth: AuthMode) {
        if let Some(form) = self.create.as_mut() {
            form.auth = auth;
        }
    }

    pub(crate) fn cancel_create(&mut self) {
        self.create = None;
    }

    pub(crate) fn submit_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.create.take() else {
            return;
        };
        let draft = CreateDraft {
            engine: form.engine,
            version: form.version.clone(),
            auth: form.auth,
            name: form
                .name_input
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
                .unwrap_or_default(),
            port_text: form
                .port_input
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
                .unwrap_or_default(),
        };
        let service = self.service.clone();
        let instances = self.state.instances.clone();
        let form_back = form;
        self.bg(cx, move || {
            let result = create_submit::submit(service.as_ref(), &draft, &instances);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| match result {
                Ok(submitted) => {
                    this.state.downloads.push(submitted.download);
                    cx.notify();
                }
                Err(create_submit::SubmitError::Invalid(message)) => {
                    this.create = Some(form_back);
                    this.toast = Some(SharedString::from(message));
                    cx.notify();
                }
                Err(create_submit::SubmitError::Service(err)) => {
                    this.create = Some(form_back);
                    this.report_error(&err);
                    cx.notify();
                }
            }
        });
        let _ = window;
    }

    #[allow(dead_code)]
    pub(crate) fn open_delete_dialog(
        &mut self,
        id: InstanceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(strings::UI_DELETE_NAME_PLACEHOLDER.to_owned())
        });
        self.delete_target = Some(id.clone());
        self.delete_mode = DeleteMode::ArchiveData;
        self.delete_name_input = Some(name_input);
        self.delete_size = 0;
        let service = self.service.clone();
        let did = id.clone();
        self.bg(cx, move || {
            let size = service.data_size(&did).unwrap_or(0);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                if this.delete_target.as_ref() == Some(&did) {
                    this.delete_size = size;
                    cx.notify();
                }
            }
        });
        cx.notify();
    }

    pub(crate) fn delete_confirmation_matches(&self, cx: &App) -> bool {
        let Some(id) = self.delete_target.as_ref() else {
            return false;
        };
        let expected = self
            .instance(id)
            .map(|v| v.name.clone())
            .unwrap_or_default();
        if let Some(input) = self.delete_name_input.as_ref() {
            input.read(cx).value().as_ref() == expected.as_str()
        } else {
            self.delete_typed_name.as_deref() == Some(expected.as_str())
        }
    }

    pub(crate) fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.delete_target.clone() else {
            return;
        };
        if self.delete_mode == DeleteMode::PurgeData && !self.delete_confirmation_matches(cx) {
            self.toast = Some(SharedString::from(strings::UI_DELETE_TYPE_NAME));
            cx.notify();
            return;
        }
        let service = self.service.clone();
        let mode = self.delete_mode;
        self.bg(cx, move || {
            let result = service.delete_instance(&id, mode);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                match result {
                    Ok(_) => {
                        this.close_tab_by_instance(&id, cx);
                        this.delete_target = None;
                        this.delete_name_input = None;
                        this.toast = Some(SharedString::from(strings::UI_TOAST_DELETED));
                        this.refresh_instances_bg(cx);
                    }
                    Err(err) => this.report_error(&err),
                }
                cx.notify();
            }
        });
    }
}
