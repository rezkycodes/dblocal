//! Pure state for the in-app update (`F-PKG-05`).
//!
//! The title-bar download icon, the update dialog and the Settings section all
//! read this one read model. It is plain data so every stage — available,
//! downloading, verifying, installing, ready, failed, cancelled and
//! cannot-self-update — is unit-tested without a window or a service.
//!
//! Nothing here starts work: the action handlers in `update_actions` call the
//! service, and `apply_progress` folds each `UpdateProgress` event into a phase.

use dblocal_core::service::{AvailableUpdateView, OperationId, UpdateStatus, UserFacingError};
use dblocal_core::update::{InstallKind, UpdateProgress};

/// What the update UI is showing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdatePhase {
    /// No newer release, or no check has found one: no icon, no dialog.
    #[default]
    Hidden,
    /// A manual check triggered from the dialog (e.g. "Try again" after a
    /// previous-run failure with no known release) is in flight.
    Checking,
    /// A manual check triggered from the dialog found no newer release.
    UpToDate,
    /// A newer release exists and this install may replace itself.
    Available,
    /// A newer release exists but this install cannot replace itself (deb, rpm,
    /// per-machine MSI, macOS, unknown). The icon shows; pressing explains.
    CannotSelfUpdate,
    /// The asset is downloading; the numbers are the latest sample.
    Downloading {
        /// Bytes received so far.
        received: u64,
        /// Expected total, when the server sent a length.
        total: Option<u64>,
    },
    /// The checksum of the download is being verified.
    Verifying,
    /// The verified files are being installed.
    Installing,
    /// Installed; the app is about to restart into `version`.
    Ready {
        /// Version that will run after the restart.
        version: String,
    },
    /// The update stopped with a stable code.
    Failed {
        /// Stable `DBL-xxxx` code.
        code: String,
        /// English explanation from the service.
        message: String,
    },
    /// The user cancelled while downloading. Nothing was changed.
    Cancelled,
}

impl UpdatePhase {
    /// Whether an update is downloading, verifying or installing right now.
    pub fn is_running(&self) -> bool {
        matches!(
            self,
            Self::Downloading { .. } | Self::Verifying | Self::Installing
        )
    }

    /// Whether a terminal outcome is on screen (failed or cancelled).
    pub fn is_stopped(&self) -> bool {
        matches!(self, Self::Failed { .. } | Self::Cancelled)
    }
}

/// The whole update read model.
#[derive(Debug, Clone, Default)]
pub struct UpdateState {
    /// The newer release the last check found.
    pub available: Option<AvailableUpdateView>,
    /// How this binary was installed, and therefore whether it may self-update.
    pub install_kind: Option<InstallKind>,
    /// The refusal reason the check returned, shown instead of the progress.
    pub cannot_reason: Option<String>,
    /// The operation `start_update` returned, while one is in flight.
    pub operation: Option<OperationId>,
    /// What the icon and the dialog show.
    pub phase: UpdatePhase,
    /// Whether the update dialog is open.
    pub dialog_open: bool,
    /// "Check for updates automatically", as the service last confirmed it.
    pub auto_check: Option<bool>,
    /// A write of the automatic-check setting is in flight.
    pub auto_check_busy: bool,
    /// The automatic-check write failed; the switch keeps the confirmed value.
    pub auto_check_error: Option<String>,
    /// Whether a manual check is in flight (the Settings "Check now" spinner).
    pub checking: bool,
    /// Whether any check returned an answer, so Settings can say "up to date".
    pub checked_once: bool,
    /// The last manual check's typed failure, shown under the button.
    pub check_error: Option<UserFacingError>,
    /// A failed restart, kept beside the `Ready` phase so the user can press
    /// "Restart now" instead of downloading the release again.
    pub restart_error: Option<UserFacingError>,
    /// The failure a previous run persisted (the detached Windows helper writes
    /// it after the app closed), read once at startup.
    ///
    /// It has no `DBL-` code and no known version: it is the helper's own
    /// sentence, so the dialog shows it verbatim.
    pub previous_failure: Option<String>,
}

