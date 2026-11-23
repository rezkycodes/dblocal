//! System-tray model and platform glue (F-TRY-01).
//!
//! The menu is a pure function of [`InstanceView`]s so it can be unit-tested
//! without a tray host. Platform glue lives behind [`TrayBackend`] and only
//! translates menu actions into the same service calls the window uses.
//!
//! Three backends exist behind the one trait, each selected with `cfg`:
//!
//! - Linux: `ksni` over D-Bus (`tray/linux.rs`), unchanged by W2-G.
//! - Windows and macOS: `tray-icon` + `muda` (`tray/desktop.rs`, W2-G).
//! - Anything else, or any failed creation: no tray at all (the F-TRY-01 AC3
//!   fallback), so callers must treat `is_available() == false` as normal.
//!
//! The platform-neutral rules that turn an [`InstanceView`] list into native
//! menu rows, status-dot pixels and action ids live in [`spec`] and are
//! exercised on Linux by `cargo test -p dblocal-gui --lib`.

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::InstanceView;
use dblocal_core::strings;

use crate::format;

pub mod spec;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod desktop;

#[cfg(test)]
mod tests;

/// Edge of the aggregate tray icon drawn into the notification area.
pub const TRAY_ICON_SIZE: u32 = 22;

/// Edge of the status dot drawn beside a menu row.
pub const MENU_ICON_SIZE: u32 = 16;

/// An action the tray can ask the app to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    /// Bring the main window to the front.
    OpenWindow,
    /// Quit the app (the quit flow decides what happens to running instances).
    Quit,
    /// Start one instance.
    Start(String),
    /// Stop one instance.
    Stop(String),
}

/// One menu entry the tray should show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayMenuEntry {
    /// Stable id, used to route activation.
    pub id: String,
    /// Visible label, already localised where required.
    pub label: String,
    /// Whether the entry can be activated.
    pub enabled: bool,
    /// Action to perform when activated.
    pub action: TrayAction,
}

/// The full tray menu derived from current instance views.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayMenuModel {
    /// Instance rows, in the same engine-group order the dock uses.
    pub instances: Vec<TrayMenuEntry>,
    /// "Buka DBLocal".
    pub open: TrayMenuEntry,
    /// "Keluar".
    pub quit: TrayMenuEntry,
}

impl TrayMenuModel {
    /// Builds the menu from instance views.
    pub fn from_views(views: &[InstanceView]) -> Self {
        let instances = format::group_by_engine(views)
            .into_iter()
            .flat_map(|group| group.instances.into_iter())
            .map(entry_for_view)
            .collect();
        Self {
            instances,
            open: TrayMenuEntry {
                id: "open".to_owned(),
                label: strings::UI_TRAY_OPEN.to_owned(),
                enabled: true,
                action: TrayAction::OpenWindow,
            },
            quit: TrayMenuEntry {
                id: "quit".to_owned(),
                label: strings::UI_TRAY_QUIT.to_owned(),
                enabled: true,
                action: TrayAction::Quit,
            },
        }
    }

    /// Returns the action for a menu id, if present.
    pub fn action_for(&self, id: &str) -> Option<TrayAction> {
        if self.open.id == id {
            return Some(self.open.action.clone());
        }
        if self.quit.id == id {
            return Some(self.quit.action.clone());
        }
        self.instances
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.action.clone())
    }
}

fn entry_for_view(view: &InstanceView) -> TrayMenuEntry {
    let glyph = format::status_glyph(view.status);
    let verb = action_verb(view.status);
    let enabled = verb.is_some();
    let label = match verb {
        Some(verb) => format!(
            "{} {}  {}  ·  {verb}",
            glyph.as_char(),
            view.name,
            view.status_label()
        ),
        None => format!("{} {}  {}", glyph.as_char(), view.name, view.status_label()),
    };
    let action = match verb {
        Some(strings::UI_START) => TrayAction::Start(view.id.to_string()),
        _ => TrayAction::Stop(view.id.to_string()),
    };
    TrayMenuEntry {
        id: view.id.to_string(),
        label,
        enabled,
        action,
    }
}

