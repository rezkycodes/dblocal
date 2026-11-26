//! Small layout helpers the render tests pin: the create dialog's engine order
//! and download note, the connection card's header, and dock-row labels.
//!
//! The honest-placeholder machinery (`PlaceholderTab`, `placeholder_layout`, ...)
//! is gone: every sub-tab now has a live screen (`W4-B` and `W4-H`).

use dblocal_core::service::{EngineEntry, InstanceView};
use dblocal_core::strings;

use super::*;

/// Category tabs shown in the New instance dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineCategory {
    /// Relational databases: PostgreSQL, MySQL, MariaDB.
    Database,
    /// In-memory caches and queues: Valkey, Redis.
    CacheAndQueue,
    /// S3-compatible object storage: RustFS.
    Storage,
}

impl EngineCategory {
    /// Category tabs in display order.
    pub const ALL: [Self; 3] = [Self::Database, Self::CacheAndQueue, Self::Storage];

    /// Display label on the tab button.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Database => "Database",
            Self::CacheAndQueue => "Cache & Queue",
            Self::Storage => "Storage",
        }
    }

    /// Stable element ID slug for the tab button.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::CacheAndQueue => "cache-queue",
            Self::Storage => "storage",
        }
    }

    /// Engines shown under this category tab, in display order.
    pub const fn engines(self) -> &'static [EngineKey] {
        match self {
            Self::Database => &[EngineKey::Postgresql, EngineKey::Mysql, EngineKey::Mariadb],
            Self::CacheAndQueue => &[EngineKey::Valkey, EngineKey::Redis],
            Self::Storage => &[EngineKey::Rustfs],
        }
    }

    /// Category tab an engine belongs to.
    pub const fn for_engine(engine: EngineKey) -> Self {
        match engine {
            EngineKey::Postgresql | EngineKey::Mysql | EngineKey::Mariadb => Self::Database,
            EngineKey::Valkey | EngineKey::Redis => Self::CacheAndQueue,
            EngineKey::Rustfs => Self::Storage,
            _ => Self::Database,
        }
    }
}

/// The engine order the create dialog draws. The catalog and the dock keep the domain order.
pub const CREATE_ENGINE_ORDER: [EngineKey; 6] = [
    EngineKey::Postgresql,
    EngineKey::Valkey,
    EngineKey::Redis,
    EngineKey::Mysql,
    EngineKey::Mariadb,
    EngineKey::Rustfs,
];

/// Reorders catalog entries into the create dialog's card order.
pub fn create_engine_cards(entries: &[EngineEntry]) -> Vec<&EngineEntry> {
    CREATE_ENGINE_ORDER
        .iter()
        .filter_map(|engine| entries.iter().find(|entry| entry.engine == *engine))
        .collect()
}

/// Returns catalog entries belonging to `category`, preserving category display order.
///
/// Always returns the engines in the category even when all of them are
/// unavailable on this platform (for example Cache & Queue on Windows), so the
/// tab shows greyed cards with their unavailable reason and is never empty.
pub fn category_engine_cards(
    entries: &[EngineEntry],
    category: EngineCategory,
) -> Vec<&EngineEntry> {
    category
        .engines()
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

#[cfg(test)]
mod tests {
    use super::*;
    use dblocal_core::service::EngineEntry;

    fn entry(engine: EngineKey, available: bool) -> EngineEntry {
        EngineEntry {
            engine,
            available,
            unavailable_reason: (!available)
                .then(|| strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM.to_owned()),
            versions: Vec::new(),
        }
    }

    #[test]
    fn category_tabs_cover_all_engines_and_keep_all_unavailable_tab_non_empty() {
        // Simulate Windows: Valkey and Redis both unavailable in Cache & Queue,
        // while RustFS is available in Storage.
        let entries = vec![
            entry(EngineKey::Postgresql, true),
            entry(EngineKey::Mysql, true),
            entry(EngineKey::Mariadb, true),
            entry(EngineKey::Valkey, false),
            entry(EngineKey::Redis, false),
            entry(EngineKey::Rustfs, true),
        ];

        let db: Vec<EngineKey> = category_engine_cards(&entries, EngineCategory::Database)
            .into_iter()
            .map(|e| e.engine)
            .collect();
        assert_eq!(
            db,
            vec![EngineKey::Postgresql, EngineKey::Mysql, EngineKey::Mariadb]
        );

        let cache = category_engine_cards(&entries, EngineCategory::CacheAndQueue);
        assert_eq!(cache.len(), 2, "Cache & Queue tab must never be empty on Windows");
        assert!(cache.iter().all(|e| !e.available && e.unavailable_reason.is_some()));

        let storage: Vec<EngineKey> = category_engine_cards(&entries, EngineCategory::Storage)
            .into_iter()
            .map(|e| e.engine)
            .collect();
        assert_eq!(storage, vec![EngineKey::Rustfs]);
    }

    #[test]
    fn aws_secret_access_key_is_masked_in_dotenv() {
        let block = "AWS_ENDPOINT_URL=http://127.0.0.1:9000\nAWS_ACCESS_KEY_ID=DBLTEST1234567890123\nAWS_SECRET_ACCESS_KEY=top-secret-24-char-value\n";
        let masked = super::mask_dotenv(block);
        assert!(masked.contains("AWS_ENDPOINT_URL=http://127.0.0.1:9000"));
        assert!(masked.contains("AWS_ACCESS_KEY_ID=DBLTEST1234567890123"));
        assert!(!masked.contains("top-secret-24-char-value"));
        assert!(masked.contains(&format!("AWS_SECRET_ACCESS_KEY={SECRET_MASK}")));
    }
}
