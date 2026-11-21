//! Pure state reduction for the event stream.
//!
//! The workspace owns GPUI entities, but the *effect* of each service event on
//! the visible collections is ordinary data handling. Keeping it here means it
//! can be unit-tested without a window, an event loop, or a live service.

use std::collections::HashMap;

use dblocal_core::model::{DownloadProgress, InstanceId};
use dblocal_core::secret::SecretString;
use dblocal_core::service::{InstanceView, OperationId, OperationStage, UserFacingError};

use crate::format;
use crate::logs::LogStore;

/// Which inner tab of an instance is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InnerTab {
    /// Overview: connection, detail and process cards.
    #[default]
    Overview,
    /// Logs: virtualised tail with follow and copy.
    Logs,
    /// Databases: Phase 2 (`F-DBU`), a placeholder in the product build.
    Databases,
    /// Config: Phase 3 (`F-CFG`), a placeholder in the product build.
    Config,
}

impl InnerTab {
    /// Every inner tab, in bar order.
    pub const ALL: [Self; 4] = [Self::Overview, Self::Logs, Self::Databases, Self::Config];

    /// Indonesian label for the tab bar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => dblocal_core::strings::UI_SUBTAB_OVERVIEW,
            Self::Logs => dblocal_core::strings::UI_SUBTAB_LOGS,
            Self::Databases => dblocal_core::strings::UI_SUBTAB_DATABASES,
            Self::Config => dblocal_core::strings::UI_SUBTAB_CONFIG,
        }
    }

    /// The tab's index in [`InnerTab::ALL`].
    pub fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Logs => 1,
            Self::Databases => 2,
            Self::Config => 3,
        }
    }

    /// The tab at an index, defaulting to Overview.
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// Whether this tab has no backend in the frozen service.
    pub fn is_placeholder(self) -> bool {
        matches!(self, Self::Databases | Self::Config)
    }
}

/// One open tab in the centre pane.
#[derive(Debug, Clone)]
pub struct OpenTab {
    /// Instance the tab shows.
    pub instance: InstanceId,
    /// Active inner tab.
    pub inner: InnerTab,
}

/// The centre pane's tabs and the current selection.
///
/// Pure data with no GPUI types, so opening, closing and removing tabs can be
/// unit-tested without a window.
#[derive(Debug, Default)]
pub struct Tabs {
    open: Vec<OpenTab>,
    selected: usize,
    selected_instance: Option<InstanceId>,
}

impl Tabs {
    /// Every open tab, in order.
    pub fn open(&self) -> &[OpenTab] {
        &self.open
    }

    /// The selected tab index (clamped to a valid index when tabs exist).
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// The instance the sidebar highlights, if any.
    pub fn selected_instance(&self) -> Option<&InstanceId> {
        self.selected_instance.as_ref()
    }

    /// The tab at the selected index, if any.
    pub fn selected_tab(&self) -> Option<&OpenTab> {
        self.open.get(self.selected)
    }

    /// Whether no tab is open.
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// Number of open tabs.
    pub fn len(&self) -> usize {
        self.open.len()
    }

    /// Selects an existing tab index and records its instance. Returns the
    /// instance that is now visible, or `None` when the index is out of range.
    pub fn select(&mut self, index: usize) -> Option<InstanceId> {
        let tab = self.open.get(index)?;
        self.selected = index;
        let id = tab.instance.clone();
        self.selected_instance = Some(id.clone());
        Some(id)
    }

    /// Sets the selected tab's inner tab.
    pub fn set_inner(&mut self, index: usize, inner: InnerTab) {
        if let Some(tab) = self.open.get_mut(index) {
            tab.inner = inner;
        }
    }

    /// Opens a tab for `id`, or selects the existing one. Returns the index.
    pub fn open_or_select(&mut self, id: InstanceId) -> usize {
        if let Some(position) = self.open.iter().position(|tab| tab.instance == id) {
            self.selected = position;
        } else {
            self.open.push(OpenTab {
                instance: id.clone(),
                inner: InnerTab::Overview,
            });
            self.selected = self.open.len() - 1;
        }
        self.selected_instance = Some(id);
        self.selected
    }

    /// Removes the tab at `index`, keeping the selection valid.
    pub fn close(&mut self, index: usize) -> Option<OpenTab> {
        if index >= self.open.len() {
            return None;
        }
        let removed = self.open.remove(index);
        if self.selected_instance.as_ref() == Some(&removed.instance) {
            self.selected_instance = None;
        }
        if self.open.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.open.len() {
            self.selected = self.open.len() - 1;
        }
        Some(removed)
    }

