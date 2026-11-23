//! Palette view-model tests, split out of `format/tests.rs` to keep it under
//! the 800-line limit. Included as a child of the `format::tests` module so it
//! shares the `view` helper and imports.

use super::*;

#[test]
fn palette_lists_start_stop_open_and_copy() {
    let running = view("run", EngineKey::Postgresql, InstanceStatus::Running, 5432);
    let stopped = view("stop", EngineKey::Mysql, InstanceStatus::Stopped, 3306);
    let items = palette_items(&[running, stopped]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&strings::UI_PALETTE_NEW));
    // Every instance lists start, stop, open and copy; the ones that do not
    // apply are disabled, not omitted (F-PLH-01 AC2).
    assert!(
        labels
            .iter()
            .any(|l| l.starts_with(strings::UI_PALETTE_START) && l.contains("stop"))
    );
    assert!(
        labels
            .iter()
            .any(|l| l.starts_with(strings::UI_PALETTE_STOP) && l.contains("run"))
    );
    assert!(
        labels
            .iter()
            .any(|l| l.starts_with(strings::UI_PALETTE_OPEN))
    );
    assert!(
        labels
            .iter()
            .any(|l| l.starts_with(strings::UI_PALETTE_COPY_URI))
    );
    assert!(matches!(items[0].action, Some(PaletteAction::NewInstance)));
}

#[test]
fn palette_disables_commands_with_a_reason_when_they_do_not_apply() {
    let running = view("run", EngineKey::Postgresql, InstanceStatus::Running, 5432);
    let stopped = view("stop", EngineKey::Mysql, InstanceStatus::Stopped, 3306);
    let items = palette_items(&[running, stopped]);

    // Start on a RUNNING instance: disabled, with the "already running" reason.
    let start_run = items
        .iter()
        .find(|i| i.label.starts_with(strings::UI_PALETTE_START) && i.label.contains("run"))
        .expect("start row for the running instance");
    assert!(
        !start_run.is_enabled(),
        "start on a running instance is disabled"
    );
    assert_eq!(
        start_run.reason.as_deref(),
        Some(strings::UI_PALETTE_REASON_ALREADY_RUNNING)
    );

    // Stop on a STOPPED instance: disabled, with the "not running" reason.
    let stop_stop = items
        .iter()
        .find(|i| i.label.starts_with(strings::UI_PALETTE_STOP) && i.label.contains("stop"))
        .expect("stop row for the stopped instance");
    assert!(!stop_stop.is_enabled());
    assert_eq!(
        stop_stop.reason.as_deref(),
        Some(strings::UI_PALETTE_REASON_NOT_RUNNING)
    );

    // And the applicable ones are enabled with no reason.
    let start_stop = items
        .iter()
        .find(|i| i.label.starts_with(strings::UI_PALETTE_START) && i.label.contains("stop"))
        .expect("start row for the stopped instance");
    assert!(start_stop.is_enabled());
    assert!(start_stop.reason.is_none());
}

#[test]
fn palette_a_busy_instance_cannot_start_stop_or_copy() {
    let busy = view(
        "boot",
        EngineKey::Postgresql,
        InstanceStatus::Starting,
        5432,
    );
    let items = palette_items(&[busy]);
    for verb in [
        strings::UI_PALETTE_START,
        strings::UI_PALETTE_STOP,
        strings::UI_PALETTE_COPY_URI,
    ] {
        let row = items
            .iter()
            .find(|i| i.label.starts_with(verb))
            .unwrap_or_else(|| panic!("no row starting with {verb}"));
        assert!(!row.is_enabled(), "{verb} on a busy instance is disabled");
        assert_eq!(
            row.reason.as_deref(),
            Some(strings::UI_PALETTE_REASON_BUSY),
            "{verb}"
        );
    }
    // Open is always available.
    let open = items
        .iter()
        .find(|i| i.label.starts_with(strings::UI_PALETTE_OPEN))
        .expect("open row");
    assert!(open.is_enabled());
}

#[test]
fn palette_filter_is_case_insensitive_substring() {
    let items = palette_items(&[view(
        "shop",
        EngineKey::Mysql,
        InstanceStatus::Stopped,
        3306,
    )]);
    assert_eq!(filter_palette(&items, "").len(), items.len());
    let hits = filter_palette(&items, "SHOP");
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|i| i.label.contains("shop")));
    assert!(filter_palette(&items, "zzz").is_empty());
}

#[test]
fn palette_finds_an_instance_by_engine_version_or_port() {
    let items = palette_items(&[view(
        "shop",
        EngineKey::Mysql,
        InstanceStatus::Stopped,
        3306,
    )]);
    // The label says "shop"; the engine name, version and port are keywords.
    for query in ["mysql", "MySQL", "17.4", "3306"] {
        let hits = filter_palette(&items, query);
        assert!(
            hits.iter().any(|i| i.label.contains("shop")),
            "query {query:?} must find the instance by its keyword"
        );
    }
}
