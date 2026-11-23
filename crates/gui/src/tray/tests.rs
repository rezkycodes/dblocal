//! Pure tray tests, run on every platform.
//!
//! The Windows/macOS natives cannot run here, so the platform-neutral pieces
//! they depend on — the menu spec, the id → action table, the icon bytes and
//! the failure path of a backend that cannot create a tray — are tested here on
//! Linux. `desktop.rs` is a thin adapter over these functions.

use super::spec;
use super::*;
use dblocal_core::model::{
    AuthMode, EngineKey, InstanceId, InstanceName, InstanceSpec, InstanceStatus, Port,
};
use std::sync::atomic::{AtomicBool, Ordering};
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
    handle.shutdown();
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

// --- W2-G: the platform-neutral spec the Windows/macOS backend consumes ------

#[test]
fn rgba_dot_is_the_argb_dot_with_the_channels_rotated() {
    for status in [
        InstanceStatus::Running,
        InstanceStatus::Stopped,
        InstanceStatus::Failed,
        InstanceStatus::Starting,
    ] {
        let argb = status_dot_argb(status, MENU_ICON_SIZE);
        let rgba = status_dot_rgba_bytes(status, MENU_ICON_SIZE);
        assert_eq!(argb.len(), rgba.len());
        let center = (MENU_ICON_SIZE / 2 * MENU_ICON_SIZE + MENU_ICON_SIZE / 2) as usize * 4;
        // ARGB [a, r, g, b] -> RGBA [r, g, b, a]
        assert_eq!(
            &rgba[center..center + 4],
            &[
                argb[center + 1],
                argb[center + 2],
                argb[center + 3],
                argb[center]
            ],
            "{status:?} dot channels rotated wrong"
        );
        // The disc is opaque only at the centre and transparent at the corner.
        assert_eq!(&rgba[0..4], &[0, 0, 0, 0]);
        assert_eq!(rgba[center + 3], 0xff);
    }
}

#[test]
fn spec_rows_are_instances_separator_open_quit() {
    let views = vec![
        view("pg-a", EngineKey::Postgresql, InstanceStatus::Running),
        view("my-a", EngineKey::Mysql, InstanceStatus::Stopped),
    ];
    let model = TrayMenuModel::from_views(&views);
    let built = spec::build(&model, &views);
    assert_eq!(built.rows.len(), 5);
    assert_eq!(built.rows[0].kind, spec::MenuRowKind::Item);
    assert!(built.rows[0].label.contains("pg-a"));
    assert!(built.rows[1].label.contains("my-a"));
    assert_eq!(built.rows[2].kind, spec::MenuRowKind::Separator);
    assert!(built.rows[2].id.is_empty());
    assert!(built.rows[2].icon_rgba.is_empty());
    assert_eq!(built.rows[3].id, "open");
    assert_eq!(built.rows[3].label, strings::UI_TRAY_OPEN);
    assert_eq!(built.rows[4].id, "quit");
    assert_eq!(built.rows[4].label, strings::UI_TRAY_QUIT);
}

#[test]
fn spec_omits_the_separator_when_there_are_no_instances() {
    let model = TrayMenuModel::from_views(&[]);
    let built = spec::build(&model, &[]);
    assert_eq!(built.rows.len(), 2);
    assert_eq!(built.rows[0].id, "open");
    assert_eq!(built.rows[1].id, "quit");
    assert!(built.rows.iter().all(|r| r.kind == spec::MenuRowKind::Item));
}

#[test]
fn spec_rows_carry_the_status_dot_and_disabled_state() {
    let views = vec![
        view("pg-run", EngineKey::Postgresql, InstanceStatus::Running),
        view("pg-busy", EngineKey::Postgresql, InstanceStatus::Starting),
    ];
    let model = TrayMenuModel::from_views(&views);
    let built = spec::build(&model, &views);
    let row = |name: &str| {
        built
            .rows
            .iter()
            .find(|r| r.label.contains(name))
            .expect("row")
    };
    let running = row("pg-run");
    assert!(running.enabled);
    assert_eq!(
        running.icon_rgba.len(),
        (MENU_ICON_SIZE * MENU_ICON_SIZE * 4) as usize
    );
    let busy = row("pg-busy");
    assert!(!busy.enabled, "a starting instance must be disabled");
    // The busy dot is amber [0xe5, 0xc0, 0x7b, 0xff] at the centre.
    let center = (MENU_ICON_SIZE / 2 * MENU_ICON_SIZE + MENU_ICON_SIZE / 2) as usize * 4;
    assert_eq!(
        &busy.icon_rgba[center..center + 4],
        &[0xe5, 0xc0, 0x7b, 0xff]
    );
}