impl UpdateState {
    /// Applies a completed check. `None` means the check was quiet (offline or
    /// rate limited) and must change nothing; a status without a newer release
    /// hides the icon and closes a finished dialog.
    pub fn apply_check(&mut self, status: Option<&UpdateStatus>) {
        let Some(status) = status else {
            return;
        };
        self.install_kind = Some(status.install_kind.clone());
        self.available = status.available.clone();
        self.checked_once = true;
        self.check_error = None;
        self.cannot_reason = status
            .cannot_update_reason
            .clone()
            .or_else(|| status.package_manager_hint.clone());
        match &status.available {
            None => {
                self.operation = None;
                if self.phase == UpdatePhase::Checking && self.dialog_open {
                    self.phase = UpdatePhase::UpToDate;
                } else {
                    self.phase = UpdatePhase::Hidden;
                    if self.previous_failure.is_none() {
                        self.dialog_open = false;
                    }
                }
            }
            Some(_) if self.phase.is_running() => {}
            Some(view) if view.can_self_update => {
                self.phase = UpdatePhase::Available;
            }
            Some(_) => self.phase = UpdatePhase::CannotSelfUpdate,
        }
    }

    /// The newer release, which the title-bar tooltip names.
    pub fn available_version(&self) -> Option<&str> {
        self.available.as_ref().map(|view| view.version.as_str())
    }

    /// Whether the title-bar download icon is on screen.
    ///
    /// Hidden with no newer release, and while nothing was checked. Shown for a
    /// newer release whether or not this install can replace itself: a
    /// package-manager install still needs to point at its release page.
    pub fn icon_visible(&self) -> bool {
        self.available.is_some() && !matches!(self.phase, UpdatePhase::Hidden)
    }

    /// Whether this install may replace its own binary right now.
    ///
    /// Reads the check's own flags (`AvailableUpdateView::can_self_update`),
    /// never the install kind alone: a release build with no signing key keeps
    /// a writable per-user install but still refuses to update. Stopped phases
    /// (`Failed`, `Cancelled`) still offer the action so Settings does not
    /// lose its button after an interrupted attempt.
    pub fn can_self_update(&self) -> bool {
        self.available
            .as_ref()
            .is_some_and(|view| view.can_self_update)
            && (self.phase == UpdatePhase::Available || self.phase.is_stopped())
    }

    /// Whether an update is running; the title-bar button then shows the
    /// dialog instead of starting a second update.
    pub fn running(&self) -> bool {
        self.phase.is_running()
    }

    /// Whether a second `start_update` must be refused.
    pub fn blocks_new_update(&self) -> bool {
        self.phase.is_running() || matches!(self.phase, UpdatePhase::Ready { .. })
    }

    /// Whether the dialog should draw.
    ///
    /// A previous failure is its own reason to draw: it can arrive with no
    /// newer release known at all.
    pub fn dialog_visible(&self) -> bool {
        self.dialog_open
            && (!matches!(self.phase, UpdatePhase::Hidden) || self.previous_failure.is_some())
    }

    /// Download percent, when the total is known.
    pub fn percent(&self) -> Option<u8> {
        match self.phase {
            UpdatePhase::Downloading {
                received,
                total: Some(total),
            } if total > 0 => Some((received.saturating_mul(100) / total).min(100) as u8),
            _ => None,
        }
    }

    /// Bytes received so far, for the dialog's size line.
    pub fn received_bytes(&self) -> Option<u64> {
        match self.phase {
            UpdatePhase::Downloading { received, .. } => Some(received),
            _ => None,
        }
    }

    /// Expected bytes, when the release declared them.
    pub fn total_bytes(&self) -> Option<u64> {
        match self.phase {
            UpdatePhase::Downloading { total, .. } => total,
            _ => self
                .available
                .as_ref()
                .map(|view| view.size_bytes)
                .filter(|size| *size > 0),
        }
    }

    /// The `received / total · percent` detail line.
    pub fn download_detail(&self) -> Option<String> {
        let received = self.received_bytes()?;
        let total = self.total_bytes();
        let percent = self.percent();
        Some(match (total, percent) {
            (Some(total), Some(percent)) => format!(
                "{} / {}  ·  {percent}%",
                crate::format::format_bytes(received),
                crate::format::format_bytes(total)
            ),
            (Some(total), None) => format!(
                "{} / {}",
                crate::format::format_bytes(received),
                crate::format::format_bytes(total)
            ),
            (None, _) => crate::format::format_bytes(received),
        })
    }