    /// Removes the tab for `id`, if open. Returns the removed tab.
    pub fn close_by_instance(&mut self, id: &InstanceId) -> Option<OpenTab> {
        let position = self.open.iter().position(|tab| &tab.instance == id)?;
        self.close(position)
    }
}

/// The connection URI of one instance, in the form the Overview card shows.
///
/// Only the masked preview is retained: the plaintext is fetched from the
/// service at the moment the user asks for it and dropped again on hide, so a
/// revealed password never sits in view state longer than the user needs it
/// (`F-CRD-02`, task requirement 5).
#[derive(Debug, Clone)]
pub struct UriView {
    /// The masked preview, built from the URI once and safe to render.
    pub masked: String,
    /// The plaintext, present only while the user has it revealed.
    revealed: Option<SecretString>,
}

impl UriView {
    /// Builds a view from a freshly fetched URI, in the masked state.
    pub fn masked_from(uri: &SecretString) -> Self {
        Self {
            masked: format::mask_uri(uri.expose()),
            revealed: None,
        }
    }

    /// Whether the plaintext is currently held.
    pub fn is_revealed(&self) -> bool {
        self.revealed.is_some()
    }

    /// The text the card renders: plaintext while revealed, the mask otherwise.
    pub fn text(&self) -> &str {
        match &self.revealed {
            Some(secret) => secret.expose(),
            None => &self.masked,
        }
    }

    /// Stores the plaintext the service returned for a reveal request.
    pub fn reveal(&mut self, uri: SecretString) {
        self.masked = format::mask_uri(uri.expose());
        self.revealed = Some(uri);
    }

    /// Drops the plaintext, keeping the masked preview.
    pub fn hide(&mut self) {
        self.revealed = None;
    }
}

/// One in-flight download the status bar tracks.
#[derive(Debug, Clone)]
pub struct ActiveDownload {
    /// Operation that owns it.
    pub operation: OperationId,
    /// Display title, e.g. `PostgreSQL 17.4`.
    pub title: String,
    /// Latest sample.
    pub progress: DownloadProgress,
}

/// The collections that events mutate.
#[derive(Debug, Default)]
pub struct ShellState {
    /// Every known instance, in stable list order.
    pub instances: Vec<InstanceView>,
    /// In-flight downloads.
    pub downloads: Vec<ActiveDownload>,
    /// The last failure surfaced to the user.
    pub last_failure: Option<UserFacingError>,
    /// Connection URI per opened tab, keyed by instance.
    pub uris: HashMap<InstanceId, UriView>,
}

impl ShellState {
    /// Inserts or replaces one instance by id.
    pub fn upsert_instance(&mut self, view: InstanceView) {
        match self
            .instances
            .iter_mut()
            .find(|existing| existing.id == view.id)
        {
            Some(existing) => *existing = view,
            None => self.instances.push(view),
        }
    }

    /// Applies an operation stage: a terminal stage clears its download.
    pub fn on_progress(&mut self, id: &OperationId, stage: OperationStage) {
        if matches!(stage, OperationStage::Done | OperationStage::Failed) {
            self.downloads.retain(|download| &download.operation != id);
        }
    }

    /// Records a download sample, creating the row on first sight.
    pub fn on_download(&mut self, id: OperationId, progress: DownloadProgress, title: String) {
        if let Some(download) = self
            .downloads
            .iter_mut()
            .find(|download| download.operation == id)
        {
            download.progress = progress;
        } else {
            self.downloads.push(ActiveDownload {
                operation: id,
                title,
                progress,
            });
        }
    }

    /// Finishes an operation, clearing its download and capturing a failure.
    pub fn on_finished(&mut self, id: &OperationId, result: Result<(), UserFacingError>) {
        self.downloads.retain(|download| &download.operation != id);
        if let Err(err) = result {
            self.last_failure = Some(err);
        }
    }

    /// Appends live lines to the buffer of the instance they belong to.
    ///
    /// The store is keyed by `InstanceId`, so a line for instance B can never
    /// land under an open tab for instance A.
    pub fn on_log_lines(&self, logs: &mut LogStore, instance: &InstanceId, lines: Vec<String>) {
        logs.append(instance, lines);
    }

    /// Replaces the instance list from a fresh read.
    pub fn replace_instances(&mut self, instances: Vec<InstanceView>) {
        self.instances = instances;
    }