#[test]
fn spec_aggregate_icon_follows_the_aggregate_status() {
    let failed = vec![
        view("pg-run", EngineKey::Postgresql, InstanceStatus::Running),
        view("pg-bad", EngineKey::Postgresql, InstanceStatus::Failed),
    ];
    let model = TrayMenuModel::from_views(&failed);
    let built = spec::build(&model, &failed);
    assert_eq!(
        built.icon_rgba.len(),
        (TRAY_ICON_SIZE * TRAY_ICON_SIZE * 4) as usize
    );
    let center = (TRAY_ICON_SIZE / 2 * TRAY_ICON_SIZE + TRAY_ICON_SIZE / 2) as usize * 4;
    assert_eq!(
        &built.icon_rgba[center..center + 4],
        &[0xe0, 0x6c, 0x75, 0xff],
        "any failed instance makes the aggregate icon red"
    );

    let idle = vec![view(
        "pg-off",
        EngineKey::Postgresql,
        InstanceStatus::Stopped,
    )];
    let idle_model = TrayMenuModel::from_views(&idle);
    let idle_built = spec::build(&idle_model, &idle);
    assert_eq!(
        &idle_built.icon_rgba[center..center + 4],
        &[0x8a, 0x8f, 0x98, 0xff],
        "nothing running makes the aggregate icon grey"
    );
}

#[test]
fn activation_map_covers_every_row_and_resolves_ids() {
    let views = vec![
        view("pg-a", EngineKey::Postgresql, InstanceStatus::Running),
        view("pg-b", EngineKey::Postgresql, InstanceStatus::Stopped),
    ];
    let model = TrayMenuModel::from_views(&views);
    let map = spec::activation_map(&model);
    assert_eq!(map.len(), 4, "two instances + open + quit");
    assert_eq!(spec::resolve(&map, "open"), Some(TrayAction::OpenWindow));
    assert_eq!(spec::resolve(&map, "quit"), Some(TrayAction::Quit));
    for entry in &model.instances {
        assert_eq!(
            spec::resolve(&map, &entry.id),
            Some(entry.action.clone()),
            "row {} did not resolve to its action",
            entry.id
        );
    }
    assert_eq!(spec::resolve(&map, "does-not-exist"), None);
}

#[test]
fn an_empty_menu_map_still_knows_open_and_quit() {
    let model = TrayMenuModel::from_views(&[]);
    let map = spec::activation_map(&model);
    assert_eq!(map.len(), 2);
    assert_eq!(spec::resolve(&map, "open"), Some(TrayAction::OpenWindow));
    assert_eq!(spec::resolve(&map, "quit"), Some(TrayAction::Quit));
}

/// A backend whose creation failed must answer unavailable, never refresh, and
/// never yield actions — the shape `desktop::start()` returns on an error.
#[test]
fn a_failed_backend_degrades_to_the_ac3_fallback() {
    #[derive(Default)]
    struct FailingBackend {
        refresh_calls: usize,
    }
    impl TrayBackend for FailingBackend {
        fn refresh(&mut self, _model: &TrayMenuModel, _views: &[InstanceView]) -> bool {
            self.refresh_calls += 1;
            false
        }
        fn is_available(&self) -> bool {
            false
        }
    }

    let mut handle = TrayHandle::new(Box::new(FailingBackend::default()));
    assert!(!handle.is_available());
    assert!(!handle.refresh(&[view("pg", EngineKey::Postgresql, InstanceStatus::Running)]));
    assert!(handle.subscribe_actions().try_recv().is_err());
    handle.shutdown();
    assert!(!handle.is_available());
}

/// `shutdown` must be reachable through the trait object and must release a live
/// backend exactly once (the ghost-icon guard).
#[test]
fn shutdown_releases_a_live_backend_once() {
    struct CountingBackend {
        available: AtomicBool,
        shut: std::sync::atomic::AtomicUsize,
    }
    impl TrayBackend for CountingBackend {
        fn refresh(&mut self, _model: &TrayMenuModel, _views: &[InstanceView]) -> bool {
            self.available.load(Ordering::SeqCst)
        }
        fn is_available(&self) -> bool {
            self.available.load(Ordering::SeqCst)
        }
        fn shutdown(&mut self) {
            self.shut.fetch_add(1, Ordering::SeqCst);
            self.available.store(false, Ordering::SeqCst);
        }
    }

    let backend = CountingBackend {
        available: AtomicBool::new(true),
        shut: std::sync::atomic::AtomicUsize::new(0),
    };
    let mut handle = TrayHandle::new(Box::new(backend));
    assert!(handle.is_available());
    handle.shutdown();
    assert!(!handle.is_available(), "shutdown must drop availability");
}