    /// Records the failure a previous run persisted, and opens the dialog so it
    /// is not silent. Called once at startup with `take_pending_update_failure`.
    pub fn record_previous_failure(&mut self, message: impl Into<String>) {
        let message = message.into();
        if message.trim().is_empty() {
            return;
        }
        self.previous_failure = Some(message);
        self.dialog_open = true;
    }

    /// Dismisses the previous-failure note (the dialog's "Close").
    pub fn dismiss_previous_failure(&mut self) {
        self.previous_failure = None;
    }

    /// Closes the update dialog, dismissing any one-time previous-failure note
    /// and resetting transient check-only phases when no release is known.
    pub fn dismiss_dialog(&mut self) {
        self.dialog_open = false;
        self.previous_failure = None;
        if self.available.is_none() {
            self.phase = UpdatePhase::Hidden;
        }
    }

    /// The previous-failure sentence, when one is on screen.
    pub fn previous_failure(&self) -> Option<&str> {
        self.previous_failure.as_deref()
    }

    /// The release page for the repository, used when no newer release is known
    /// (a previous failure has no release URL of its own).
    pub fn releases_page(&self) -> String {
        format!(
            "https://github.com/{}/releases",
            dblocal_core::consts::UPDATE_REPO
        )
    }

    /// The dialog headline for the current phase.
    pub fn headline(&self) -> String {
        use dblocal_core::strings;
        if self.previous_failure.is_some() && matches!(self.phase, UpdatePhase::Hidden) {
            return strings::UI_UPDATE_FAILED_PREVIOUS.to_owned();
        }
        let version = self.available_version().unwrap_or_default();
        match &self.phase {
            UpdatePhase::Hidden => String::new(),
            UpdatePhase::Checking => strings::UI_SETTINGS_CHECKING.to_owned(),
            UpdatePhase::UpToDate => strings::UI_SETTINGS_UP_TO_DATE.to_owned(),
            UpdatePhase::Available => strings::settings_available(version),
            UpdatePhase::CannotSelfUpdate => strings::UI_UPDATE_CANNOT.to_owned(),
            UpdatePhase::Downloading { .. } => strings::update_downloading(version),
            UpdatePhase::Verifying => strings::UI_UPDATE_VERIFYING.to_owned(),
            UpdatePhase::Installing => strings::update_installing(version),
            UpdatePhase::Ready { version } => strings::update_ready(version),
            UpdatePhase::Failed { .. } => strings::UI_UPDATE_FAILED.to_owned(),
            UpdatePhase::Cancelled => strings::UI_UPDATE_CANCELLED.to_owned(),
        }
    }

    /// The refusal reason `CannotSelfUpdate` explains, from the check.
    pub fn cannot_update_reason(&self) -> Option<&str> {
        if self.phase != UpdatePhase::CannotSelfUpdate {
            return None;
        }
        self.cannot_reason.as_deref()
    }

    /// The release page URL, when a newer release was found.
    pub fn release_url(&self) -> Option<&str> {
        self.available
            .as_ref()
            .map(|view| view.release_url.as_str())
            .filter(|url| !url.is_empty())
    }

    /// Whether the dialog offers "Cancel update" (downloading only: after that
    /// the files are already being replaced).
    pub fn can_cancel(&self) -> bool {
        matches!(self.phase, UpdatePhase::Downloading { .. })
    }

    /// Whether the dialog offers "Try again" (a stopped attempt, an available
    /// one that has not started, or a previous run's failure).
    ///
    /// A managed install never offers it: retrying cannot change the install
    /// method, so the release page is the only action there.
    pub fn can_retry(&self) -> bool {
        self.phase.is_stopped()
            || self.phase == UpdatePhase::Available
            || matches!(self.phase, UpdatePhase::Hidden) && self.previous_failure.is_some()
    }

    /// Whether the dialog offers "Restart now".
    pub fn can_restart(&self) -> bool {
        matches!(self.phase, UpdatePhase::Ready { .. })
    }

