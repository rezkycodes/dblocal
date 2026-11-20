//! Pure state reduction for the event stream.
//!
//! The workspace owns GPUI entities, but the *effect* of each service event on
//! the visible collections is ordinary data handling. Keeping it here means it
//! can be unit-tested without a window, an event loop, or a live service.

use dblocal_core::model::{DownloadProgress, InstanceId};
use dblocal_core::service::{InstanceView, OperationId, OperationStage, UserFacingError};

use crate::logs::LogStore;

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
