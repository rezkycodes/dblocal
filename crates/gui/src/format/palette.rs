//! The command palette's pure view-model (`F-PLH-01` AC2).
//!
//! Split out of `format/mod.rs` to keep that file under the 800-line limit. A
//! palette row carries a label, the keywords it also matches (engine, version,
//! port), the action it runs, and — when it cannot run right now — the reason
//! shown to the user. Re-exported from `format` so call sites are unchanged.

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::InstanceView;
use dblocal_core::strings;

use super::{can_start, can_stop, is_busy};

/// One command palette entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteAction {
    /// Open the create-instance dialog.
    NewInstance,
    /// Open an instance tab.
    Open(String),
    /// Start an instance.
    Start(String),
    /// Stop an instance.
    Stop(String),
    /// Copy an instance URI.
    CopyUri(String),
    /// Open the delete dialog for an instance (`F-INS-05`).
    Delete(String),
}

/// A palette row: a label, the words it also matches, the action it runs, and
/// — when the action cannot run right now — the reason shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    /// Text shown and searched.
    pub label: String,
    /// Extra terms the query also matches (engine name, version, port), so a
    /// user can find an instance by engine even when its name does not say so.
    pub keywords: Vec<String>,
    /// Action to run on confirm; `None` for a row that is shown but cannot run
    /// (the row is disabled and carries [`Self::reason`]).
    pub action: Option<PaletteAction>,
    /// Why the action is unavailable, shown when `action` is `None`.
    pub reason: Option<String>,
}

impl PaletteItem {
    /// Whether this row runs an action when confirmed.
    pub fn is_enabled(&self) -> bool {
        self.action.is_some()
    }
}

/// The words a palette row is matched by, label plus keywords, lower-cased.
fn palette_haystack(item: &PaletteItem) -> String {
    let mut text = item.label.to_lowercase();
    for keyword in &item.keywords {
        text.push(' ');
        text.push_str(&keyword.to_lowercase());
    }
    text
}

/// Builds the full palette list for the current instances.
///
/// `F-PLH-01` AC2: every instance offers start or stop as applicable, open, and
/// copy URI; a command that does not apply (start on a running instance, stop on
/// a stopped one) is still listed but disabled with its reason, so the palette
/// explains itself instead of silently hiding rows. Each row also carries the
/// engine, version and port as keywords so typing an engine name finds it.
pub fn palette_items(instances: &[InstanceView]) -> Vec<PaletteItem> {
    let mut items = vec![PaletteItem {
        label: strings::UI_PALETTE_NEW.to_owned(),
        keywords: Vec::new(),
        action: Some(PaletteAction::NewInstance),
        reason: None,
    }];
    for view in instances {
        let keywords = vec![
            view.engine.display_name().to_owned(),
            view.version.clone(),
            view.port.get().to_string(),
        ];
        // Start: offered only for a stopped/failed instance, else disabled with
        // the reason. A busy instance cannot start either.
        let start = if can_start(view.status) {
            PaletteItem {
                label: format!("{} {}", strings::UI_PALETTE_START, view.name),
                keywords: keywords.clone(),
                action: Some(PaletteAction::Start(view.id.to_string())),
                reason: None,
            }
        } else {
            PaletteItem {
                label: format!("{} {}", strings::UI_PALETTE_START, view.name),
                keywords: keywords.clone(),
                action: None,
                reason: Some(palette_reason(view.status, PaletteVerb::Start).to_owned()),
            }
        };
        items.push(start);
        // Stop: offered only for a running instance.
        let stop = if can_stop(view.status) {
            PaletteItem {
                label: format!("{} {}", strings::UI_PALETTE_STOP, view.name),
                keywords: keywords.clone(),
                action: Some(PaletteAction::Stop(view.id.to_string())),
                reason: None,
            }
        } else {
            PaletteItem {
                label: format!("{} {}", strings::UI_PALETTE_STOP, view.name),
                keywords: keywords.clone(),
                action: None,
                reason: Some(palette_reason(view.status, PaletteVerb::Stop).to_owned()),
            }
        };
        items.push(stop);
        items.push(PaletteItem {
            label: format!("{} {}", strings::UI_PALETTE_OPEN, view.name),
            keywords: keywords.clone(),
            action: Some(PaletteAction::Open(view.id.to_string())),
            reason: None,
        });
        // Delete (`F-INS-05`) is always offered: the dialog shows the data size
        // and both modes and explains the consequence, so there is no state in
        // which the row would mislead a user by opening.
        items.push(PaletteItem {
            label: format!("{} {}", strings::UI_PALETTE_DELETE, view.name),
            keywords: keywords.clone(),
            action: Some(PaletteAction::Delete(view.id.to_string())),
            reason: None,
        });
        // Copy URI only needs a settled instance with a known URI: a busy
        // transition has none yet, so it is disabled with that reason.
        let copy = if is_busy(view.status) {
            PaletteItem {
                label: format!("{} {}", strings::UI_PALETTE_COPY_URI, view.name),
                keywords,
                action: None,
                reason: Some(palette_reason(view.status, PaletteVerb::CopyUri).to_owned()),
            }
        } else {
            PaletteItem {
                label: format!("{} {}", strings::UI_PALETTE_COPY_URI, view.name),
                keywords,
                action: Some(PaletteAction::CopyUri(view.id.to_string())),
                reason: None,
            }
        };
        items.push(copy);
    }
    items
}

/// The verb a palette reason sentence is about.
#[derive(Debug, Clone, Copy)]
enum PaletteVerb {
    Start,
    Stop,
    CopyUri,
}

/// The reason a palette row is disabled, from the instance status.
///
/// `F-PLH-01` AC2 names "start on a running instance" as the canonical case; the
/// busy states get their own sentence so the reason is never generic.
fn palette_reason(status: InstanceStatus, verb: PaletteVerb) -> &'static str {
    if is_busy(status) {
        return strings::UI_PALETTE_REASON_BUSY;
    }
    match verb {
        PaletteVerb::Start => strings::UI_PALETTE_REASON_ALREADY_RUNNING,
        PaletteVerb::Stop => strings::UI_PALETTE_REASON_NOT_RUNNING,
        PaletteVerb::CopyUri => strings::UI_PALETTE_REASON_BUSY,
    }
}

/// Case-insensitive substring filter used by the palette.
///
/// Matches the label and every keyword, so `postgres` finds an instance named
/// `shop` and `17.4` finds every instance of that version.
pub fn filter_palette<'a>(items: &'a [PaletteItem], query: &str) -> Vec<&'a PaletteItem> {
    let query = query.trim().to_lowercase();
    items
        .iter()
        .filter(|item| query.is_empty() || palette_haystack(item).contains(&query))
        .collect()
}
