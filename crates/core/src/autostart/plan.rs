//! Pure decisions for autostart mode (F-AUT-01 AC3, F-AUT-02).
//!
//! The functions here decide *what* to do without touching the service, the
//! tray, or the filesystem. The GUI bootstrap and the CLI call them, then act
//! on the returned value; the tests pin the decisions without starting any
//! engine or opening any window.

use crate::model::InstanceStatus;
use crate::service::InstanceView;

/// What the app should do when it was started by the login item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutostartMode {
    /// No tray is available, but at least one flagged instance is starting or
    /// was started: keep the process alive headless so the engines stay up.
    HeadlessKeepAlive,
    /// No tray, no flagged instance to start, nothing to keep: exit quietly
    /// with a clear message instead of surprising the user with a window.
    HeadlessExit,
    /// A tray host is present: go straight to the tray with no window (AC3).
    TrayOnly,
}

/// How the GUI login-mode process presents itself (D4).
///
/// Kept separate from [`AutostartMode`] because the GUI always has a display:
/// the only question is whether a tray host accepted the icon. When it did not,
/// the PRD `F-TRY-01` AC3 fallback applies — a full window, never an invisible
/// process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiPresentation {
    /// The tray accepted the icon: no window (AC3).
    TrayOnly,
    /// No tray host: open the main window (the documented fallback).
    Window,
}

/// Decides the GUI presentation for a login-mode launch.
///
/// A working tray means no window; without a tray the window is the only UI, so
/// it is opened rather than leaving an invisible 170 MB process that nothing can
/// quit (D4).
pub fn decide_gui_presentation(tray_available: bool) -> GuiPresentation {
    if tray_available {
        GuiPresentation::TrayOnly
    } else {
        GuiPresentation::Window
    }
}

/// Decides how `--autostart` should present itself.
///
/// The tray is preferred; a missing tray is not a reason to pop up a window
/// the user never asked for. We only keep a headless process when there is
/// actual work (a flagged instance) to keep alive.
pub fn decide_mode(tray_available: bool, flagged_instances: usize) -> AutostartMode {
    if tray_available {
        return AutostartMode::TrayOnly;
    }
    if flagged_instances > 0 {
        AutostartMode::HeadlessKeepAlive
    } else {
        AutostartMode::HeadlessExit
    }
}

/// A flagged instance that should be started during autostart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartTarget {
    /// Instance id to pass to `DblService::start_instance`.
    pub id: crate::model::InstanceId,
    /// Instance name, used for the per-instance stderr line and diagnostics.
    pub name: String,
    /// Engine, used to pick the start-wait bound (D1).
    pub engine: crate::model::EngineKey,
}

impl AutostartTarget {
    /// Builds a target.
    pub fn new(
        id: crate::model::InstanceId,
        name: impl Into<String>,
        engine: crate::model::EngineKey,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            engine,
        }
    }
}

/// Picks the instances that `--autostart` should start, in a stable order.
///
/// Only instances with `autostart == true` are returned. Instances already
/// running or in the middle of starting/stopping/provisioning are skipped so a
/// double-login or a fast restart never races a live engine. The order is the
/// order the service reported, which is stable by name in the real store.
pub fn flagged_targets(views: &[InstanceView]) -> Vec<AutostartTarget> {
    views
        .iter()
        .filter(|view| view.autostart && should_start_status(view.status))
        .map(|view| AutostartTarget {
            id: view.id.clone(),
            name: view.name.clone(),
            engine: view.engine,
        })
        .collect()
}

/// Whether an autostart-flagged instance in `status` should get a start
/// request.
///
/// `Stopped` and `Failed` are the only states where asking to start is useful
/// and safe: a running instance must be left alone, and a transitional one is
/// already owned by some operation.
pub fn should_start_status(status: InstanceStatus) -> bool {
    matches!(status, InstanceStatus::Stopped | InstanceStatus::Failed)
}

/// Outcome of one autostarted instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartOutcome {
    /// Instance that was attempted.
    pub target: AutostartTarget,
    /// What happened.
    pub result: Result<(), String>,
}

impl AutostartOutcome {
    /// A successful start request/completion.
    pub fn ok(target: AutostartTarget) -> Self {
        Self {
            target,
            result: Ok(()),
        }
    }

    /// A failed start, carrying a user-safe diagnostic string.
    pub fn err(target: AutostartTarget, message: impl Into<String>) -> Self {
        Self {
            target,
            result: Err(message.into()),
        }
    }

    /// Whether the start failed.
    pub fn is_err(&self) -> bool {
        self.result.is_err()
    }
}

