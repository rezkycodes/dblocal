//! The `crate::format` surface the tray model calls, copied in shape from
//! `crates/gui/src/format/mod.rs`.

use dblocal_core::model::{EngineKey, InstanceStatus};
use dblocal_core::service::InstanceView;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusGlyph {
    Running,
    Stopped,
    Failed,
    Busy,
}

pub fn status_glyph(status: InstanceStatus) -> StatusGlyph {
    match status {
        InstanceStatus::Running => StatusGlyph::Running,
        InstanceStatus::Failed => StatusGlyph::Failed,
        InstanceStatus::Provisioning
        | InstanceStatus::Initializing
        | InstanceStatus::Starting
        | InstanceStatus::Stopping
        | InstanceStatus::Deleting => StatusGlyph::Busy,
        _ => StatusGlyph::Stopped,
    }
}

pub fn status_label(status: InstanceStatus) -> &'static str {
    match status {
        InstanceStatus::Running => dblocal_core::strings::STATUS_RUNNING,
        InstanceStatus::Stopped => dblocal_core::strings::STATUS_STOPPED,
        InstanceStatus::Failed => dblocal_core::strings::STATUS_FAILED,
        InstanceStatus::Provisioning => dblocal_core::strings::STATUS_PROVISIONING,
        InstanceStatus::Initializing => dblocal_core::strings::STATUS_INITIALIZING,
        InstanceStatus::Starting => dblocal_core::strings::STATUS_STARTING,
        InstanceStatus::Stopping => dblocal_core::strings::STATUS_STOPPING,
        InstanceStatus::Deleting => dblocal_core::strings::STATUS_STOPPING,
    }
}

pub fn can_start(status: InstanceStatus) -> bool {
    matches!(status, InstanceStatus::Stopped | InstanceStatus::Failed)
}

pub fn can_stop(status: InstanceStatus) -> bool {
    status == InstanceStatus::Running
}

pub struct EngineGroup<'a> {
    pub engine: EngineKey,
    pub label: &'static str,
    pub instances: Vec<&'a InstanceView>,
}

pub fn group_by_engine(instances: &[InstanceView]) -> Vec<EngineGroup<'_>> {
    let mut groups = Vec::new();
    for engine in EngineKey::ALL {
        let mut members: Vec<&InstanceView> = instances
            .iter()
            .filter(|view| view.engine == engine)
            .collect();
        members.sort_by(|a, b| a.name.cmp(&b.name));
        if members.is_empty() {
            continue;
        }
        groups.push(EngineGroup {
            engine,
            label: engine.display_name(),
            instances: members,
        });
    }
    groups
}
