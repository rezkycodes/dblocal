//! Linux tray backend: `ksni` over D-Bus (W2-B; unchanged by W2-G).
//!
//! The menu is the shared [`TrayMenuModel`]; this module only translates it to
//! `ksni` items and forwards activations on the app's flume channel.

use super::*;
use ksni::Tray;
use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem};
use std::sync::{Arc, Mutex};

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
            width: TRAY_ICON_SIZE as i32,
            height: TRAY_ICON_SIZE as i32,
            data: status_dot_argb(status, TRAY_ICON_SIZE),
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
            let icon_data = super::entry_icon_data(entry, MENU_ICON_SIZE);
            items.push(MenuItem::Standard(StandardItem {
                label,
                enabled,
                icon_data,
                activate: Box::new(move |_| {
                    if let Err(err) = tx.send(action.clone()) {
                        tracing::warn!("tray action dropped (channel closed): {err}");
                    }
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
                if let Err(err) = tx_open.send(open_action.clone()) {
                    tracing::warn!("tray open action dropped (channel closed): {err}");
                }
            }),
            ..Default::default()
        }));
        let quit_action = model.quit.action.clone();
        let tx_quit = self.tx.clone();
        items.push(MenuItem::Standard(StandardItem {
            label: model.quit.label,
            activate: Box::new(move |_| {
                if let Err(err) = tx_quit.send(quit_action.clone()) {
                    tracing::warn!("tray quit action dropped (channel closed): {err}");
                }
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
    fn shutdown(&mut self) {
        if !self.available {
            return;
        }
        let _ = self.handle.shutdown();
        self.available = false;
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
            TrayStartup::none()
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
        assert_eq!(
            (pixmap[0].width, pixmap[0].height),
            (TRAY_ICON_SIZE as i32, TRAY_ICON_SIZE as i32)
        );
        let center = (11 * TRAY_ICON_SIZE + 11) as usize * 4;
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
        let center = (11 * TRAY_ICON_SIZE + 11) as usize * 4;
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