    /// Whether the dialog offers "Open release page".
    ///
    /// Never during a live download: the release page would compete with the
    /// one action that matters there.
    pub fn offer_release_page(&self) -> bool {
        !self.running()
            && (self.release_url().is_some()
                || matches!(
                    self.phase,
                    UpdatePhase::CannotSelfUpdate | UpdatePhase::Failed { .. }
                )
                || self.previous_failure.is_some())
    }

    /// Whether "Open release page" is the dialog's single primary action.
    ///
    /// True when no retry or restart is offered (managed installs and builds
    /// without a signing key); stays secondary whenever another primary action
    /// is present so the dialog never has two primary buttons.
    pub fn release_page_is_primary(&self) -> bool {
        self.offer_release_page() && !self.can_restart() && !self.can_retry()
    }

    /// Folds one progress event for `id`.
    ///
    /// An event for an operation this state does not own is ignored, so a stale
    /// sample can never move a fresh attempt.
    pub fn apply_progress(&mut self, id: &OperationId, progress: &UpdateProgress) {
        if self.operation.as_ref() != Some(id) {
            return;
        }
        match progress {
            UpdateProgress::Downloading {
                received_bytes,
                total_bytes,
            } => {
                self.previous_failure = None;
                self.phase = UpdatePhase::Downloading {
                    received: *received_bytes,
                    total: *total_bytes,
                };
                self.dialog_open = true;
            }
            UpdateProgress::Verifying => {
                self.previous_failure = None;
                self.phase = UpdatePhase::Verifying;
                self.dialog_open = true;
            }
            UpdateProgress::Installing => {
                self.previous_failure = None;
                self.phase = UpdatePhase::Installing;
                self.dialog_open = true;
            }
            UpdateProgress::ReadyToRestart { version, .. } => {
                self.previous_failure = None;
                self.operation = None;
                self.phase = UpdatePhase::Ready {
                    version: version.clone(),
                };
                self.dialog_open = true;
            }
            UpdateProgress::Cancelled => {
                self.previous_failure = None;
                self.operation = None;
                self.phase = UpdatePhase::Cancelled;
                self.dialog_open = true;
            }
            UpdateProgress::Failed { code, message } => {
                self.previous_failure = None;
                self.operation = None;
                self.phase = UpdatePhase::Failed {
                    code: code.clone(),
                    message: message.clone(),
                };
                self.dialog_open = true;
            }
            // `UpdateProgress` is non-exhaustive; a stage this build does not
            // know leaves the current phase alone rather than guessing.
            _ => {}
        }
    }