    /// Drops everything that referenced a removed instance.
    ///
    /// Removes the row, closes its tab (clearing the selection if it pointed at
    /// the instance), drops its per-instance log buffer and forgets its URI, so
    /// a deleted instance cannot leave a secret behind. An unknown id is a
    /// no-op.
    ///
    /// The caller still issues `watch_logs(id, false)`; that is I/O, not state.
    pub fn apply_removed(&mut self, id: &InstanceId, tabs: &mut Tabs, logs: &mut LogStore) {
        self.instances.retain(|view| &view.id != id);
        tabs.close_by_instance(id);
        logs.close(id);
        self.uris.remove(id);
    }

    /// Stores the masked URI preview for an opened tab.
    pub fn set_uri(&mut self, id: &InstanceId, uri: &SecretString) {
        self.uris.insert(id.clone(), UriView::masked_from(uri));
    }

    /// The URI view for an instance, if one was fetched.
    pub fn uri(&self, id: &InstanceId) -> Option<&UriView> {
        self.uris.get(id)
    }

    /// Mutable URI view, for reveal and hide.
    pub fn uri_mut(&mut self, id: &InstanceId) -> Option<&mut UriView> {
        self.uris.get_mut(id)
    }

    /// Forgets an instance's URI entirely (hide drops the plaintext, not the
    /// mask; this is for a closed tab).
    pub fn forget_uri(&mut self, id: &InstanceId) {
        self.uris.remove(id);
    }

    /// The number of instances currently running.
    pub fn running_count(&self) -> usize {
        format::running_count(&self.instances)
    }

    /// The three status-bar counts.
    pub fn counts(&self) -> format::InstanceCounts {
        format::instance_counts(&self.instances)
    }
}

#[cfg(test)]
mod tests {
    use dblocal_core::model::{
        AuthMode, EngineKey, InstanceId, InstanceName, InstanceSpec, InstanceStatus, Port,
    };
    use dblocal_core::service::UserFacingError;

    use super::*;

