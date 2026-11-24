//! Tests for the preview constructors (W4-I: split out of `preview.rs`, which the
//! new palette/log previews pushed past the 800-line limit).

use std::sync::Arc;

use dblocal_core::model::{
    AuthMode, EngineKey, InstanceId, InstanceName, InstanceSpec, InstanceStatus, Port,
};
use dblocal_core::service::{DblService, mock::MockService};

use crate::logs::LogStore;

fn seeded() -> Arc<dyn DblService> {
    let service = MockService::demo();
    Arc::new(service)
}

#[test]
fn the_demo_seeds_a_running_and_a_failed_instance() {
    let service = seeded();
    let instances = service.list_instances().expect("list");
    assert!(
        instances
            .iter()
            .any(|view| view.status == InstanceStatus::Running)
    );
    assert!(
        instances
            .iter()
            .any(|view| view.status == InstanceStatus::Failed)
    );
}

#[test]
fn a_deleted_instance_leaves_the_dock_and_its_tab() {
    let service = seeded();
    let instances = service.list_instances().expect("list");
    let target = instances
        .iter()
        .find(|view| view.name == "pg-legacy")
        .expect("seeded instance")
        .clone();

    let mut state = crate::app::ShellState {
        instances: instances.clone(),
        downloads: Vec::new(),
        last_failure: None,
        uris: std::collections::HashMap::new(),
    };
    let mut tabs = crate::app::Tabs::default();
    let mut logs = LogStore::new();
    tabs.open_or_select(target.id.clone());
    logs.load(&target.id, vec!["line".to_owned()]);

    let before: Vec<String> = crate::format::group_by_engine(&state.instances)
        .into_iter()
        .flat_map(|group| group.instances.into_iter().map(|v| v.name.clone()))
        .collect();
    assert!(before.contains(&target.name), "instance starts in the dock");

    state.apply_removed(&target.id, &mut tabs, &mut logs);

    let after: Vec<String> = crate::format::group_by_engine(&state.instances)
        .into_iter()
        .flat_map(|group| group.instances.into_iter().map(|v| v.name.clone()))
        .collect();
    assert!(
        !after.contains(&target.name),
        "deleted instance still in the dock: {after:?}"
    );
    assert_eq!(after.len(), before.len() - 1);
    assert!(tabs.open().iter().all(|tab| tab.instance != target.id));
    assert!(logs.get(&target.id).is_none());
}

#[test]
fn deleting_the_selected_instance_keeps_the_other_tabs() {
    let service = seeded();
    let instances = service.list_instances().expect("list");
    assert!(instances.len() >= 2);
    let mut state = crate::app::ShellState {
        instances,
        downloads: Vec::new(),
        last_failure: None,
        uris: std::collections::HashMap::new(),
    };
    let mut tabs = crate::app::Tabs::default();
    let mut logs = LogStore::new();
    let first = state.instances[0].id.clone();
    let second = state.instances[1].id.clone();
    tabs.open_or_select(first.clone());
    tabs.open_or_select(second.clone());

    state.apply_removed(&second, &mut tabs, &mut logs);

    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs.open()[0].instance, first);
}

#[test]
fn the_demo_dock_groups_every_engine_but_valkey_present() {
    let service = seeded();
    let instances = service.list_instances().expect("list");
    let groups = crate::format::group_by_engine(&instances);
    assert_eq!(groups.len(), 4, "all four engines have an instance");
    let total: usize = groups.iter().map(|group| group.instances.len()).sum();
    assert_eq!(total, 6, "the prototype cast has six instances");
}

#[test]
fn the_download_preview_picks_a_version_that_is_not_installed() {
    // Defect 7: the default PostgreSQL version in the demo (17.4) is
    // installed, so the create render never showed a download. The preview
    // selects 16.6 instead and the footer must promise a download.
    let service = seeded();
    let catalog = service.catalog().expect("catalog");
    let entry = catalog
        .engines
        .iter()
        .find(|entry| entry.engine == EngineKey::Postgresql)
        .expect("postgresql entry");
    let selected = entry
        .versions
        .iter()
        .find(|version| version.version == "16.6")
        .expect("16.6 in the demo catalog");
    assert!(!selected.installed, "16.6 must need a download");
    assert_eq!(
        crate::format::create_footer_note(selected.installed),
        dblocal_core::strings::UI_CREATE_DOWNLOAD_NOTE
    );
}

#[test]
fn the_demo_env_block_never_shows_a_demo_password() {
    // Reproduces the coordinator's round-2 finding on `overview-dark.png`:
    // the MariaDB `.env` printed `DATABASE_URL=mysql://root:pwd-…@host/`.
    let service = seeded();
    for view in service.list_instances().expect("list") {
        if view.auth != AuthMode::Password {
            continue;
        }
        let uri = service
            .connection_uri(&view.id, dblocal_core::service::UriFormat::Uri)
            .expect("uri");
        let dotenv = service
            .connection_uri(&view.id, dblocal_core::service::UriFormat::DotEnv)
            .expect("dotenv");
        let password = uri
            .expose()
            .split_once("://")
            .and_then(|(_, rest)| rest.split_once('@').map(|(creds, _)| creds.to_owned()))
            .and_then(|creds| creds.split_once(':').map(|(_, p)| p.to_owned()))
            .expect("demo uri carries a password");
        assert!(!password.is_empty());

        let rows = crate::app::masked_env_rows(dotenv.expose(), false, Some(&uri));
        let text: String = rows
            .iter()
            .map(|row| match &row.key {
                Some(key) => format!("{key}={}", row.value),
                None => row.value.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !text.contains(&password),
            "{} leaked its demo password: {text}",
            view.name
        );
        assert!(
            text.contains("••••••"),
            "{} hidden env block has no mask: {text}",
            view.name
        );
    }
}

#[test]
fn an_instance_view_can_be_built_for_any_engine() {
    // Guards the fixture helper the render tests use.
    for engine in EngineKey::ALL {
        let spec = InstanceSpec::new(
            InstanceName::new("probe").expect("name"),
            engine,
            "17.4",
            Some(Port::new(5600).expect("port")),
            AuthMode::Password,
        )
        .expect("spec");
        let instance = dblocal_core::model::Instance::new(
            &spec,
            InstanceId::new(),
            InstanceStatus::Stopped,
            std::time::SystemTime::UNIX_EPOCH,
        );
        let view = dblocal_core::service::InstanceView::from_instance(&instance);
        assert_eq!(view.engine, engine);
    }
}
