//! Platform-neutral tray menu and icon specification (W2-G).
//!
//! `tray-icon` on Windows and macOS wants a flat list of native menu rows and
//! RGBA icon bytes; the Linux `ksni` backend wants ARGB bytes. The rules that
//! decide *what* a row is (label, enabled flag, status dot, aggregate icon) and
//! *which* [`TrayAction`] an activation id maps to are identical on every
//! platform, so they live here, are pure, and are tested on Linux.
//!
//! [`build`] turns the window's [`TrayMenuModel`] into [`TraySpec`];
//! [`activation_map`] turns it into the id → action table the desktop backend's
//! forwarding thread reads. Neither touches an OS API.

use std::collections::BTreeMap;

use dblocal_core::service::InstanceView;

use super::{
    MENU_ICON_SIZE, TRAY_ICON_SIZE, TrayAction, TrayMenuEntry, TrayMenuModel,
    aggregate_tray_status, status_dot_rgba_bytes, status_from_tray_label,
};

/// What a native menu row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuRowKind {
    /// A clickable entry with an id, label and status dot.
    Item,
    /// A non-clickable divider.
    Separator,
}

/// One row of the native menu, ready for `tray-icon` to build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuRow {
    /// Row kind; a separator has empty `id`, `label` and `icon_rgba`.
    pub kind: MenuRowKind,
    /// Activation id (`open`, `quit`, or an instance id).
    pub id: String,
    /// Visible label.
    pub label: String,
    /// Whether the row can be clicked.
    pub enabled: bool,
    /// RGBA8 status dot, `[r, g, b, a]` per pixel (empty for a separator).
    pub icon_rgba: Vec<u8>,
}

/// The complete native menu plus the aggregate tray icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraySpec {
    /// Rows in menu order: instances, a separator, "Buka DBLocal", "Keluar".
    pub rows: Vec<MenuRow>,
    /// RGBA8 aggregate tray icon (`TRAY_ICON_SIZE` square).
    pub icon_rgba: Vec<u8>,
}

/// Builds the native menu spec from the tray model and the instance views.
///
/// The row order matches the Linux menu: grouped instances, a separator (only
/// when there is at least one instance), then "Buka DBLocal" and "Keluar".
pub fn build(model: &TrayMenuModel, views: &[InstanceView]) -> TraySpec {
    let mut rows: Vec<MenuRow> = model
        .instances
        .iter()
        .map(item_row)
        .collect();
    if !rows.is_empty() {
        rows.push(separator_row());
    }
    rows.push(item_row(&model.open));
    rows.push(item_row(&model.quit));
    TraySpec {
        rows,
        icon_rgba: status_dot_rgba_bytes(aggregate_tray_status(views), TRAY_ICON_SIZE),
    }
}

fn item_row(entry: &TrayMenuEntry) -> MenuRow {
    let status = status_from_tray_label(&entry.label);
    MenuRow {
        kind: MenuRowKind::Item,
        id: entry.id.clone(),
        label: entry.label.clone(),
        enabled: entry.enabled,
        icon_rgba: status_dot_rgba_bytes(status, MENU_ICON_SIZE),
    }
}

fn separator_row() -> MenuRow {
    MenuRow {
        kind: MenuRowKind::Separator,
        id: String::new(),
        label: String::new(),
        enabled: false,
        icon_rgba: Vec::new(),
    }
}

/// Maps every menu id to the action it triggers.
///
/// The desktop forwarding thread receives a `muda` menu id on the event loop's
/// channel and looks it up here (through a shared, mutex-guarded copy) to decide
/// which [`TrayAction`] to send on the app's single flume action channel.
pub fn activation_map(model: &TrayMenuModel) -> BTreeMap<String, TrayAction> {
    let mut map = BTreeMap::new();
    map.insert(model.open.id.clone(), model.open.action.clone());
    map.insert(model.quit.id.clone(), model.quit.action.clone());
    for entry in &model.instances {
        map.insert(entry.id.clone(), entry.action.clone());
    }
    map
}

/// Resolves an activation id to the action it must produce, or `None` if the
/// id is unknown (e.g. a stale row for a removed instance).
pub fn resolve(map: &BTreeMap<String, TrayAction>, id: &str) -> Option<TrayAction> {
    map.get(id).cloned()
}
