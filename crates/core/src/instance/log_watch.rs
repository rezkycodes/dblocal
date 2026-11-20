//! Opt-in log streaming (`F-LOG-01` AC1).
//!
//! A follower runs only while someone is watching that instance, so an idle app
//! does no log I/O at all.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::model::InstanceId;
use crate::process::logs::{LogFollower, follow_interval};
use crate::service::ServiceEvent;

/// Running followers, one per watched instance.
#[derive(Clone, Default, Debug)]
pub struct LogWatchers {
    tasks: Arc<Mutex<HashMap<InstanceId, JoinHandle<()>>>>,
}

impl LogWatchers {
    /// Builds an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts following `path`; enabling twice is idempotent.
    pub fn enable(
        &self,
        id: InstanceId,
        path: PathBuf,
        events: broadcast::Sender<ServiceEvent>,
        handle: &tokio::runtime::Handle,
    ) {
        let Ok(mut tasks) = self.tasks.lock() else {
            return;
        };
        if tasks.contains_key(&id) {
            return;
        }
        let task_id = id.clone();
        let task = handle.spawn(async move {
            let mut follower = LogFollower::new(path, false).ok();
            let interval = follow_interval();
            loop {
                if let Some(follower) = follower.as_mut()
                    && let Ok(lines) = follower.poll()
                    && !lines.is_empty()
                {
                    let _ = events.send(ServiceEvent::LogLines {
                        id: task_id.clone(),
                        lines,
                    });
                }
                tokio::time::sleep(interval).await;
            }
        });
        tasks.insert(id, task);
    }

    /// Stops following `id`; disabling an unwatched instance is a no-op.
    pub fn disable(&self, id: &InstanceId) {
        let Ok(mut tasks) = self.tasks.lock() else {
            return;
        };
        if let Some(task) = tasks.remove(id) {
            task.abort();
        }
    }

    /// Aborts every follower, used when the service is torn down.
    pub fn stop_all(&self) {
        let Ok(mut tasks) = self.tasks.lock() else {
            return;
        };
        for (_, task) in tasks.drain() {
            task.abort();
        }
    }

    /// How many instances are currently watched (idle-check test and diagnostics).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn watched(&self) -> usize {
        self.tasks.lock().map(|tasks| tasks.len()).unwrap_or(0)
    }
}
