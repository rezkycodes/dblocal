//! Small layout helpers the render tests pin: the create dialog's engine order
//! and download note, the connection card's header, and dock-row labels.
//!
//! The honest-placeholder machinery (`PlaceholderTab`, `placeholder_layout`, ...)
//! is gone: every sub-tab now has a live screen (`W4-B` and `W4-H`).

use dblocal_core::service::{EngineEntry, InstanceView};
use dblocal_core::strings;

use super::*;

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
/// "Connection" twice, which is the regression this struct prevents.
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

/// How many times "Connection" appears in the connection card's chrome and header.
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
