//! Background service work: every `DblService` call runs off the GPUI thread.
//!
//! The trait is synchronous and its implementations take the store mutex, touch
//! SQLite and may wait on the OS keychain. Routing through
//! `cx.background_spawn` keeps the render thread free; results return with
//! `this.update`, so a slow call cannot freeze a frame or drop input.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use dblocal_core::model::CatalogStatus;
use dblocal_core::model::InstanceId;
use dblocal_core::service::{CatalogView, InstanceView};
use dblocal_core::strings;
use gpui_kit::{AppContext as _, Context, Task};

use crate::service_bridge::UiEvent;

use super::{ShellState, Workspace};

impl Workspace {
    /// Runs `work` on the background executor and applies its result on the GPUI
    /// thread. This is the only path a service call may take from the UI.
    pub(crate) fn bg<R>(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> R + Send + 'static,
    ) where
        R: FnOnce(&mut Self, &mut Context<Self>) + Send + 'static,
    {
        let pending = Arc::clone(&self.pending);
        pending.fetch_add(1, Ordering::SeqCst);
        let task: Task<()> = cx.spawn(async move |this, cx| {
            let apply = cx.background_spawn(async move { work() }).await;
            let _ = this.update(cx, |this, cx| {
                pending.fetch_sub(1, Ordering::SeqCst);
                apply(this, cx);
                cx.notify();
            });
        });
        self.bg_tasks.push(task);
        self.bg_tasks.retain(|t| !t.is_ready());
    }

    /// Whether a background service call is in flight right now.
    pub(crate) fn is_pending(&self) -> bool {
        self.pending_preview || self.pending.load(Ordering::SeqCst) > 0
    }

    // Intentionally no synchronous refresh_instances: every DblService read
    // goes through bg() so the GPUI thread never blocks on SQLite/keychain.

    pub(crate) fn refresh_instances_bg(&mut self, cx: &mut Context<Self>) {
        let service = self.service.clone();
        self.bg(cx, move || {
            let instances = service.list_instances().ok();
            let runtimes = service.list_runtimes().ok();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                if let Some(instances) = instances {
                    this.state.replace_instances(instances);
                }
                if let Some(runtimes) = runtimes {
                    this.runtimes = runtimes;
                }
                this.refresh_tray();
                cx.notify();
            }
        });
    }

    /// First read after construction: list, catalog and runtimes, then a catalog
    /// refresh that is *not* awaited (F-CAT-01/02: an offline start still shows
    /// the bundled catalog and the offline banner, never an empty screen).
    pub(crate) fn startup_load(&mut self, cx: &mut Context<Self>) {
        let service = self.service.clone();
        self.bg(cx, move || {
            let instances = service.list_instances().ok();
            let catalog = service.catalog().ok();
            let runtimes = service.list_runtimes().ok();
            let _ = service.refresh_catalog(false);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                if let Some(instances) = instances {
                    this.state.replace_instances(instances);
                }
                if let Some(catalog) = catalog {
                    this.catalog = catalog;
                }
                if let Some(runtimes) = runtimes {
                    this.runtimes = runtimes;
                }
                this.refresh_tray();
                cx.notify();
            }
        });
    }

    pub(crate) fn start_event_pump(&mut self, cx: &mut Context<Self>) {
        let events = self.events.clone();
        let task = cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv_async().await {
                let closed = matches!(event, UiEvent::Closed);
                let _ = this.update(cx, |workspace, cx| {
                    workspace.apply_event_with_cx(event, cx);
                    cx.notify();
                });
                if closed {
                    break;
                }
            }
        });
        self._event_task = Some(task);
    }

    pub(crate) fn start_tray_pump(&mut self, cx: &mut Context<Self>) {
        let actions = self.tray_actions.clone();
        let task = cx.spawn(async move |this, cx| {
            while let Ok(action) = actions.recv_async().await {
                let _ = this.update(cx, |workspace, cx| {
                    workspace.apply_tray_action(action, cx);
                    cx.notify();
                });
            }
        });
        self._tray_task = Some(task);
    }

    /// Refreshes the tray menu from current views; records the note once when no
    /// host exists (F-TRY-01 AC3).
    pub(crate) fn refresh_tray(&mut self) {
        let available = self.tray.refresh(&self.state.instances);
        if !available && !self.tray_note_shown {
            self.tray_note_shown = true;
            self.tray_note = Some(gpui_kit::SharedString::from(
                strings::UI_TRAY_UNAVAILABLE_NOTE,
            ));
        }
    }

    /// Re-reads the catalog in the background (after `CatalogUpdated`).
    pub(crate) fn reload_catalog_bg(&mut self, cx: &mut Context<Self>) {
        let service = self.service.clone();
        self.bg(cx, move || {
            let catalog = service.catalog().ok();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                if let Some(catalog) = catalog {
                    this.catalog = catalog;
                }
                cx.notify();
            }
        });
    }

    /// Drops a removed instance's cached derived state.
    pub(crate) fn forget_instance(&mut self, id: &InstanceId) {
        self.dotenv_cache.remove(id);
        self.port_suggestions.remove(id);
        self.data_sizes.remove(id);
    }

    /// The offline first frame: bundled catalog + banner, never an empty form
    /// (`F-CAT-01`/`F-CAT-02`).
    pub(crate) fn offline_catalog() -> CatalogView {
        CatalogView {
            status: CatalogStatus::Offline,
            banner: Some(strings::CATALOG_OFFLINE.to_owned()),
            engines: Vec::new(),
        }
    }

    /// Seeds the first frame from an `InstanceView` list (previews and tests).
    #[allow(dead_code)]
    pub(crate) fn seed_state(&mut self, instances: Vec<InstanceView>) {
        self.state = ShellState {
            instances,
            downloads: Vec::new(),
            last_failure: None,
            uris: std::collections::HashMap::new(),
        };
    }
}
