//! System-tray model and platform glue (F-TRY-01).
//!
//! The menu is a pure function of [`InstanceView`]s so it can be unit-tested
//! without a tray host. Platform glue lives behind [`TrayBackend`] and only
//! translates menu actions into the same service calls the window uses.

use std::sync::Arc;

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::InstanceView;
use dblocal_core::strings;

use crate::format;

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
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    let center = size as f32 / 2.0;
    let radius = (size as f32 / 2.0) - 1.0;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let inside = dx * dx + dy * dy <= radius * radius;
            if inside {
                data.extend_from_slice(&[a, r, g, b]);
            } else {
                data.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    data
}

fn status_dot_rgba(status: InstanceStatus) -> [u8; 4] {
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
    /// A startup with no tray host and no actions.
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
/// Linux uses `ksni` over D-Bus. macOS/Windows are compile-gated: if the
/// target toolchain exists the code compiles, but this Linux host cannot run
/// them; see `docs/architecture/gui.md`.
pub fn start_tray() -> TrayStartup {
    #[cfg(target_os = "linux")]
    {
        linux::start()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let (tx, rx) = flume::unbounded();
        TrayStartup {
            handle: TrayHandle::unavailable(),
            actions: rx,
            sender: tx,
            backend_name: "none",
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use ksni::Tray;
    use ksni::blocking::TrayMethods;
    use ksni::menu::{MenuItem, StandardItem};
    use std::sync::Mutex;

    struct DblocalTray {
        model: Arc<Mutex<TrayMenuModel>>,
        views: Arc<Mutex<Vec<InstanceView>>>,
        tx: flume::Sender<TrayAction>,
    }

    impl DblocalTray {
        fn model_views(&self, _model: &TrayMenuModel) -> Vec<InstanceView> {
            self.views.lock().unwrap_or_else(|p| p.into_inner()).clone()
        }
    }

    impl Tray for DblocalTray {
        fn id(&self) -> String {
            "dblocal".to_owned()
        }
        fn title(&self) -> String {
            strings::APP_NAME.to_owned()
        }
        fn icon_name(&self) -> String {
            String::new()
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            let model = self.model.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let status = aggregate_tray_status(&self.model_views(&model));
            vec![ksni::Icon {
                width: 22,
                height: 22,
                data: status_dot_argb(status, 22),
            }]
        }
        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: strings::APP_NAME.to_owned(),
                description: String::new(),
                ..Default::default()
            }
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            let model = self.model.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let mut items: Vec<MenuItem<Self>> = Vec::new();
            for entry in &model.instances {
                let action = entry.action.clone();
                let enabled = entry.enabled;
                let label = entry.label.clone();
                let tx = self.tx.clone();
                let icon_data = super::entry_icon_data(entry, 16);
                items.push(MenuItem::Standard(StandardItem {
                    label,
                    enabled,
                    icon_data,
                    activate: Box::new(move |_| {
                        let _ = tx.send(action.clone());
                    }),
                    ..Default::default()
                }));
            }
            if !model.instances.is_empty() {
                items.push(MenuItem::Separator);
            }
            let open_action = model.open.action.clone();
            let tx_open = self.tx.clone();
            items.push(MenuItem::Standard(StandardItem {
                label: model.open.label,
                activate: Box::new(move |_| {
                    let _ = tx_open.send(open_action.clone());
                }),
                ..Default::default()
            }));
            let quit_action = model.quit.action.clone();
            let tx_quit = self.tx.clone();
            items.push(MenuItem::Standard(StandardItem {
                label: model.quit.label,
                activate: Box::new(move |_| {
                    let _ = tx_quit.send(quit_action.clone());
                }),
                ..Default::default()
            }));
            items
        }
    }

    struct KsniBackend {
        handle: ksni::blocking::Handle<DblocalTray>,
        available: bool,
        actions: flume::Receiver<TrayAction>,
    }

    impl TrayBackend for KsniBackend {
        fn refresh(&mut self, model: &TrayMenuModel, views: &[InstanceView]) -> bool {
            if !self.available {
                return false;
            }
            let _ = self.handle.update(|tray| {
                if let Ok(mut m) = tray.model.lock() {
                    *m = model.clone();
                }
                if let Ok(mut v) = tray.views.lock() {
                    *v = views.to_vec();
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
    }

    pub fn start() -> TrayStartup {
        let (tx, rx) = flume::unbounded();
        let model = Arc::new(Mutex::new(TrayMenuModel::from_views(&[])));
        let views = Arc::new(Mutex::new(Vec::new()));
        let tray = DblocalTray {
            model,
            views,
            tx: tx.clone(),
        };
        match tray.assume_sni_available(false).spawn() {
            Ok(handle) => TrayStartup {
                handle: TrayHandle::new(Box::new(KsniBackend {
                    handle,
                    available: true,
                    actions: rx.clone(),
                })),
                actions: rx,
                sender: tx,
                backend_name: "ksni",
            },
            Err(err) => {
                tracing::warn!("tray unavailable: {err}");
                TrayStartup {
                    handle: TrayHandle::unavailable(),
                    actions: rx,
                    sender: tx,
                    backend_name: "none",
                }
            }
        }
    }

    #[cfg(test)]
    mod glue_tests {
        #![allow(clippy::expect_used, clippy::unwrap_used)]

        use super::*;

        fn tray_with(views: Vec<InstanceView>) -> (DblocalTray, flume::Receiver<TrayAction>) {
            let (tx, rx) = flume::unbounded();
            let tray = DblocalTray {
                model: Arc::new(Mutex::new(TrayMenuModel::from_views(&views))),
                views: Arc::new(Mutex::new(views)),
                tx,
            };
            (tray, rx)
        }

        fn sample(status: InstanceStatus) -> InstanceView {
            use dblocal_core::model::{
                AuthMode, EngineKey, InstanceId, InstanceName, InstanceSpec, Port,
            };
            let spec = InstanceSpec::new(
                InstanceName::new("pg-glue").expect("name"),
                EngineKey::Postgresql,
                "17.4",
                Some(Port::new(5432).expect("port")),
                AuthMode::Password,
            )
            .expect("spec");
            let instance = dblocal_core::model::Instance::new(
                &spec,
                InstanceId::new(),
                status,
                std::time::SystemTime::UNIX_EPOCH,
            );
            InstanceView::from_instance(&instance)
        }

        /// The D-Bus-facing accessors answer from the model, with an empty
        /// `icon_name` so the host prefers the pixmap (R8).
        #[test]
        fn the_glue_reports_title_icon_and_tooltip() {
            let (tray, _rx) = tray_with(vec![sample(InstanceStatus::Running)]);
            assert_eq!(tray.id(), "dblocal");
            assert_eq!(tray.title(), strings::APP_NAME);
            assert_eq!(tray.icon_name(), "");
            let pixmap = tray.icon_pixmap();
            assert_eq!(pixmap.len(), 1);
            assert_eq!((pixmap[0].width, pixmap[0].height), (22, 22));
            let center = (11 * 22 + 11) * 4;
            assert_eq!(
                &pixmap[0].data[center..center + 4],
                &[0xff, 0x59, 0xa8, 0x69],
                "a running aggregate must be the green dot"
            );
            assert_eq!(tray.tool_tip().title, strings::APP_NAME);
        }

        /// The aggregate icon is grey with nothing running and red with a failure.
        #[test]
        fn the_glue_icon_follows_the_aggregate() {
            let (idle, _) = tray_with(vec![sample(InstanceStatus::Stopped)]);
            let grey = idle.icon_pixmap();
            let center = (11 * 22 + 11) * 4;
            assert_eq!(&grey[0].data[center..center + 4], &[0xff, 0x8a, 0x8f, 0x98]);
            let (failing, _) = tray_with(vec![
                sample(InstanceStatus::Running),
                sample(InstanceStatus::Failed),
            ]);
            let red = failing.icon_pixmap();
            assert_eq!(&red[0].data[center..center + 4], &[0xff, 0xe0, 0x6c, 0x75]);
        }

        /// Every menu entry carries icon data, and activating one sends its action.
        #[test]
        fn the_glue_menu_carries_icons_and_actions() {
            let (tray, rx) = tray_with(vec![sample(InstanceStatus::Running)]);
            let menu = tray.menu();
            // instance + separator + open + quit
            assert_eq!(menu.len(), 4);
            let mut saw_icon = false;
            for item in &menu {
                if let MenuItem::Standard(standard) = item {
                    assert!(!standard.label.is_empty());
                    if standard.label.contains("pg-glue") {
                        assert!(!standard.icon_data.is_empty(), "entry has no icon");
                        saw_icon = true;
                    }
                }
            }
            assert!(saw_icon, "the instance entry was missing");
            // Activate the instance row: the action reaches the channel.
            let (mut target, _rx_keep) = tray_with(vec![sample(InstanceStatus::Running)]);
            for item in menu {
                if let MenuItem::Standard(standard) = item
                    && standard.label.contains("pg-glue")
                {
                    (standard.activate)(&mut target);
                }
            }
            let action = rx.try_recv().expect("an activation sends an action");
            assert!(matches!(action, TrayAction::Stop(_) | TrayAction::Start(_)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dblocal_core::model::{
        AuthMode, EngineKey, InstanceId, InstanceName, InstanceSpec, InstanceStatus, Port,
    };
    use std::time::SystemTime;

    fn view(name: &str, engine: EngineKey, status: InstanceStatus) -> InstanceView {
        let spec = InstanceSpec::new(
            InstanceName::new(name).expect("name"),
            engine,
            "17.4",
            Some(Port::new(engine.canonical_port()).expect("port")),
            AuthMode::Password,
        )
        .expect("spec");
        let instance = dblocal_core::model::Instance::new(
            &spec,
            InstanceId::new(),
            status,
            SystemTime::UNIX_EPOCH,
        );
        InstanceView::from_instance(&instance)
    }

    #[test]
    fn menu_groups_by_engine_and_has_open_and_quit() {
        let views = vec![
            view("pg-a", EngineKey::Postgresql, InstanceStatus::Running),
            view("my-a", EngineKey::Mysql, InstanceStatus::Stopped),
            view("pg-b", EngineKey::Postgresql, InstanceStatus::Failed),
        ];
        let model = TrayMenuModel::from_views(&views);
        assert_eq!(model.instances.len(), 3);
        // grouped by engine order: postgresql first
        assert!(model.instances[0].label.contains("pg-a"));
        assert!(model.instances[1].label.contains("pg-b"));
        assert!(model.instances[2].label.contains("my-a"));
        assert_eq!(model.open.label, strings::UI_TRAY_OPEN);
        assert_eq!(model.quit.label, strings::UI_TRAY_QUIT);
    }

    #[test]
    fn menu_marks_busy_instances_disabled() {
        let views = vec![
            view("starting", EngineKey::Postgresql, InstanceStatus::Starting),
            view("stopping", EngineKey::Postgresql, InstanceStatus::Stopping),
            view("running", EngineKey::Postgresql, InstanceStatus::Running),
        ];
        let model = TrayMenuModel::from_views(&views);
        // grouped by engine, sorted by name: running, starting, stopping
        assert_eq!(model.instances.len(), 3);
        assert!(model.instances[0].label.contains("running"));
        assert!(model.instances[0].enabled);
        assert!(model.instances[0].label.contains(strings::UI_STOP));
        assert!(model.instances[1].label.contains("starting"));
        assert!(!model.instances[1].enabled);
        assert!(model.instances[1].label.contains('◐'));
        assert!(model.instances[2].label.contains("stopping"));
        assert!(!model.instances[2].enabled);
    }

    #[test]
    fn action_for_resolves_ids() {
        let views = vec![view("pg-a", EngineKey::Postgresql, InstanceStatus::Stopped)];
        let model = TrayMenuModel::from_views(&views);
        let id = model.instances[0].id.clone();
        assert!(matches!(model.action_for(&id), Some(TrayAction::Start(_))));
        assert_eq!(model.action_for("open"), Some(TrayAction::OpenWindow));
        assert_eq!(model.action_for("quit"), Some(TrayAction::Quit));
        assert_eq!(model.action_for("missing"), None);
    }

    #[test]
    fn no_tray_is_unavailable() {
        let mut handle = TrayHandle::unavailable();
        assert!(!handle.is_available());
        assert!(!handle.refresh(&[]));
    }

    #[test]
    fn status_dot_argb_has_correct_size_and_transparency() {
        let data = status_dot_argb(InstanceStatus::Running, 16);
        assert_eq!(data.len(), 16 * 16 * 4);
        // corner pixel should be transparent
        assert_eq!(&data[0..4], &[0, 0, 0, 0]);
        // center pixel should be green-ish (ARGB)
        let center = (8 * 16 + 8) * 4;
        let px = &data[center..center + 4];
        assert_eq!(px[0], 0xff); // alpha
        assert_eq!(px[1], 0x59); // red
        assert_eq!(px[2], 0xa8); // green
        assert_eq!(px[3], 0x69); // blue
    }

    #[test]
    fn aggregate_tray_status_prefers_failed_then_running_then_stopped() {
        let running = view("pg-a", EngineKey::Postgresql, InstanceStatus::Running);
        let failed = view("pg-b", EngineKey::Postgresql, InstanceStatus::Failed);
        let stopped = view("pg-c", EngineKey::Postgresql, InstanceStatus::Stopped);
        assert_eq!(aggregate_tray_status(&[]), InstanceStatus::Stopped);
        assert_eq!(
            aggregate_tray_status(&[stopped.clone(), running.clone()]),
            InstanceStatus::Running
        );
        assert_eq!(
            aggregate_tray_status(&[running.clone(), failed.clone()]),
            InstanceStatus::Failed
        );
        assert_eq!(aggregate_tray_status(&[stopped]), InstanceStatus::Stopped);
    }

    #[test]
    fn every_instance_entry_carries_its_own_status_dot() {
        let views = vec![
            view("pg-run", EngineKey::Postgresql, InstanceStatus::Running),
            view("pg-bad", EngineKey::Postgresql, InstanceStatus::Failed),
            view("pg-off", EngineKey::Postgresql, InstanceStatus::Stopped),
        ];
        let model = TrayMenuModel::from_views(&views);
        for entry in &model.instances {
            let bytes = entry_icon_data(entry, 16);
            assert_eq!(bytes.len(), 16 * 16 * 4, "{} has no icon data", entry.id);
            let center = (8 * 16 + 8) * 4;
            assert_eq!(bytes[center], 0xff, "{} alpha must be first", entry.id);
        }
        let status = |name: &str| {
            let entry = model
                .instances
                .iter()
                .find(|e| e.label.contains(name))
                .expect("entry");
            status_from_tray_label(&entry.label)
        };
        assert_eq!(status("pg-run"), InstanceStatus::Running);
        assert_eq!(status("pg-bad"), InstanceStatus::Failed);
        assert_eq!(status("pg-off"), InstanceStatus::Stopped);
    }

    #[test]
    fn busy_status_draws_the_amber_dot() {
        let bytes = status_dot_argb(InstanceStatus::Starting, 8);
        let center = (4 * 8 + 4) * 4;
        assert_eq!(&bytes[center..center + 4], &[0xff, 0xe5, 0xc0, 0x7b]);
        assert_eq!(
            status_from_tray_label("◐ pg running"),
            InstanceStatus::Starting
        );
        assert_eq!(
            status_from_tray_label("○ pg stopped"),
            InstanceStatus::Stopped
        );
    }

    #[test]
    fn the_app_flag_and_the_empty_startup_are_wired() {
        // The flag is process-wide; set it and read it back through both paths.
        enable_for_app();
        assert!(app_enabled());
        let mut startup = TrayStartup::none();
        assert!(!startup.handle.is_available());
        assert_eq!(startup.backend_name, "none");
        assert!(!startup.handle.refresh(&[]));
        // A handle with no backend never yields tray actions.
        assert!(startup.handle.subscribe_actions().try_recv().is_err());
    }

    #[test]
    fn tray_label_status_maps_to_icon_bytes() {
        let views = vec![
            view("pg-a", EngineKey::Postgresql, InstanceStatus::Running),
            view("pg-b", EngineKey::Postgresql, InstanceStatus::Failed),
            view("pg-c", EngineKey::Postgresql, InstanceStatus::Stopped),
        ];
        let model = TrayMenuModel::from_views(&views);
        for entry in &model.instances {
            let status = status_from_tray_label(&entry.label);
            let bytes = status_dot_argb(status, 8);
            let center = (4 * 8 + 4) * 4;
            assert_eq!(bytes[center], 0xff);
        }
        let a = model
            .instances
            .iter()
            .find(|e| e.label.contains("pg-a"))
            .unwrap();
        assert_eq!(status_from_tray_label(&a.label), InstanceStatus::Running);
        let b = model
            .instances
            .iter()
            .find(|e| e.label.contains("pg-b"))
            .unwrap();
        assert_eq!(status_from_tray_label(&b.label), InstanceStatus::Failed);
    }
}
