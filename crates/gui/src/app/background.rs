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
use dblocal_core::service::CatalogView;
use dblocal_core::strings;
use gpui_kit::{AppContext as _, BorrowAppContext, Context, Task};

use crate::service_bridge::UiEvent;

use super::Workspace;

fn dir_size_bytes(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

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
                this.refresh_tray(cx);
                cx.notify();
            }
        });
    }

    /// First read after construction: list, catalog and runtimes. The catalog
    /// refresh runs in its own background task so the UI receives the local state
    /// before a slow/offline refresh returns (F-CAT-01/02).
    pub(crate) fn startup_load(&mut self, cx: &mut Context<Self>) {
        let service = self.service.clone();
        self.bg(cx, move || {
            let instances = service.list_instances().ok();
            let catalog = service.catalog().ok();
            let runtimes = service.list_runtimes().ok();
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
                this.refresh_tray(cx);
                cx.notify();
            }
        });
        let service = self.service.clone();
        self.bg(cx, move || {
            let _ = service.refresh_catalog(false);
            move |_: &mut Workspace, _: &mut Context<Workspace>| {}
        });
        self.refresh_home_size_bg(cx);
    }

    /// Recompute the total DBLocal home size in the background.
    pub(crate) fn refresh_home_size_bg(&mut self, cx: &mut Context<Self>) {
        let root = self.home_root.clone();
        self.bg(cx, move || {
            let size = dir_size_bytes(&root);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.home_size_bytes = Some(size);
                cx.notify();
            }
        });
    }

    /// Refresh tray from the live workspace's view list.
    pub(crate) fn refresh_tray(&mut self, cx: &mut gpui_kit::Context<Self>) {
        let available = if cx.has_global::<crate::app::bootstrap::AppRuntime>() {
            cx.update_global::<crate::app::bootstrap::AppRuntime, _>(|runtime, _| {
                runtime.refresh_tray(&self.state.instances)
            })
        } else {
            self.tray_available
        };
        self.tray_available = available;
        if !available && !self.tray_note_shown {
            self.tray_note_shown = true;
            self.tray_note = Some(gpui_kit::SharedString::from(
                strings::UI_TRAY_UNAVAILABLE_NOTE,
            ));
        }
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
    #[cfg(test)]
    pub(crate) fn seed_state(&mut self, instances: Vec<InstanceView>) {
        self.state = ShellState {
            instances,
            downloads: Vec::new(),
            last_failure: None,
            uris: std::collections::HashMap::new(),
        };
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::sync::Arc;

    use dblocal_core::model::InstanceStatus;
    use dblocal_core::service::DblService;
    use gpui_kit::TestAppContext;

    use super::super::test_service::QuietService;
    use super::Workspace;

    /// A workspace on the emit-free service.
    ///
    /// `new_preview(MockService::demo())` would start the real bridge thread,
    /// which wakes gpui's deterministic test scheduler from off-thread and
    /// fails the test as non-deterministic. QuietService never emits.
    fn quiet(
        cx: &mut TestAppContext,
    ) -> (
        gpui_kit::Entity<Workspace>,
        QuietService,
        &mut gpui_kit::VisualTestContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(crate::theme::init);
        let service = QuietService::three();
        let svc: Arc<dyn DblService> = Arc::new(service.clone());
        let (view, cx) = cx.add_window_view(|window, cx| Workspace::new_preview(svc, window, cx));
        cx.run_until_parked();
        (view, service, cx)
    }

    /// A background refresh really replaces both collections from the service.
    #[gpui_kit::test]
    fn background_refresh_applies_instances_and_runtimes(cx: &mut TestAppContext) {
        let (view, service, cx) = quiet(cx);
        let planted = dblocal_core::model::Runtime::new(
            "planted-runtime",
            dblocal_core::model::EngineKey::Postgresql,
            "0.0.1",
            dblocal_core::platform::Arch::X86_64,
            std::path::PathBuf::from("/nonexistent/planted"),
            0,
            std::time::SystemTime::UNIX_EPOCH,
            dblocal_core::model::RuntimeState::Installed,
        );
        view.update(cx, |this, _| this.runtimes = vec![planted]);

        view.update(cx, |this, cx| this.refresh_instances_bg(cx));
        cx.run_until_parked();

        assert!(
            view.read_with(cx, |this, _| this.runtimes.is_empty()),
            "background refresh must replace the planted runtimes with the service's"
        );
        assert_eq!(
            view.read_with(cx, |this, _| this.instances().len()),
            3,
            "background refresh must apply the service's instance list"
        );
        assert!(!view.read_with(cx, |this, _| this.is_pending()));
        assert!(service.calls() > 0, "the service was never asked");
    }

    /// `forget_instance` drops every per-instance cache entry.
    #[gpui_kit::test]
    fn forget_instance_drops_every_cache(cx: &mut TestAppContext) {
        let (view, service, cx) = quiet(cx);
        let id = service.id_with(InstanceStatus::Running);
        view.update(cx, |this, _| {
            this.dotenv_cache
                .insert(id.clone(), dblocal_core::secret::SecretString::new("A=1"));
            this.port_suggestions.insert(id.clone(), 5433);
            this.data_sizes.insert(id.clone(), 4096);
        });
        view.update(cx, |this, _| this.forget_instance(&id));

        let cached = view.read_with(cx, |this, _| {
            (
                this.dotenv_cache.contains_key(&id),
                this.port_suggestions.contains_key(&id),
                this.data_sizes.contains_key(&id),
            )
        });
        assert_eq!(
            cached,
            (false, false, false),
            "forget_instance left a cache"
        );
    }

    /// The offline first frame is the bundled catalog plus the offline banner,
    /// never an empty form (F-CAT-01/02).
    #[test]
    fn offline_catalog_carries_the_banner() {
        let catalog = Workspace::offline_catalog();
        assert_eq!(catalog.status, dblocal_core::model::CatalogStatus::Offline);
        assert_eq!(
            catalog.banner.as_deref(),
            Some(dblocal_core::strings::CATALOG_OFFLINE)
        );
        assert!(catalog.engines.is_empty());
    }

    /// A close event ends the event pump; it must not spin afterwards.
    #[gpui_kit::test]
    fn event_pump_stops_on_closed(cx: &mut TestAppContext) {
        let (view, _service, cx) = quiet(cx);
        let (sender, receiver) = flume::unbounded::<crate::service_bridge::UiEvent>();
        // `new_preview` never starts the pump, so this is the only one.
        view.update(cx, |this, cx| {
            this.events = receiver.clone();
            this.start_event_pump(cx);
        });
        sender
            .send(crate::service_bridge::UiEvent::Closed)
            .expect("send closed");
        cx.run_until_parked();
        assert!(
            receiver.is_disconnected() || receiver.is_empty(),
            "the event pump kept running after Closed"
        );
    }
}