/// Summary of one `--autostart` pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AutostartSummary {
    /// One entry per flagged instance, in the order they were attempted.
    pub outcomes: Vec<AutostartOutcome>,
}

impl AutostartSummary {
    /// Number of instances that started successfully.
    pub fn succeeded(&self) -> usize {
        self.outcomes.iter().filter(|o| o.result.is_ok()).count()
    }

    /// Number of instances that failed to start.
    pub fn failed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.is_err()).count()
    }

    /// Whether any instance ended up running or at least attempted.
    pub fn has_any_attempt(&self) -> bool {
        !self.outcomes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::model::{
        AuthMode, EngineKey, Instance, InstanceId, InstanceName, InstanceSpec, Port,
    };

    fn view(name: &str, status: InstanceStatus, autostart: bool) -> InstanceView {
        let spec = InstanceSpec::new(
            InstanceName::new(name).expect("name"),
            EngineKey::Postgresql,
            "18.0",
            Some(Port::new(5432).expect("port")),
            AuthMode::Password,
        )
        .expect("spec")
        .with_autostart(autostart);
        let inst = Instance::new(
            &spec,
            InstanceId::new(),
            status,
            std::time::SystemTime::UNIX_EPOCH,
        );
        InstanceView::from_instance(&inst)
    }

    #[test]
    fn tray_available_always_means_tray_only() {
        assert_eq!(decide_mode(true, 0), AutostartMode::TrayOnly);
        assert_eq!(decide_mode(true, 3), AutostartMode::TrayOnly);
    }

    #[test]
    fn headless_without_flagged_instances_exits() {
        assert_eq!(decide_mode(false, 0), AutostartMode::HeadlessExit);
    }

    #[test]
    fn gui_presentation_without_a_tray_opens_a_window() {
        // D4: no tray means the window is the only UI, never an invisible process.
        assert_eq!(decide_gui_presentation(true), GuiPresentation::TrayOnly);
        assert_eq!(decide_gui_presentation(false), GuiPresentation::Window);
    }

    #[test]
    fn a_target_carries_its_engine_for_the_wait_bound() {
        let views = vec![view("vk", InstanceStatus::Stopped, true)];
        let targets = flagged_targets(&views);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].engine, EngineKey::Postgresql);
    }

    #[test]
    fn headless_with_flagged_instances_keeps_alive() {
        assert_eq!(decide_mode(false, 1), AutostartMode::HeadlessKeepAlive);
        assert_eq!(decide_mode(false, 5), AutostartMode::HeadlessKeepAlive);
    }

    #[test]
    fn flagged_targets_only_pick_autostart_stopped_or_failed() {
        let views = vec![
            view("aa", InstanceStatus::Stopped, true),
            view("bb", InstanceStatus::Running, true),
            view("cc", InstanceStatus::Failed, true),
            view("dd", InstanceStatus::Stopped, false),
            view("ee", InstanceStatus::Starting, true),
            view("ff", InstanceStatus::Stopping, true),
        ];
        let targets = flagged_targets(&views);
        let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["aa", "cc"]);
    }

    #[test]
    fn should_start_status_is_narrow() {
        assert!(should_start_status(InstanceStatus::Stopped));
        assert!(should_start_status(InstanceStatus::Failed));
        assert!(!should_start_status(InstanceStatus::Running));
        assert!(!should_start_status(InstanceStatus::Starting));
        assert!(!should_start_status(InstanceStatus::Stopping));
        assert!(!should_start_status(InstanceStatus::Provisioning));
        assert!(!should_start_status(InstanceStatus::Initializing));
        assert!(!should_start_status(InstanceStatus::Deleting));
    }

    #[test]
    fn summary_counts_failures_independently() {
        let a = AutostartTarget::new(InstanceId::new(), "a", EngineKey::Postgresql);
        let b = AutostartTarget::new(InstanceId::new(), "b", EngineKey::Postgresql);
        let c = AutostartTarget::new(InstanceId::new(), "c", EngineKey::Postgresql);
        let summary = AutostartSummary {
            outcomes: vec![
                AutostartOutcome::ok(a),
                AutostartOutcome::err(b, "port"),
                AutostartOutcome::ok(c),
            ],
        };
        assert_eq!(summary.succeeded(), 2);
        assert_eq!(summary.failed(), 1);
        assert!(summary.has_any_attempt());
    }

    #[test]
    fn an_empty_summary_has_no_attempt() {
        let summary = AutostartSummary::default();
        assert!(!summary.has_any_attempt());
        assert_eq!(summary.succeeded(), 0);
        assert_eq!(summary.failed(), 0);
    }
}
