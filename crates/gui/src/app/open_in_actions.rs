//! Action handlers for the "Open in…" menu (`F-OPN-01..02`).
//!
//! Detection is cached for the process and re-run only when the user asks
//! (`refresh == true`), which is the "refresh item" the task requires. Launching
//! goes through `open_in_client`, which builds and runs the launcher plan on the
//! service side; the GUI only reports the outcome. Both calls run on the GPUI
//! background executor, so a slow `which` scan or a spawning process never
//! blocks a frame.

use dblocal_core::clients::{ClientKind, DetectedClient};
use dblocal_core::model::{EngineKey, InstanceId};
use dblocal_core::strings;
use gpui_kit::{Context, SharedString};

use super::Workspace;
use super::open_in::OpenInMenu;

impl Workspace {
    /// The menu for one engine, from the cached detection.
    ///
    /// Before the first read the menu is empty, so it shows "Copy URI" and the
    /// refresh item rather than an invented list.
    pub(crate) fn open_in_menu(&self, engine: EngineKey) -> OpenInMenu {
        match &self.clients {
            Some(Ok(clients)) => OpenInMenu::build(clients, engine),
            _ => OpenInMenu::build(&[], engine),
        }
    }

    /// Reads the detected clients once, unless a read already happened.
    pub(crate) fn ensure_clients_loaded(&mut self, cx: &mut Context<Self>) {
        if self.clients.is_some() {
            return;
        }
        self.detect_clients(false, cx);
    }

    /// Re-runs detection and replaces the cache (the menu's refresh item).
    pub(crate) fn detect_clients_refresh(&mut self, cx: &mut Context<Self>) {
        self.detect_clients(true, cx);
    }

    fn detect_clients(&mut self, refresh: bool, cx: &mut Context<Self>) {
        let service = self.service.clone();
        self.bg(cx, move || {
            let result = service.detect_clients(refresh);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                if let Err(err) = &result {
                    this.report_user_facing_error(err);
                }
                this.clients = Some(result.map(|clients| {
                    clients
                        .into_iter()
                        .map(|client| {
                            DetectedClient::new(
                                client.kind(),
                                client.path().clone(),
                                client.engines().to_vec(),
                            )
                        })
                        .collect::<Vec<_>>()
                }));
                cx.notify();
            }
        });
    }

    /// Opens one instance in `client` (`F-OPN-02`).
    pub(crate) fn open_instance_in(
        &mut self,
        id: InstanceId,
        client: ClientKind,
        cx: &mut Context<Self>,
    ) {
        if self.opening.is_some() {
            return;
        }
        self.opening = Some(id.clone());
        let service = self.service.clone();
        let iid = id.clone();
        let name = client.display_name();
        self.bg(cx, move || {
            let result = service.open_in_client(&iid, client, false);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.opening = None;
                match result {
                    Ok(_plan) => {
                        this.toast = Some(SharedString::from(strings::open_in_launched(name)));
                    }
                    Err(err) => this.report_user_facing_error(&err),
                }
                cx.notify();
            }
        });
        cx.notify();
    }

    /// The instance whose launch is in flight, if any.
    pub(crate) fn opening_instance(&self) -> Option<&InstanceId> {
        self.opening.as_ref()
    }
}