    /// Starts a fresh attempt: remembers the operation before any event lands.
    pub fn begin_operation(&mut self, id: OperationId) {
        self.restart_error = None;
        self.previous_failure = None;
        self.operation = Some(id);
        self.phase = UpdatePhase::Downloading {
            received: 0,
            total: self.available.as_ref().map(|view| view.size_bytes),
        };
        self.dialog_open = true;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use dblocal_core::update::InstallKind;

    use super::*;

    fn view(can_self_update: bool) -> AvailableUpdateView {
        AvailableUpdateView {
            version: "0.9.0".to_owned(),
            tag: "v0.9.0".to_owned(),
            release_notes: "Notes".to_owned(),
            release_url: "https://github.com/rezkycodes/dblocal/releases/tag/v0.9.0".to_owned(),
            asset_name: "dblocal-linux-x86_64.tar.gz".to_owned(),
            size_bytes: 42 * 1024 * 1024,
            can_self_update,
        }
    }

    fn status(available: Option<AvailableUpdateView>, kind: InstallKind) -> UpdateStatus {
        UpdateStatus {
            current: "0.8.0".to_owned(),
            available,
            install_kind: kind,
            package_manager_hint: None,
            cannot_update_reason: None,
        }
    }

    fn portable() -> InstallKind {
        InstallKind::PortableTarball {
            exe_path: "/opt/dblocal/dblocal".into(),
        }
    }

    fn distro() -> InstallKind {
        InstallKind::DistPackage {
            exe_path: "/usr/bin/dblocal".into(),
            hint: dblocal_core::update::PackageManagerHint::Dpkg,
        }
    }

    /// A quiet check (offline or rate limited) changes nothing.
    #[test]
    fn a_quiet_check_keeps_the_previous_state() {
        let mut state = UpdateState::default();
        state.apply_check(None);
        assert!(!state.icon_visible());
        assert_eq!(state.phase, UpdatePhase::Hidden);

        let found = status(Some(view(true)), portable());
        state.apply_check(Some(&found));
        assert!(state.icon_visible());
        state.apply_check(None);
        assert_eq!(state.phase, UpdatePhase::Available);
    }

    /// No newer release: no icon, no dialog, and a found release is forgotten.
    #[test]
    fn up_to_date_hides_the_icon_and_closes_the_dialog() {
        let mut state = UpdateState::default();
        state.apply_check(Some(&status(Some(view(true)), portable())));
        state.dialog_open = true;
        state.apply_check(Some(&status(None, portable())));
        assert!(!state.icon_visible());
        assert!(!state.dialog_visible());
        assert!(state.available_version().is_none());
    }

    /// A newer release on a self-updatable install shows the icon.
    #[test]
    fn a_newer_release_shows_the_icon_and_names_the_version() {
        let mut state = UpdateState::default();
        state.apply_check(Some(&status(Some(view(true)), portable())));
        assert!(state.icon_visible());
        assert_eq!(state.available_version(), Some("0.9.0"));
        assert_eq!(state.phase, UpdatePhase::Available);
        assert!(
            !state.dialog_visible(),
            "the icon alone, until it is pressed"
        );
    }

    /// A package-manager install still shows the icon, and its phase refuses.
    #[test]
    fn a_managed_install_shows_the_icon_but_cannot_self_update() {
        let mut state = UpdateState::default();
        state.apply_check(Some(&status(Some(view(false)), distro())));
        assert!(state.icon_visible());
        assert_eq!(state.phase, UpdatePhase::CannotSelfUpdate);
        assert!(
            !state.can_retry(),
            "a managed install offers the release page, not a retry"
        );
        assert!(state.release_url().is_some());
        assert!(!state.can_cancel());
        assert!(!state.can_restart());
    }

    /// Download samples fill the numbers; cancel is offered only here.
    #[test]
    fn downloading_reports_percent_and_size() {
        let mut state = UpdateState::default();
        state.apply_check(Some(&status(Some(view(true)), portable())));
        let op = OperationId::new("op-update");
        state.begin_operation(op.clone());
        assert!(state.running());
        assert!(state.dialog_visible());
        assert_eq!(state.percent(), Some(0));

        state.apply_progress(
            &op,
            &UpdateProgress::Downloading {
                received_bytes: 21 * 1024 * 1024,
                total_bytes: Some(42 * 1024 * 1024),
            },
        );
        assert_eq!(state.percent(), Some(50));
        assert_eq!(state.received_bytes(), Some(21 * 1024 * 1024));
        assert!(
            state
                .download_detail()
                .is_some_and(|line| line.contains("50%") && line.contains('/')),
            "detail must carry size and percent"
        );
        assert!(state.can_cancel());
        assert!(!state.can_retry());
    }

    /// An unknown total still shows bytes received, without a percent.
    #[test]
    fn an_unknown_total_reports_bytes_only() {
        let mut state = UpdateState {
            available: Some(view(true)),
            ..UpdateState::default()
        };
        let op = OperationId::new("op");
        state.begin_operation(op.clone());
        state.apply_progress(
            &op,
            &UpdateProgress::Downloading {
                received_bytes: 1024,
                total_bytes: None,
            },
        );
        assert_eq!(state.percent(), None);
        assert_eq!(state.download_detail().as_deref(), Some("1.00 KB"));
    }

    /// Verify and install are their own phases, with no cancel.
    #[test]
    fn verifying_and_installing_are_separate_and_not_cancellable() {
        let mut state = UpdateState {
            available: Some(view(true)),
            ..UpdateState::default()
        };
        let op = OperationId::new("op");
        state.begin_operation(op.clone());

        state.apply_progress(&op, &UpdateProgress::Verifying);
        assert_eq!(state.phase, UpdatePhase::Verifying);
        assert!(!state.can_cancel(), "the download is over");
        assert!(state.running());

        state.apply_progress(&op, &UpdateProgress::Installing);
        assert_eq!(state.phase, UpdatePhase::Installing);
        assert!(state.running());
    }

    /// Ready carries the new version and offers the restart.
    #[test]
    fn ready_to_restart_offers_the_restart_and_blocks_a_second_update() {
        let mut state = UpdateState {
            available: Some(view(true)),
            ..UpdateState::default()
        };
        let op = OperationId::new("op");
        state.begin_operation(op.clone());
        state.apply_progress(
            &op,
            &UpdateProgress::ReadyToRestart {
                version: "0.9.0".to_owned(),
                restart_target: "/opt/dblocal/dblocal".into(),
            },
        );
        assert_eq!(
            state.phase,
            UpdatePhase::Ready {
                version: "0.9.0".to_owned()
            }
        );
        assert!(state.can_restart());
        assert!(state.blocks_new_update());
        assert!(!state.running(), "installing finished");
        assert_eq!(state.operation, None);
    }

    /// Failure carries the code and message and offers a retry.
    #[test]
    fn failure_carries_a_code_and_offers_a_retry() {
        let mut state = UpdateState {
            available: Some(view(true)),
            ..UpdateState::default()
        };
        let op = OperationId::new("op");
        state.begin_operation(op.clone());
        state.apply_progress(
            &op,
            &UpdateProgress::Failed {
                code: "DBL-8003".to_owned(),
                message: "The network connection failed.".to_owned(),
            },
        );
        assert!(matches!(state.phase, UpdatePhase::Failed { .. }));
        assert!(state.can_retry());
        assert!(!state.running());
        assert!(
            state.icon_visible(),
            "the release is still there to retry or to open"
        );
        assert_eq!(state.headline(), "The update did not finish.");
    }

    /// Cancel lands in its own phase and leaves the app usable.
    #[test]
    fn cancel_is_its_own_phase_and_offers_a_retry() {
        let mut state = UpdateState {
            available: Some(view(true)),
            ..UpdateState::default()
        };
        let op = OperationId::new("op");
        state.begin_operation(op.clone());
        state.apply_progress(&op, &UpdateProgress::Cancelled);
        assert_eq!(state.phase, UpdatePhase::Cancelled);
        assert!(state.can_retry());
        assert!(!state.running());
        assert_eq!(state.operation, None);
    }

    /// An event for another operation (or a stale one) is ignored.
    #[test]
    fn a_progress_event_for_another_operation_is_ignored() {
        let mut state = UpdateState {
            available: Some(view(true)),
            ..UpdateState::default()
        };
        let op = OperationId::new("mine");
        state.begin_operation(op);
        state.apply_progress(
            &OperationId::new("other"),
            &UpdateProgress::Failed {
                code: "DBL-8003".to_owned(),
                message: "x".to_owned(),
            },
        );
        assert!(
            state.running(),
            "a foreign failure must not stop our update"
        );
    }

    /// A running update is not reset by a check that returns the same release.
    #[test]
    fn a_check_during_a_download_does_not_reset_the_phase() {
        let mut state = UpdateState::default();
        state.apply_check(Some(&status(Some(view(true)), portable())));
        let op = OperationId::new("op");
        state.begin_operation(op.clone());
        state.apply_progress(
            &op,
            &UpdateProgress::Downloading {
                received_bytes: 10,
                total_bytes: Some(100),
            },
        );
        state.apply_check(Some(&status(Some(view(true)), portable())));
        assert_eq!(state.percent(), Some(10));
    }

    /// Starting a new attempt clears any persisted previous-run failure so the
    /// old warning never lingers beside a live download or ready state.
    #[test]
    fn begin_operation_clears_previous_failure() {
        let mut state = UpdateState::default();
        state.record_previous_failure("The previous DBLocal process did not exit.");
        state.apply_check(Some(&status(Some(view(true)), portable())));
        state.begin_operation(OperationId::new("op-retry"));
        assert!(
            state.previous_failure().is_none(),
            "a fresh attempt must clear the stale previous-failure note"
        );
    }

    /// A startup check that finds no newer release must not close a pending
    /// previous-failure note before the user has read and closed it.
    #[test]
    fn up_to_date_check_keeps_previous_failure_dialog_open() {
        let mut state = UpdateState::default();
        state.record_previous_failure("The previous DBLocal process did not exit.");
        assert!(state.dialog_visible());
        state.apply_check(Some(&status(None, portable())));
        assert!(
            state.dialog_visible(),
            "startup check with no release must not dismiss the previous-failure note"
        );
        assert!(state.previous_failure().is_some());
    }

    /// Settings keeps offering self-update after a failed or cancelled attempt,
    /// while hiding it for managed installs, missing signing keys, running or ready.
    #[test]
    fn can_self_update_includes_stopped_phases_only_for_updatable_installs() {
        let mut state = UpdateState::default();
        state.apply_check(Some(&status(Some(view(true)), portable())));
        assert!(state.can_self_update(), "available");

        let op = OperationId::new("op");
        state.begin_operation(op.clone());
        assert!(!state.can_self_update(), "downloading");
        state.apply_progress(&op, &UpdateProgress::Verifying);
        assert!(!state.can_self_update(), "verifying");
        state.apply_progress(&op, &UpdateProgress::Installing);
        assert!(!state.can_self_update(), "installing");

        state.apply_progress(
            &op,
            &UpdateProgress::Failed {
                code: "DBL-8003".to_owned(),
                message: "network error".to_owned(),
            },
        );
        assert!(
            state.can_self_update(),
            "failed attempt must still offer self-update in Settings"
        );

        let op2 = OperationId::new("op-2");
        state.begin_operation(op2.clone());
        state.apply_progress(&op2, &UpdateProgress::Cancelled);
        assert!(
            state.can_self_update(),
            "cancelled attempt must still offer self-update in Settings"
        );

        let op3 = OperationId::new("op-3");
        state.begin_operation(op3.clone());
        state.apply_progress(
            &op3,
            &UpdateProgress::ReadyToRestart {
                version: "0.9.0".to_owned(),
                restart_target: "/opt/dblocal/dblocal".into(),
            },
        );
        assert!(!state.can_self_update(), "ready to restart");

        let mut managed = UpdateState::default();
        managed.apply_check(Some(&status(Some(view(false)), distro())));
        assert!(!managed.can_self_update(), "managed install");

        let mut no_key = UpdateState::default();
        no_key.apply_check(Some(&status(Some(view(false)), portable())));
        assert!(!no_key.can_self_update(), "no signing key");
    }

    /// "Open release page" is primary only when it is the sole action (managed
    /// install or missing signing key), and secondary whenever retry or restart
    /// is present.
    #[test]
    fn release_page_is_primary_only_when_it_is_the_only_action() {
        let mut managed = UpdateState::default();
        managed.apply_check(Some(&status(Some(view(false)), distro())));
        assert!(managed.release_page_is_primary(), "managed install");

        let mut no_key = UpdateState::default();
        no_key.apply_check(Some(&status(Some(view(false)), portable())));
        assert!(no_key.release_page_is_primary(), "no signing key");

        let mut avail = UpdateState::default();
        avail.apply_check(Some(&status(Some(view(true)), portable())));
        assert!(
            !avail.release_page_is_primary(),
            "secondary alongside Try again (Available)"
        );

        let op = OperationId::new("op");
        avail.begin_operation(op.clone());
        assert!(!avail.offer_release_page(), "hidden while downloading");
        assert!(!avail.release_page_is_primary());

        avail.apply_progress(
            &op,
            &UpdateProgress::Failed {
                code: "DBL-8003".to_owned(),
                message: "boom".to_owned(),
            },
        );
        assert!(
            !avail.release_page_is_primary(),
            "secondary alongside Try again (Failed)"
        );

        let op2 = OperationId::new("op-2");
        avail.begin_operation(op2.clone());
        avail.apply_progress(
            &op2,
            &UpdateProgress::ReadyToRestart {
                version: "0.9.0".to_owned(),
                restart_target: "/opt/dblocal/dblocal".into(),
            },
        );
        assert!(
            !avail.release_page_is_primary(),
            "secondary alongside Restart now (Ready)"
        );

        let mut prev = UpdateState::default();
        prev.record_previous_failure("helper aborted");
        assert!(
            !prev.release_page_is_primary(),
            "secondary alongside Try again (previous failure)"
        );
    }
}