/// ARGB32 status-dot icon bytes for SNI menu items (R8/F-TRY-01).
///
/// Network byte order: `[a, r, g, b]` per pixel.
pub fn status_dot_argb(status: InstanceStatus, size: u32) -> Vec<u8> {
    let [r, g, b, a] = status_dot_rgba(status);
    dot_mask(size, [a, r, g, b])
}

/// RGBA8 status-dot icon bytes, the layout `tray-icon`/`muda` take.
///
/// Byte order `[r, g, b, a]` per pixel; the same disc as [`status_dot_argb`].
pub fn status_dot_rgba_bytes(status: InstanceStatus, size: u32) -> Vec<u8> {
    dot_mask(size, status_dot_rgba(status))
}

/// Draws a filled disc of `pixel` on a transparent `size * size` canvas.
fn dot_mask(size: u32, pixel: [u8; 4]) -> Vec<u8> {
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    let center = size as f32 / 2.0;
    let radius = (size as f32 / 2.0) - 1.0;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let inside = dx * dx + dy * dy <= radius * radius;
            if inside {
                data.extend_from_slice(&pixel);
            } else {
                data.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    data
}

/// The RGBA colour of a status dot, `[r, g, b, a]`.
pub(super) fn status_dot_rgba(status: InstanceStatus) -> [u8; 4] {
    match format::status_glyph(status) {
        format::StatusGlyph::Running => [0x59, 0xa8, 0x69, 0xff], // green
        format::StatusGlyph::Stopped => [0x8a, 0x8f, 0x98, 0xff], // grey
        format::StatusGlyph::Failed => [0xe0, 0x6c, 0x75, 0xff],  // red
        format::StatusGlyph::Busy => [0xe5, 0xc0, 0x7b, 0xff],    // amber
    }
}

/// Aggregate tray status: any failed => red, any running/busy => green, else grey.
pub fn aggregate_tray_status(views: &[InstanceView]) -> InstanceStatus {
    if views.iter().any(|v| v.status == InstanceStatus::Failed) {
        return InstanceStatus::Failed;
    }
    if views.iter().any(|v| {
        matches!(
            v.status,
            InstanceStatus::Running
                | InstanceStatus::Starting
                | InstanceStatus::Stopping
                | InstanceStatus::Provisioning
                | InstanceStatus::Initializing
                | InstanceStatus::Deleting
        )
    }) {
        return InstanceStatus::Running;
    }
    InstanceStatus::Stopped
}

/// ARGB32 dot bytes for one menu entry, from the status its label carries (R8).
pub fn entry_icon_data(entry: &TrayMenuEntry, size: u32) -> Vec<u8> {
    status_dot_argb(status_from_tray_label(&entry.label), size)
}

/// Status indicated by a tray menu entry label (R8).
pub fn status_from_tray_label(label: &str) -> InstanceStatus {
    if label.starts_with('●') {
        InstanceStatus::Running
    } else if label.starts_with('▲') {
        InstanceStatus::Failed
    } else if label.starts_with('◐') {
        InstanceStatus::Starting
    } else {
        InstanceStatus::Stopped
    }
}

fn action_verb(status: InstanceStatus) -> Option<&'static str> {
    if format::can_start(status) {
        Some(strings::UI_START)
    } else if format::can_stop(status) {
        Some(strings::UI_STOP)
    } else {
        None
    }
}

/// Status glyph as a plain char for menu text (no colour in SNI menus).
trait GlyphChar {
    fn as_char(&self) -> char;
}

impl GlyphChar for format::StatusGlyph {
    fn as_char(&self) -> char {
        match self {
            Self::Running => '●',
            Self::Stopped => '○',
            Self::Failed => '▲',
            Self::Busy => '◐',
        }
    }
}

trait StatusLabel {
    fn status_label(&self) -> &'static str;
}

