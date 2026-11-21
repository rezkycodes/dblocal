//! Pure state reduction for the event stream.
//!
//! The workspace owns GPUI entities, but the *effect* of each service event on
//! the visible collections is ordinary data handling. Keeping it here means it
//! can be unit-tested without a window, an event loop, or a live service.

use dblocal_core::model::{DownloadProgress, InstanceId};
use dblocal_core::service::{InstanceView, OperationId, OperationStage, UserFacingError};

use crate::logs::LogStore;

/// Which inner tab of an instance is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InnerTab {
    /// Overview: engine, port, URI, actions.
    Overview,
    /// Logs: virtualised tail with follow and copy.
    Logs,
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
    /// the instance), and drops its per-instance log buffer. `InstanceRemoved`
    /// is the contract's way of saying "this row is gone": `InstanceChanged`
    /// cannot carry a view that no longer exists. An unknown id is a no-op.
    ///
    /// The caller still issues `watch_logs(id, false)`; that is I/O, not state.
    pub fn apply_removed(&mut self, id: &InstanceId, tabs: &mut Tabs, logs: &mut LogStore) {
        self.instances.retain(|view| &view.id != id);
        tabs.close_by_instance(id);
        logs.close(id);
    }

    /// The number of instances currently running.
    pub fn running_count(&self) -> usize {
        crate::format::running_count(&self.instances)
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

    #[test]
    fn upsert_replaces_by_id_and_appends_new() {
        let mut state = ShellState::default();
        let first = view("one", InstanceStatus::Stopped);
        let id = first.id.clone();
        state.upsert_instance(first);
        assert_eq!(state.instances.len(), 1);

        // Same id, new status: replace, do not duplicate.
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

        // A later sample updates the same row.
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
        // Neither instance sees the other's line.
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
    fn tabs_open_select_and_deduplicate() {
        let mut tabs = Tabs::default();
        let a = instance_id();
        assert_eq!(tabs.open_or_select(a.clone()), 0);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.selected_instance(), Some(&a));
        let b = instance_id();
        assert_eq!(tabs.open_or_select(b.clone()), 1);
        assert_eq!(tabs.len(), 2);
        // Re-opening an existing instance selects it, no duplicate.
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
        // Closing the selected last tab falls back to the remaining one.
        let closed = tabs.close(1).expect("tab 1 exists");
        assert_eq!(closed.instance, b);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.selected_index(), 0);
        assert!(tabs.selected_instance().is_none());
        // Closing the only tab resets cleanly.
        tabs.close(0);
        assert!(tabs.is_empty());
        assert_eq!(tabs.selected_index(), 0);
        assert!(tabs.selected_tab().is_none());
        assert!(tabs.close(0).is_none(), "out-of-range close is a no-op");
    }

    #[test]
    fn apply_removed_drops_row_tab_selection_and_log_buffer() {
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

        state.apply_removed(&gone.id, &mut tabs, &mut logs);

        assert_eq!(state.instances.len(), 1, "the removed row is gone");
        assert_eq!(state.instances[0].id, kept.id);
        assert!(tabs.open().iter().all(|tab| tab.instance != gone.id));
        assert!(tabs.selected_instance().is_none(), "selection cleared");
        assert!(logs.get(&gone.id).is_none(), "log buffer dropped");
        assert!(logs.get(&kept.id).is_some(), "other logs untouched");
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
        // The selection pointed at the removed instance, so it is cleared
        // rather than silently pointing at a different instance.
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

        state.apply_removed(&instance_id(), &mut tabs, &mut logs);

        assert_eq!(state.instances.len(), 1);
        assert_eq!(tabs.len(), 1);
        assert!(logs.get(&present.id).is_some());
    }

    #[test]
    fn running_count_reads_the_list() {
        let mut state = ShellState::default();
        state.upsert_instance(view("run", InstanceStatus::Running));
        state.upsert_instance(view("stop", InstanceStatus::Stopped));
        assert_eq!(state.running_count(), 1);
        state.replace_instances(vec![]);
        assert_eq!(state.running_count(), 0);
    }
}
