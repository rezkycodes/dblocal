//! Stand-in for `dblocal-core` with only the items the tray module touches.
//!
//! This exists so the Windows/macOS tray sources can be compiled for a
//! non-Linux target on a host where the real core cannot (aws-lc-sys needs a
//! mingw C compiler). The type shapes mirror `crates/core/src/service/view.rs`
//! and `crates/core/src/model`; nothing here is shipped.

pub mod strings {
    pub const APP_NAME: &str = "DBLocal";
    pub const UI_TRAY_OPEN: &str = "Open DBLocal";
    pub const UI_TRAY_QUIT: &str = "Quit";
    pub const UI_START: &str = "Start";
    pub const UI_STOP: &str = "Stop";
    pub const STATUS_RUNNING: &str = "running";
    pub const STATUS_STOPPED: &str = "stopped";
    pub const STATUS_FAILED: &str = "failed";
    pub const STATUS_PROVISIONING: &str = "provisioning";
    pub const STATUS_INITIALIZING: &str = "initializing";
    pub const STATUS_STARTING: &str = "starting";
    pub const STATUS_STOPPING: &str = "stopping";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
    Provisioning,
    Initializing,
    Deleting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKey {
    Postgresql,
    Mysql,
    Mariadb,
    Valkey,
}

impl EngineKey {
    pub const ALL: [EngineKey; 4] = [
        EngineKey::Postgresql,
        EngineKey::Mysql,
        EngineKey::Mariadb,
        EngineKey::Valkey,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            EngineKey::Postgresql => "PostgreSQL",
            EngineKey::Mysql => "MySQL",
            EngineKey::Mariadb => "MariaDB",
            EngineKey::Valkey => "Valkey",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceId(pub String);

impl std::fmt::Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub mod service {
    use super::{EngineKey, InstanceId, InstanceStatus};

    #[derive(Debug, Clone)]
    pub struct InstanceView {
        pub id: InstanceId,
        pub name: String,
        pub engine: EngineKey,
        pub status: InstanceStatus,
    }
}

pub mod model {
    pub use super::{EngineKey, InstanceId, InstanceStatus};
}
