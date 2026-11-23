//! Pure model for the "Open in…" menu (`F-OPN-01..02`).
//!
//! Detection returns whatever the service found; this model decides what the
//! menu shows. Two rules from the PRD drive it:
//!
//! - Only clients that were really detected appear (`F-OPN-01` AC1).
//! - When nothing was detected, the menu offers "Salin URI" instead of an empty
//!   list (`F-OPN-01` AC2).
//!
//! Clients are grouped GUI-first, then CLI, because that is the order the
//! prototype shows and the order a user scans for a launcher. A client that
//! cannot open the visible instance's engine is listed disabled with its
//! reason, never silently dropped, so the menu cannot look empty on a machine
//! where a client of another family is installed.

use dblocal_core::clients::{ClientKind, DetectedClient};
use dblocal_core::model::EngineKey;

/// One row of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenInEntry {
    /// The client this row launches.
    pub kind: ClientKind,
    /// Display name.
    pub label: &'static str,
    /// Whether the client can open the visible instance's engine.
    pub supports_engine: bool,
}

impl OpenInEntry {
    /// Whether the row is clickable.
    pub fn enabled(&self) -> bool {
        self.supports_engine
    }
}

/// One group of the menu, with the heading the view draws above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenInGroup {
    /// Heading, from `strings`.
    pub heading: &'static str,
    /// Rows in detection order.
    pub entries: Vec<OpenInEntry>,
}

/// What the menu renders for one instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenInMenu {
    /// Non-empty groups, GUI first.
    pub groups: Vec<OpenInGroup>,
    /// Whether to show "Salin URI" instead of the (empty) client list.
    pub copy_uri_fallback: bool,
}

impl OpenInMenu {
    /// Builds the menu from the detected clients and the visible engine.
    pub fn build(clients: &[DetectedClient], engine: EngineKey) -> Self {
        let mut gui = Vec::new();
        let mut cli = Vec::new();
        for client in clients {
            let entry = OpenInEntry {
                kind: client.kind(),
                label: client.kind().display_name(),
                supports_engine: client.supports(engine),
            };
            if client.kind().is_gui() {
                gui.push(entry);
            } else {
                cli.push(entry);
            }
        }
        let mut groups = Vec::new();
        if !gui.is_empty() {
            groups.push(OpenInGroup {
                heading: dblocal_core::strings::UI_OPEN_IN_GUI_GROUP,
                entries: gui,
            });
        }
        if !cli.is_empty() {
            groups.push(OpenInGroup {
                heading: dblocal_core::strings::UI_OPEN_IN_CLI_GROUP,
                entries: cli,
            });
        }
        Self {
            copy_uri_fallback: groups.is_empty(),
            groups,
        }
    }

    /// Every row, in menu order.
    pub fn entries(&self) -> impl Iterator<Item = &OpenInEntry> {
        self.groups.iter().flat_map(|group| group.entries.iter())
    }

    /// Whether any row can open this engine.
    pub fn has_launchable(&self) -> bool {
        self.entries().any(OpenInEntry::enabled)
    }

    /// Finds the row for a client kind, if it is on the menu.
    #[cfg(test)]
    pub fn entry(&self, kind: ClientKind) -> Option<&OpenInEntry> {
        self.entries().find(|entry| entry.kind == kind)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use dblocal_core::clients::DetectedClient;

    use super::*;

    fn client(kind: ClientKind) -> DetectedClient {
        DetectedClient::new(
            kind,
            PathBuf::from(format!("/usr/bin/{}", kind.as_str())),
            kind.supported_engines(),
        )
    }

    #[test]
    fn an_empty_detection_offers_copy_uri_instead_of_a_list() {
        let menu = OpenInMenu::build(&[], EngineKey::Postgresql);
        assert!(menu.copy_uri_fallback);
        assert!(menu.groups.is_empty());
        assert!(!menu.has_launchable());
    }

    #[test]
    fn gui_clients_come_before_cli_clients() {
        let clients = [
            client(ClientKind::Psql),
            client(ClientKind::TablePlus),
            client(ClientKind::DBeaver),
        ];
        let menu = OpenInMenu::build(&clients, EngineKey::Postgresql);
        assert!(!menu.copy_uri_fallback);
        assert_eq!(menu.groups.len(), 2);
        assert_eq!(
            menu.groups[0].heading,
            dblocal_core::strings::UI_OPEN_IN_GUI_GROUP
        );
        let gui: Vec<ClientKind> = menu.groups[0].entries.iter().map(|e| e.kind).collect();
        assert_eq!(gui, vec![ClientKind::TablePlus, ClientKind::DBeaver]);
        assert_eq!(
            menu.groups[1].heading,
            dblocal_core::strings::UI_OPEN_IN_CLI_GROUP
        );
        assert_eq!(menu.groups[1].entries[0].kind, ClientKind::Psql);
    }

    #[test]
    fn a_cli_only_machine_shows_only_the_cli_group() {
        let menu = OpenInMenu::build(&[client(ClientKind::Psql)], EngineKey::Postgresql);
        assert_eq!(menu.groups.len(), 1);
        assert_eq!(
            menu.groups[0].heading,
            dblocal_core::strings::UI_OPEN_IN_CLI_GROUP
        );
        assert!(!menu.copy_uri_fallback);
    }

    #[test]
    fn a_client_that_cannot_open_the_engine_is_listed_disabled() {
        // psql cannot open Valkey; the row stays, greyed, with no reason field
        // to invent.
        let menu = OpenInMenu::build(&[client(ClientKind::Psql)], EngineKey::Valkey);
        let entry = menu.entry(ClientKind::Psql).expect("listed");
        assert!(!entry.supports_engine);
        assert!(!entry.enabled());
        assert!(!menu.has_launchable());
    }

    #[test]
    fn a_matching_client_is_launchable() {
        let menu = OpenInMenu::build(
            &[client(ClientKind::Psql), client(ClientKind::TablePlus)],
            EngineKey::Postgresql,
        );
        assert!(menu.has_launchable());
        assert!(
            menu.entry(ClientKind::Psql)
                .is_some_and(OpenInEntry::enabled)
        );
        assert!(
            menu.entry(ClientKind::DBeaver).is_none(),
            "a client that was not detected must not appear"
        );
    }
}
