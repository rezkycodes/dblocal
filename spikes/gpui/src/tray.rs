use crate::app::InstanceAction;
use crate::progress::UiEvent;
use flume::Sender;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct TraySnapshot {
    pub tray_available: bool,
    pub backend: String,
    pub note: String,
}

/// Start the system tray if possible. Returns a snapshot describing what happened
/// (success, unavailable, or failed). Tray actions are forwarded to `event_tx` as
/// [`UiEvent::Tray`], so the main-thread loop wakes on real activity (no polling).
///
/// On Linux this uses `ksni` (StatusNotifierItem over D-Bus) which does not
/// require GTK and can coexist with GPUI's event loop. See docs for trade-offs.
pub fn start_tray(event_tx: Sender<UiEvent>) -> TraySnapshot {
    #[cfg(target_os = "linux")]
    {
        start_ksni(event_tx)
    }

    #[cfg(target_os = "macos")]
    {
        let _ = event_tx;
        TraySnapshot {
            tray_available: false,
            backend: "tray-icon (macOS) — not prototyped on Linux host".to_string(),
            note: "macOS target not runnable in this environment; tray-icon 0.26 requires the \
                   tray to be created and its event loop pumped on the main thread inside the \
                   NSApplication run loop (see docs). Marked untested."
                .to_string(),
        }
    }

    #[cfg(target_os = "windows")]
    {
        let _ = event_tx;
        TraySnapshot {
            tray_available: false,
            backend: "tray-icon (Windows) — not prototyped on Linux host".to_string(),
            note: "Windows target not runnable in this environment; tray-icon 0.26 requires a \
                   Win32 message loop on the creating thread and integrates with winit/muda event \
                   loops (see docs). Marked untested."
                .to_string(),
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = event_tx;
        TraySnapshot {
            tray_available: false,
            backend: "none".to_string(),
            note: "Unsupported platform for tray prototype.".to_string(),
        }
    }
}

#[cfg(target_os = "linux")]
fn start_ksni(event_tx: Sender<UiEvent>) -> TraySnapshot {
    use ksni::blocking::TrayMethods;

    // Quick probe: a StatusNotifierWatcher on the session bus is the host that
    // actually displays items (GNOME needs the AppIndicator extension to provide it).
    let watcher_present = dbus_status_notifier_watcher_present();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".to_string());
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());

    let tray_impl = LinuxTray::new(event_tx);
    // Run tray service on its own thread; ksni blocking API owns its event loop.
    match tray_impl.assume_sni_available(false).spawn() {
        Ok(handle) => {
            // Keep handle alive for the process lifetime in a detached thread so the
            // tray stays registered even though we don't hold it in the caller.
            // A failure to spawn the park thread only means the handle is
            // dropped here, which unregisters the tray. Log it and continue:
            // the window fallback still leaves the app fully usable.
            if let Err(err) = std::thread::Builder::new()
                .name("ksni-handle".to_string())
                .spawn(move || {
                    // Park forever; dropping would unregister the tray.
                    let _handle = handle;
                    loop {
                        std::thread::park();
                    }
                })
            {
                tracing::warn!("failed to park ksni handle thread: {err}");
            }

            let mut note = format!(
                "Linux tray registered via ksni (StatusNotifierItem/D-Bus). \
                 Desktop={desktop}, session={session}. StatusNotifierWatcher present={watcher_present}."
            );
            if desktop.to_uppercase().contains("GNOME") {
                note.push_str(
                    " GNOME Shell does not show SNI items without an AppIndicator/KStatusNotifier \
                     extension (e.g. appindicatorsupport@rgcjonas.gmail.com).",
                );
            }
            TraySnapshot {
                tray_available: true,
                backend: "ksni 0.3 (StatusNotifierItem, no GTK)".to_string(),
                note,
            }
        }
        Err(err) => {
            let note = format!(
                "Tray unavailable (ksni): {err}. Desktop={desktop}, session={session}, \
                 StatusNotifierWatcher present={watcher_present}. \
                 On GNOME install/enable an AppIndicator extension; the app remains usable via \
                 the window (F-TRY-01 AC3)."
            );
            TraySnapshot {
                tray_available: false,
                backend: "ksni 0.3 (attempted, failed)".to_string(),
                note,
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn dbus_status_notifier_watcher_present() -> bool {
    std::process::Command::new("gdbus")
        .args([
            "introspect",
            "--session",
            "--dest",
            "org.kde.StatusNotifierWatcher",
            "--object-path",
            "/StatusNotifierWatcher",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct LinuxTray {
    event_tx: Sender<UiEvent>,
    quitting: Arc<AtomicBool>,
    _state: Arc<Mutex<()>>,
}

#[cfg(target_os = "linux")]
impl LinuxTray {
    fn new(event_tx: Sender<UiEvent>) -> Self {
        Self {
            event_tx,
            quitting: Arc::new(AtomicBool::new(false)),
            _state: Arc::new(Mutex::new(())),
        }
    }
}

#[cfg(target_os = "linux")]
impl ksni::Tray for LinuxTray {
    fn id(&self) -> String {
        "dblocal-gpui-spike".to_string()
    }

    fn title(&self) -> String {
        "DBLocal (spike)".to_string()
    }

    fn icon_name(&self) -> String {
        "utilities-terminal".to_string()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "DBLocal GPUI spike".to_string(),
            description: "Tray prototype (ksni/SNI)".to_string(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let make_toggle = |idx: usize, label: &'static str| -> ksni::MenuItem<Self> {
            StandardItem {
                label: format!("{label}: Start/Stop"),
                activate: Box::new(move |this: &mut Self| {
                    let _ = this.event_tx.send(UiEvent::Tray(InstanceAction::Toggle {
                        idx,
                        at: Instant::now(),
                        source: "tray",
                    }));
                }),
                ..Default::default()
            }
            .into()
        };

        vec![
            StandardItem {
                label: "DBLocal (spike)".to_string(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            make_toggle(0, "postgres-main"),
            make_toggle(1, "mysql-shop"),
            make_toggle(2, "valkey-cache"),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".to_string(),
                icon_name: "application-exit".to_string(),
                activate: Box::new(|this: &mut Self| {
                    this.quitting.store(true, Ordering::SeqCst);
                    let _ = this.event_tx.send(UiEvent::Tray(InstanceAction::Quit));
                    let _ = std::io::stdout().flush();
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_records_platform_backend_descriptions() {
        let (tx, _rx) = flume::unbounded();
        let snapshot = start_tray(tx);
        assert!(!snapshot.backend.is_empty());
        assert!(!snapshot.note.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_backend_name_is_ksni_or_explicit_failure() {
        let (tx, _rx) = flume::unbounded();
        let snapshot = start_tray(tx);
        assert!(
            snapshot.backend.contains("ksni"),
            "unexpected backend string: {}",
            snapshot.backend
        );
        // Either it registered, or the note explains why it could not.
        if !snapshot.tray_available {
            assert!(snapshot.note.contains("Tray unavailable"));
            assert!(snapshot.note.contains("window"));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_tray_menu_lists_instances_quit_and_uses_channel() {
        let (tx, rx) = flume::unbounded();
        let mut tray = LinuxTray::new(tx);
        let menu = ksni::Tray::menu(&tray);
        assert_eq!(
            menu.len(),
            7,
            "header + separator + 3 toggles + separator + quit"
        );

        // Labels are the contract the tray host renders.
        let labels: Vec<String> = menu
            .iter()
            .filter_map(|item| match item {
                ksni::MenuItem::Standard(standard) => Some(standard.label.clone()),
                _ => None,
            })
            .collect();
        assert!(labels.iter().any(|l| l.starts_with("postgres-main")));
        assert!(labels.iter().any(|l| l.starts_with("mysql-shop")));
        assert!(labels.iter().any(|l| l.starts_with("valkey-cache")));
        assert!(labels.iter().any(|l| l == "Quit"));

        // Activating an instance item must cross the channel as a Tray event.
        for item in &menu {
            if let ksni::MenuItem::Standard(standard) = item
                && standard.label.starts_with("postgres-main")
            {
                (standard.activate)(&mut tray);
            }
        }
        match rx.try_recv() {
            Ok(UiEvent::Tray(InstanceAction::Toggle { idx, source, .. })) => {
                assert_eq!(idx, 0);
                assert_eq!(source, "tray");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn quit_item_records_quitting_flag_and_sends_quit() {
        let (tx, rx) = flume::unbounded();
        let mut tray = LinuxTray::new(tx);
        let menu = ksni::Tray::menu(&tray);
        for item in &menu {
            if let ksni::MenuItem::Standard(standard) = item
                && standard.label == "Quit"
            {
                (standard.activate)(&mut tray);
            }
        }
        assert!(tray.quitting.load(Ordering::SeqCst));
        assert!(matches!(
            rx.try_recv(),
            Ok(UiEvent::Tray(InstanceAction::Quit))
        ));
    }
}