    fn view(name: &str, status: InstanceStatus) -> InstanceView {
        let spec = InstanceSpec::new(
            InstanceName::new(name).expect("name"),
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

    fn secret() -> SecretString {
        SecretString::new("postgresql://postgres:hunter2@127.0.0.1:5432/postgres")
    }

    #[test]
    fn upsert_replaces_by_id_and_appends_new() {
        let mut state = ShellState::default();
        let first = view("one", InstanceStatus::Stopped);
        let id = first.id.clone();
        state.upsert_instance(first);
        assert_eq!(state.instances.len(), 1);

        let mut updated = view("ignored", InstanceStatus::Running);
        updated.id = id.clone();
        state.upsert_instance(updated);
        assert_eq!(state.instances.len(), 1);
        assert_eq!(state.instances[0].status, InstanceStatus::Running);

        state.upsert_instance(view("two", InstanceStatus::Stopped));
        assert_eq!(state.instances.len(), 2);
    }

    #[test]
    fn progress_and_finish_clear_downloads() {
        let mut state = ShellState::default();
        let op = OperationId::new("op-1");
        state.on_download(
            op.clone(),
            DownloadProgress::new(1, 10, 2, None),
            "PostgreSQL 17.4".to_owned(),
        );
        assert_eq!(state.downloads.len(), 1);

        state.on_download(
            op.clone(),
            DownloadProgress::new(5, 10, 2, None),
            "ignored".to_owned(),
        );
        assert_eq!(state.downloads.len(), 1);
        assert_eq!(state.downloads[0].progress.bytes_done, 5);

        state.on_progress(&op, OperationStage::Downloading);
        assert_eq!(state.downloads.len(), 1, "non-terminal stage keeps the row");
        state.on_progress(&op, OperationStage::Done);
        assert!(state.downloads.is_empty());
    }

    #[test]
    fn finish_captures_the_failure() {
        let mut state = ShellState::default();
        let op = OperationId::new("op-2");
        state.on_finished(&op, Err(UserFacingError::new("DBL-1005", "port busy")));
        assert_eq!(
            state.last_failure.as_ref().map(|e| e.code.as_str()),
            Some("DBL-1005")
        );
    }

    #[test]
    fn log_lines_land_in_their_own_instance_buffer() {
        use crate::logs::LogStore;
        let mut logs = LogStore::new();
        let pg = InstanceId::new();
        let other = InstanceId::new();
        logs.load(&pg, Vec::new());
        logs.load(&other, Vec::new());
        let state = ShellState::default();
        state.on_log_lines(&mut logs, &pg, vec!["a".to_owned()]);
        assert_eq!(logs.get(&pg).expect("pg").len(), 1);
        assert_eq!(logs.get(&other).expect("other").len(), 0);
        state.on_log_lines(&mut logs, &other, vec!["b".to_owned()]);
        assert_eq!(
            logs.get(&pg).and_then(|b| b.line(0)).map(|s| s.as_ref()),
            Some("a")
        );
        assert_eq!(
            logs.get(&other).and_then(|b| b.line(0)).map(|s| s.as_ref()),
            Some("b")
        );
    }

    fn instance_id() -> InstanceId {
        InstanceId::new()
    }

    #[test]
    fn inner_tabs_are_labelled_indexed_and_flag_the_placeholders() {
        assert_eq!(InnerTab::default(), InnerTab::Overview);
        for (index, tab) in InnerTab::ALL.iter().enumerate() {
            assert_eq!(tab.index(), index);
            assert_eq!(InnerTab::from_index(index), *tab);
            assert!(!tab.label().is_empty());
        }
        assert_eq!(InnerTab::from_index(99), InnerTab::Overview);
        assert!(!InnerTab::Overview.is_placeholder());
        assert!(!InnerTab::Logs.is_placeholder());
        assert!(InnerTab::Databases.is_placeholder());
        assert!(InnerTab::Config.is_placeholder());
    }

    #[test]
    fn inner_tab_labels_match_the_prototype_screen() {
        assert_eq!(InnerTab::Overview.label(), "Overview");
        assert_eq!(InnerTab::Logs.label(), "Logs");
        assert_eq!(InnerTab::Databases.label(), "Databases");
        assert_eq!(InnerTab::Config.label(), "Config");
    }

    #[test]
    fn tabs_open_select_and_deduplicate() {
        let mut tabs = Tabs::default();
        let a = instance_id();
        assert_eq!(tabs.open_or_select(a.clone()), 0);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.selected_instance(), Some(&a));
        let b = instance_id();
        assert_eq!(tabs.open_or_select(b.clone()), 1);
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs.open_or_select(a.clone()), 0);
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs.select(1), Some(b));
        assert_eq!(tabs.selected_index(), 1);
        assert!(tabs.select(9).is_none());
    }

    #[test]
    fn tabs_close_keeps_the_selection_valid() {
        let mut tabs = Tabs::default();
        let a = instance_id();
        let b = instance_id();
        tabs.open_or_select(a.clone());
        tabs.open_or_select(b.clone());
        let closed = tabs.close(1).expect("tab 1 exists");
        assert_eq!(closed.instance, b);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.selected_index(), 0);
        assert!(tabs.selected_instance().is_none());
        tabs.close(0);
        assert!(tabs.is_empty());
        assert_eq!(tabs.selected_index(), 0);
        assert!(tabs.selected_tab().is_none());
        assert!(tabs.close(0).is_none(), "out-of-range close is a no-op");
    }

    #[test]
    fn inner_tab_is_per_tab_not_global() {
        let mut tabs = Tabs::default();
        let a = instance_id();
        let b = instance_id();
        tabs.open_or_select(a);
        tabs.open_or_select(b);
        tabs.set_inner(0, InnerTab::Logs);
        assert_eq!(tabs.open()[0].inner, InnerTab::Logs);
        assert_eq!(tabs.open()[1].inner, InnerTab::Overview);
        // Out-of-range writes are ignored rather than panicking.
        tabs.set_inner(9, InnerTab::Config);
        assert_eq!(tabs.open()[1].inner, InnerTab::Overview);
    }

    #[test]
    fn apply_removed_drops_row_tab_selection_logs_and_uri() {
        use crate::logs::LogStore;
        let mut state = ShellState::default();
        let mut tabs = Tabs::default();
        let mut logs = LogStore::new();

        let gone = view("gone", InstanceStatus::Stopped);
        let kept = view("kept", InstanceStatus::Running);
        state.upsert_instance(gone.clone());
        state.upsert_instance(kept.clone());
        tabs.open_or_select(gone.id.clone());
        logs.load(&gone.id, vec!["log line".to_owned()]);
        logs.load(&kept.id, vec!["keep me".to_owned()]);
        state.set_uri(&gone.id, &secret());
        state.set_uri(&kept.id, &secret());

        state.apply_removed(&gone.id, &mut tabs, &mut logs);

        assert_eq!(state.instances.len(), 1, "the removed row is gone");
        assert_eq!(state.instances[0].id, kept.id);
        assert!(tabs.open().iter().all(|tab| tab.instance != gone.id));
        assert!(tabs.selected_instance().is_none(), "selection cleared");
        assert!(logs.get(&gone.id).is_none(), "log buffer dropped");
        assert!(logs.get(&kept.id).is_some(), "other logs untouched");
        assert!(state.uri(&gone.id).is_none(), "the secret was forgotten");
        assert!(state.uri(&kept.id).is_some());
    }

    #[test]
    fn apply_removed_clears_selection_even_when_another_tab_remains() {
        use crate::logs::LogStore;
        let mut state = ShellState::default();
        let mut tabs = Tabs::default();
        let mut logs = LogStore::new();
        let gone = view("gone", InstanceStatus::Stopped);
        let other = view("other", InstanceStatus::Running);
        state.upsert_instance(gone.clone());
        state.upsert_instance(other.clone());
        tabs.open_or_select(other.id.clone());
        tabs.open_or_select(gone.id.clone());
        assert_eq!(tabs.selected_instance(), Some(&gone.id));

        state.apply_removed(&gone.id, &mut tabs, &mut logs);

        assert_eq!(tabs.len(), 1, "the other tab stays open");
        assert_eq!(tabs.open()[0].instance, other.id);
        assert!(tabs.selected_instance().is_none());
    }

    #[test]
    fn apply_removed_for_an_unknown_id_is_a_no_op() {
        use crate::logs::LogStore;
        let mut state = ShellState::default();
        let mut tabs = Tabs::default();
        let mut logs = LogStore::new();
        let present = view("present", InstanceStatus::Running);
        state.upsert_instance(present.clone());
        tabs.open_or_select(present.id.clone());
        logs.load(&present.id, vec!["x".to_owned()]);
        state.set_uri(&present.id, &secret());

        state.apply_removed(&instance_id(), &mut tabs, &mut logs);

        assert_eq!(state.instances.len(), 1);
        assert_eq!(tabs.len(), 1);
        assert!(logs.get(&present.id).is_some());
        assert!(state.uri(&present.id).is_some());
    }

    #[test]
    fn running_count_and_counts_read_the_list() {
        let mut state = ShellState::default();
        state.upsert_instance(view("run", InstanceStatus::Running));
        state.upsert_instance(view("stop", InstanceStatus::Stopped));
        state.upsert_instance(view("bad", InstanceStatus::Failed));
        assert_eq!(state.running_count(), 1);
        assert_eq!(state.counts().running, 1);
        assert_eq!(state.counts().stopped, 1);
        assert_eq!(state.counts().failed, 1);
        state.replace_instances(vec![]);
        assert_eq!(state.running_count(), 0);
        assert!(state.counts().is_empty());
    }

    #[test]
    fn a_uri_view_masks_until_revealed_and_hides_again() {
        let mut view = UriView::masked_from(&secret());
        assert!(!view.is_revealed());
        assert!(!view.text().contains("hunter2"));
        assert!(view.text().contains("••••••"));
        assert!(view.text().contains("127.0.0.1:5432"));

        view.reveal(secret());
        assert!(view.is_revealed());
        assert!(view.text().contains("hunter2"));

        view.hide();
        assert!(!view.is_revealed());
        assert!(!view.text().contains("hunter2"));
        assert!(view.text().contains("••••••"));
    }

    #[test]
    fn a_revealed_uri_view_never_renders_its_password_in_debug() {
        let mut view = UriView::masked_from(&secret());
        view.reveal(secret());
        // `SecretString` redacts itself, so the derived `Debug` is safe.
        let rendered = format!("{view:?}");
        assert!(!rendered.contains("hunter2"));
        assert!(rendered.contains("SecretString(***)"));
    }

    #[test]
    fn uri_state_is_keyed_per_instance_and_forget_drops_it() {
        let mut state = ShellState::default();
        let a = instance_id();
        let b = instance_id();
        state.set_uri(&a, &secret());
        state.set_uri(
            &b,
            &SecretString::new("redis://default:other@127.0.0.1:6379"),
        );
        assert!(state.uri(&a).is_some());
        assert!(state.uri(&b).is_some());
        assert_ne!(
            state.uri(&a).map(UriView::text),
            state.uri(&b).map(UriView::text)
        );

        state.uri_mut(&a).expect("a").reveal(secret());
        assert!(state.uri(&a).expect("a").is_revealed());

        state.forget_uri(&a);
        assert!(state.uri(&a).is_none());
        assert!(state.uri(&b).is_some());
        assert!(state.uri_mut(&a).is_none());
    }
}
