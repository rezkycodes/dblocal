//! Windows and macOS tray backend on `tray-icon` + `muda` (W2-G).
//!
//! # Why the thread matters
//!
//! `tray_icon::TrayIcon` and every `muda` menu type are `Rc`-based and are only
//! valid on the thread that runs the platform event loop:
//!
//! - **macOS:** `tray_icon` calls `MainThreadMarker::new()` and returns
//!   `Error::NotMainThread` on any other thread; `muda` `expect`s on the main
//!   thread. AppKit's `NSStatusBar` also only exists on the main thread.
//! - **Windows:** the icon is a hidden window (`CreateWindowExW`) whose
//!   `WndProc` receives the shell callbacks, so it must live on the thread that
//!   pumps messages (`GetMessageW`).
//!
//! In both cases GPUI's `Platform::run` calls the `on_finish_launching`
//! callback on exactly that thread and pumps that loop:
//!
//! - `gpui-pre-windows`: `fn run` calls `on_finish_launching()` and then loops
//!   on `GetMessageW` (`src/platform.rs:575`).
//! - `gpui-pre-macos`: `fn run` stores the callback and `did_finish_launching`
//!   invokes it from the AppKit run loop on the main thread
//!   (`src/platform.rs:527`, `:1312`).
//!
//! `dblocal_gui::run()` starts the tray from `AppRuntime::new`, which runs
//! inside that callback, so the tray is created on the right thread. The
//! `TrayBackend` trait is `Send` (the handle lives in a GPUI `Global` and a
//! `bg` closure may move it), so the `Rc` object itself is not stored in the
//! backend: it lives in a thread-local slot on the owning thread and the
//! backend only reaches it when it finds itself on that thread.
//!
//! # Activation path (no timer, no polling)
//!
//! `muda` delivers activations on a process-global crossbeam channel. One
//! forwarding thread does a blocking `recv()` in a loop, maps the menu id to a
//! [`TrayAction`] through a mutex-guarded table the UI thread republishes on
//! each refresh, and sends the action on the app's existing flume channel. There
//! is no timer, no `sleep` and no polling (OPEN-08).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use dblocal_core::service::InstanceView;
use tray_icon::menu::{IconMenuItem, Menu, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

use super::spec::{self, MenuRow, MenuRowKind, TraySpec};
use super::{
    MENU_ICON_SIZE, TRAY_ICON_SIZE, TrayAction, TrayBackend, TrayHandle, TrayMenuModel, TrayStartup,
};

thread_local! {
    /// The live OS tray object, valid only on the thread that created it.
    static UI_TRAY: RefCell<Option<UiTray>> = const { RefCell::new(None) };
}

/// The `Rc`-based tray and its native menu, kept alive on the UI thread.
///
/// `Menu` owns its children internally (`MenuState::children`), so the rows do
/// not need to be held here; dropping the `Menu` drops them.
struct UiTray {
    icon: TrayIcon,
    menu: Menu,
}

impl UiTray {
    fn build(spec: &TraySpec) -> Result<Self, tray_icon::Error> {
        let menu = menu_from_spec(spec);
        let icon = TrayIconBuilder::new()
            .with_id("dblocal")
            .with_tooltip(dblocal_core::strings::APP_NAME)
            .with_menu(Box::new(menu.clone()))
            .with_icon(tray_icon_from_rgba(&spec.icon_rgba, TRAY_ICON_SIZE)?)
            .build()?;
        Ok(Self { icon, menu })
    }

    /// Replaces the menu and aggregate icon. Runs on the UI thread only.
    fn apply(&mut self, spec: &TraySpec) {
        let menu = menu_from_spec(spec);
        self.icon.set_menu(Some(Box::new(menu.clone())));
        match tray_icon_from_rgba(&spec.icon_rgba, TRAY_ICON_SIZE) {
            Ok(icon) => {
                if let Err(err) = self.icon.set_icon(Some(icon)) {
                    tracing::warn!("tray icon update failed: {err}");
                }
            }
            Err(err) => tracing::warn!("tray icon bytes rejected: {err}"),
        }
        self.menu = menu;
    }
}

/// Builds a `muda` menu from the spec. The returned `Menu` owns its rows.
fn menu_from_spec(spec: &TraySpec) -> Menu {
    let menu = Menu::new();
    for row in &spec.rows {
        let appended = if row.kind == MenuRowKind::Separator {
            menu.append(&PredefinedMenuItem::separator())
        } else {
            menu.append(&icon_item(row))
        };
        if let Err(err) = appended {
            tracing::warn!("tray menu append failed: {err}");
        }
    }
    menu
}

/// One clickable row. `MenuItemKind` is only a sum type, not an
/// `IsMenuItem`, so rows are built as the concrete `IconMenuItem`.
fn icon_item(row: &MenuRow) -> IconMenuItem {
    let icon = menu_icon_from_rgba(&row.icon_rgba, MENU_ICON_SIZE);
    IconMenuItem::with_id(row.id.clone(), &row.label, row.enabled, icon, None)
}

/// The tray-icon (notification area) icon, RGBA8.
fn tray_icon_from_rgba(rgba: &[u8], size: u32) -> Result<tray_icon::Icon, tray_icon::Error> {
    tray_icon::Icon::from_rgba(rgba.to_vec(), size, size).map_err(|err| {
        tray_icon::Error::OsError(std::io::Error::new(std::io::ErrorKind::InvalidData, err))
    })
}

/// A menu-row icon, RGBA8. `muda::Icon` is a distinct type from
/// `tray_icon::Icon`; a rejected byte buffer degrades to a row without an icon.
fn menu_icon_from_rgba(rgba: &[u8], size: u32) -> Option<tray_icon::menu::Icon> {
    match tray_icon::menu::Icon::from_rgba(rgba.to_vec(), size, size) {
        Ok(icon) => Some(icon),
        Err(err) => {
            tracing::warn!("tray menu icon bytes rejected: {err}");
            None
        }
    }
}

/// Sends menu activations from muda's channel to the app's action channel.
fn spawn_forwarder(tx: flume::Sender<TrayAction>, map: Arc<Mutex<BTreeMap<String, TrayAction>>>) {
    let events = tray_icon::menu::MenuEvent::receiver().clone();
    let spawned = thread::Builder::new()
        .name("dblocal-tray-menu".to_owned())
        .spawn(move || {
            while let Ok(event) = events.recv() {
                let action = spec::resolve(&lock(&map), event.id.as_ref());
                if let Some(action) = action {
                    let _ = tx.send(action);
                }
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("tray forwarding thread could not start: {err}");
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Backend handle stored by the app; it never holds the `Rc` tray itself.
struct DesktopBackend {
    available: bool,
    ui_thread: ThreadId,
    actions: flume::Receiver<TrayAction>,
    activation: Arc<Mutex<BTreeMap<String, TrayAction>>>,
}

impl TrayBackend for DesktopBackend {
    fn refresh(&mut self, model: &TrayMenuModel, views: &[InstanceView]) -> bool {
        if !self.available {
            return false;
        }
        if thread::current().id() != self.ui_thread {
            // Never touch the `Rc` tray from another thread; the menu is
            // rebuilt on the next UI-thread refresh instead.
            tracing::warn!("tray refresh from a non-UI thread ignored");
            return false;
        }
        let spec = spec::build(model, views);
        *lock(&self.activation) = spec::activation_map(model);
        UI_TRAY.with(|cell| {
            if let Some(tray) = cell.borrow_mut().as_mut() {
                tray.apply(&spec);
            }
        });
        true
    }

    fn is_available(&self) -> bool {
        self.available
    }

    fn subscribe_actions(&self) -> flume::Receiver<TrayAction> {
        self.actions.clone()
    }

    fn shutdown(&mut self) {
        if !self.available {
            return;
        }
        if thread::current().id() != self.ui_thread {
            tracing::warn!("tray shutdown from a non-UI thread ignored");
            return;
        }
        // Dropping the `TrayIcon` calls `Shell_NotifyIconW(NIM_DELETE)` on
        // Windows and `removeStatusItem` on macOS, so the icon leaves the tray
        // instead of becoming a ghost.
        let dropped = UI_TRAY.with(|cell| cell.borrow_mut().take().is_some());
        self.available = false;
        tracing::info!(dropped, "tray released");
    }
}

/// Starts the desktop tray on the calling (event-loop) thread.
///
/// Any failure — no shell tray area, a restricted session, an `NSStatusBar`
/// error — degrades to [`TrayStartup::none`], the F-TRY-01 AC3 fallback. It
/// never panics.
pub fn start() -> TrayStartup {
    let empty = TrayMenuModel::from_views(&[]);
    let initial = spec::build(&empty, &[]);
    let ui = match UiTray::build(&initial) {
        Ok(ui) => ui,
        Err(err) => {
            tracing::warn!("tray unavailable: {err}");
            return TrayStartup::none();
        }
    };
    let (tx, rx) = flume::unbounded();
    let activation = Arc::new(Mutex::new(spec::activation_map(&empty)));
    spawn_forwarder(tx.clone(), activation.clone());
    UI_TRAY.with(|cell| *cell.borrow_mut() = Some(ui));
    let backend = DesktopBackend {
        available: true,
        ui_thread: thread::current().id(),
        actions: rx.clone(),
        activation,
    };
    TrayStartup {
        handle: TrayHandle::new(Box::new(backend)),
        actions: rx,
        sender: tx,
        backend_name: "tray-icon",
    }
}
