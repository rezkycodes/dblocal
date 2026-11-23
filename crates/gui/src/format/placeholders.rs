//! Pure view-model descriptors for the prototype-port placeholders and
//! small layout helpers the render tests pin.

use dblocal_core::service::{EngineEntry, InstanceView};
use dblocal_core::strings;

use super::*;

/// Which honest placeholder tab a descriptor describes.
///
/// Only Config is left: `F-DBU` and `F-OPN` now have their view, so the
/// Databases tab never renders the "belum tersedia" state (`W4-B`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceholderTab {
    /// Config (`F-CFG`, Phase 3).
    Config,
}

/// How a placeholder control is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceholderControlKind {
    /// A disabled button with the label.
    Button,
}

/// One disabled control inside a placeholder card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderControl {
    /// Visible label, from `strings`.
    pub label: &'static str,
    /// Whether the control is disabled (always true in the product build).
    pub disabled: bool,
    /// Control shape (currently all buttons; kept so a field can be added back).
    pub kind: PlaceholderControlKind,
}

impl PlaceholderControl {
    fn button(label: &'static str) -> Self {
        Self {
            label,
            disabled: true,
            kind: PlaceholderControlKind::Button,
        }
    }
}

/// One card of a placeholder layout: the prototype structure with no data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderCard {
    /// Card heading, when the prototype draws one.
    pub title: Option<&'static str>,
    /// Muted explanation the card carries.
    pub body: Option<&'static str>,
    /// Column hint the prototype prints on its table header.
    pub columns: Option<&'static str>,
    /// Empty-state sentence shown where rows would go.
    pub empty: Option<&'static str>,
    /// Disabled controls in the order the prototype draws them.
    pub controls: Vec<PlaceholderControl>,
}

/// The prototype structure of a placeholder tab, with every control disabled.
///
/// This is the single source the render code iterates, so a test asserting the
/// label list also pins what the view draws. No sample rows ever appear here:
/// the demo build has its own content path.
pub fn placeholder_layout(tab: PlaceholderTab) -> Vec<PlaceholderCard> {
    match tab {
        PlaceholderTab::Config => vec![
            PlaceholderCard {
                title: None,
                body: Some(strings::UI_CONFIG_RESTART_REQUIRED),
                columns: None,
                empty: None,
                controls: vec![
                    PlaceholderControl::button(strings::UI_CONFIG_LATER),
                    PlaceholderControl::button(strings::UI_CONFIG_RESTART_NOW),
                ],
            },
            PlaceholderCard {
                title: Some(strings::UI_CONFIG_HEADING),
                body: None,
                columns: Some(strings::UI_CONFIG_COLUMNS),
                empty: Some(strings::UI_PLACEHOLDER_CONFIG),
                controls: vec![],
            },
            PlaceholderCard {
                title: None,
                body: None,
                columns: None,
                empty: None,
                controls: vec![
                    PlaceholderControl::button(strings::UI_CONFIG_OPEN_FILE),
                    PlaceholderControl::button(strings::UI_CONFIG_REVERT),
                    PlaceholderControl::button(strings::UI_CONFIG_DISCARD),
                    PlaceholderControl::button(strings::UI_CONFIG_SAVE),
                ],
            },
        ],
    }
}

/// Every label a placeholder tab draws, in order.
pub fn placeholder_labels(tab: PlaceholderTab) -> Vec<&'static str> {
    placeholder_layout(tab)
        .into_iter()
        .flat_map(|card| card.controls)
        .map(|control| control.label)
        .collect()
}

/// The phase label a placeholder carries (`Fase 2`, `Fase 3`).
pub fn placeholder_phase_label(tab: PlaceholderTab) -> &'static str {
    match tab {
        PlaceholderTab::Config => "3",
    }
}

/// The engine order the create dialog draws (`D-25` prototype: PostgreSQL,
/// Valkey, MySQL, MariaDB). The catalog and the dock keep the domain order.
pub const CREATE_ENGINE_ORDER: [EngineKey; 4] = [
    EngineKey::Postgresql,
    EngineKey::Valkey,
    EngineKey::Mysql,
    EngineKey::Mariadb,
];

/// Reorders catalog entries into the create dialog's card order.
pub fn create_engine_cards(entries: &[EngineEntry]) -> Vec<&EngineEntry> {
    CREATE_ENGINE_ORDER
        .iter()
        .filter_map(|engine| entries.iter().find(|entry| entry.engine == *engine))
        .collect()
}

/// The footer note the create dialog shows for the selected version
/// (`F-INS-01` AC2: promise a download only when one is needed).
pub fn create_footer_note(installed: bool) -> &'static str {
    if create_needs_download(installed) {
        strings::UI_CREATE_DOWNLOAD_NOTE
    } else {
        strings::UI_CREATE_READY_NOTE
    }
}

/// The connection card's header: the title once, and the credential note.
///
/// `card_title` is the optional heading the card chrome draws. It stays `None`
/// because the header row already paints the title; setting both would print
/// "Koneksi" twice, which is the regression this struct prevents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionHeader {
    /// Heading drawn by the card chrome itself, if any.
    pub card_title: Option<&'static str>,
    /// Title drawn inside the header row.
    pub title: &'static str,
    /// Credential-location note shown on the right.
    pub credential: String,
}

/// Builds the connection card's header from the credential note.
pub fn connection_header(credential_note: &str) -> ConnectionHeader {
    ConnectionHeader {
        card_title: None,
        title: strings::UI_OVERVIEW_CONNECTION,
        credential: credential_note.to_owned(),
    }
}

/// How many times "Koneksi" appears in the connection card's chrome and header.
pub fn connection_title_occurrences(header: &ConnectionHeader) -> usize {
    usize::from(header.card_title == Some(strings::UI_OVERVIEW_CONNECTION))
        + usize::from(header.title == strings::UI_OVERVIEW_CONNECTION)
}

/// The three labels a dock instance row draws on its single line.
pub fn dock_row_labels(view: &InstanceView) -> [String; 3] {
    [
        view.name.clone(),
        short_version(&view.version),
        format!(":{}", view.port),
    ]
}

/// The accessible label of a dock row: name plus the status word, so status
/// stays available to assistive technology when the row is one line.
pub fn dock_row_accessible(view: &InstanceView) -> String {
    format!("{} — {}", view.name, status_label(view.status))
}

/// Height of one dock instance row in logical pixels, from the prototype.
pub const DOCK_ROW_HEIGHT: f32 = 30.0;