impl StatusLabel for InstanceView {
    fn status_label(&self) -> &'static str {
        format::status_label(self.status)
    }
}

/// Platform tray glue. Implementations only forward activations; they never
/// call the service directly.
pub trait TrayBackend: Send {
    /// Refreshes the visible menu. Returns `true` when the tray is live.
    fn refresh(&mut self, model: &TrayMenuModel, views: &[InstanceView]) -> bool;
    /// Whether a tray host is present.
    fn is_available(&self) -> bool;
    /// Subscribe to tray actions. Default: disconnected receiver.
    fn subscribe_actions(&self) -> flume::Receiver<TrayAction> {
        let (_tx, rx) = flume::unbounded();
        rx
    }
    /// Releases the platform tray object on the thread that owns it.
    ///
    /// The default is a no-op: a backend whose handle cleans up when the
    /// process exits needs nothing here.
    fn shutdown(&mut self) {}
}

/// No tray host: every operation is a no-op.
#[derive(Debug, Default)]
pub struct NoTray;

impl TrayBackend for NoTray {
    fn refresh(&mut self, _model: &TrayMenuModel, _views: &[InstanceView]) -> bool {
        false
    }
    fn is_available(&self) -> bool {
        false
    }
}

/// Handle kept by the app to refresh the tray.
pub struct TrayHandle {
    backend: Box<dyn TrayBackend>,
}

impl TrayHandle {
    /// Creates a handle around a backend.
    pub fn new(backend: Box<dyn TrayBackend>) -> Self {
        Self { backend }
    }

    /// Creates a no-op handle.
    pub fn unavailable() -> Self {
        Self::new(Box::new(NoTray))
    }

    /// Refreshes the menu model. Returns whether a tray is present.
    pub fn refresh(&mut self, views: &[InstanceView]) -> bool {
        let model = TrayMenuModel::from_views(views);
        self.backend.refresh(&model, views)
    }

    /// Whether a tray host is present.
    pub fn is_available(&self) -> bool {
        self.backend.is_available()
    }

    /// Subscribe to tray action events.
    pub fn subscribe_actions(&self) -> flume::Receiver<TrayAction> {
        self.backend.subscribe_actions()
    }

    /// Releases the platform tray object (called from the quit path so no
    /// ghost icon is left behind).
    pub fn shutdown(&mut self) {
        self.backend.shutdown();
    }
}

/// Whether the real app asked for a tray. Preview and test construction never
/// does, so a headless render cannot register a StatusNotifierItem.
static TRAY_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Marks this process as a real app run, so `Workspace::new` starts the tray.
pub fn enable_for_app() {
    TRAY_ENABLED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Whether a tray should be started for this construction.
pub fn app_enabled() -> bool {
    TRAY_ENABLED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Outcome of starting the platform tray.
pub struct TrayStartup {
    /// Handle used to refresh the menu.
    pub handle: TrayHandle,
    /// Receiver of user actions from the tray.
    pub actions: flume::Receiver<TrayAction>,
    /// Sending half; the platform menu closures hold a clone.
    pub sender: flume::Sender<TrayAction>,
    /// Human-readable backend name for logs.
    pub backend_name: &'static str,
}

impl TrayStartup {
    /// A startup with no tray host and no actions (the AC3 fallback).
    pub fn none() -> Self {
        let (tx, rx) = flume::unbounded();
        Self {
            handle: TrayHandle::unavailable(),
            actions: rx,
            sender: tx,
            backend_name: "none",
        }
    }
}

/// Starts the best tray backend for this platform.
///
/// Linux uses `ksni` over D-Bus. Windows and macOS use `tray-icon` + `muda`;
/// neither can be run on this Linux host, so those two are compile-checked and
/// listed as unverified in `docs/architecture/gui.md`.
pub fn start_tray() -> TrayStartup {
    #[cfg(target_os = "linux")]
    {
        linux::start()
    }
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        desktop::start()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        TrayStartup::none()
    }
}
