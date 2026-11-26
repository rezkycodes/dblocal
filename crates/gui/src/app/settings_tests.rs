//! Settings dialog: handler tests (`F-PKG-05`, `F-AUT-01`).
//!
//! The update section runs over the scriptable service; the login-item section
//! drives a fake seam so the tests never touch the machine's real login items.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use dblocal_core::service::{DblService, UpdateStatus, UserFacingError};
use dblocal_core::update::InstallKind;
use gpui_kit::TestAppContext;

use crate::app::scripted_service::ScriptedService;
use crate::app::{Workspace, login_item};

fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::Entity<Workspace>,
    ScriptedService,
    &mut gpui_kit::VisualTestContext,
) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = ScriptedService::new();
    let svc: Arc<dyn DblService> = Arc::new(service.clone());
    let (view, cx) = cx.add_window_view(|window, cx| Workspace::new_preview(svc, window, cx));
    cx.run_until_parked();
    (view, service, cx)
}

fn settle(cx: &mut gpui_kit::VisualTestContext) {
    for _ in 0..100 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    cx.run_until_parked();
}

#[gpui_kit::test]
fn opening_settings_starts_with_no_answer(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update_in(cx, |this, window, cx| this.open_settings(window, cx));
    settle(cx);
    assert!(view.read_with(cx, |this, _| this.settings_state().open));
    assert!(
        view.read_with(cx, |this, _| this.settings_state().checked().is_none()),
        "opening must not run an update check (F-PKG-05 AC1)"
    );
}

#[gpui_kit::test]
fn check_now_shows_the_available_version(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    view.update_in(cx, |this, window, cx| {
        this.open_settings(window, cx);
        this.check_updates(cx);
    });
    settle(cx);
    let (version, can_install) = view.read_with(cx, |this, _| {
        let state = this.settings_state();
        (
            state
                .checked()
                .and_then(|s| s.available.as_ref().map(|a| a.version.clone())),
            crate::app::settings_state::can_install(state),
        )
    });
    assert_eq!(version.as_deref(), Some("0.9.0"));
    assert!(can_install, "a portable tarball may self-update");
    assert_eq!(service.counts().check_update, 1);
}

#[gpui_kit::test]
fn a_package_manager_install_shows_the_hint_not_the_install_button(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.update = Ok(package_manager_status());
    });
    view.update_in(cx, |this, window, cx| {
        this.open_settings(window, cx);
        this.check_updates(cx);
    });
    settle(cx);
    let (can_install, hint) = view.read_with(cx, |this, _| {
        let state = this.settings_state();
        (
            crate::app::settings_state::can_install(state),
            crate::app::settings_state::package_manager_hint(state).map(str::to_owned),
        )
    });
    assert!(!can_install, "a distro install must not self-update");
    assert!(hint.is_some(), "the manager hint replaces the button");
}

#[gpui_kit::test]
fn install_applies_the_update(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    view.update_in(cx, |this, window, cx| {
        this.open_settings(window, cx);
        this.check_updates(cx);
    });
    settle(cx);
    view.update(cx, |this, cx| this.install_update(cx));
    settle(cx);
    assert_eq!(service.counts().apply_update, 1);
    assert_eq!(
        view.read_with(cx, |this, _| this.settings_state().install.clone()),
        crate::app::settings_state::InstallState::Installed
    );
}

#[gpui_kit::test]
fn a_failed_check_is_a_typed_error(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.update = Err(UserFacingError::new("DBL-8003", "offline"));
    });
    view.update_in(cx, |this, window, cx| {
        this.open_settings(window, cx);
        this.check_updates(cx);
    });
    settle(cx);
    let code = view.read_with(cx, |this, _| {
        this.settings_state().check_error().map(|e| e.code.clone())
    });
    assert_eq!(code.as_deref(), Some("DBL-8003"));
}

#[gpui_kit::test]
fn enabling_the_login_item_records_only_the_confirmed_state(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    // Install a fake seam that refuses the enable.
    view.update(cx, |this, _| {
        this.login = Some(Ok(
            Arc::new(RefusingLoginItem) as Arc<dyn login_item::LoginItemControl>
        ));
    });
    view.update(cx, |this, cx| this.set_login_item(true, cx));
    settle(cx);
    let (enabled, error) = view.read_with(cx, |this, _| {
        let state = this.settings_state();
        (
            state.login_enabled(),
            state.login_error().map(|e| e.code.clone()),
        )
    });
    assert!(
        !enabled,
        "a failed enable must not show enabled (F-AUT-01 AC2)"
    );
    assert_eq!(error.as_deref(), Some("DBL-8502"));
}

/// F2: a failed disable must not flip an enabled switch off.
///
/// The unit was still enabled after `systemctl --user disable` failed; showing
/// the switch off claims a state the machine is not in.
#[gpui_kit::test]
fn a_failed_disable_keeps_the_switch_on_and_shows_the_error(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update(cx, |this, _| {
        this.login = Some(Ok(Arc::new(SwitchLoginItem::enabled_failing_disable())
            as Arc<dyn login_item::LoginItemControl>));
    });
    view.update(cx, |this, cx| this.read_login_status(cx));
    settle(cx);
    assert!(
        view.read_with(cx, |this, _| this.settings_state().login_enabled()),
        "the fixture starts enabled"
    );

    view.update(cx, |this, cx| this.set_login_item(false, cx));
    settle(cx);
    let (enabled, known, error) = view.read_with(cx, |this, _| {
        let state = this.settings_state();
        (
            state.login_enabled(),
            state.login_known().is_some(),
            state.login_error().map(|e| e.code.clone()),
        )
    });
    assert!(
        enabled,
        "a failed disable must keep showing the last confirmed on state"
    );
    assert!(known, "the confirmed state must survive the failure");
    assert_eq!(error.as_deref(), Some("DBL-8503"));
}

/// F2: a failed enable on a disabled item keeps the switch off.
#[gpui_kit::test]
fn a_failed_enable_keeps_the_switch_off(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update(cx, |this, _| {
        this.login = Some(Ok(
            Arc::new(RefusingLoginItem) as Arc<dyn login_item::LoginItemControl>
        ));
    });
    view.update(cx, |this, cx| this.read_login_status(cx));
    settle(cx);
    view.update(cx, |this, cx| this.set_login_item(true, cx));
    settle(cx);
    let (enabled, error) = view.read_with(cx, |this, _| {
        let state = this.settings_state();
        (
            state.login_enabled(),
            state.login_error().map(|e| e.code.clone()),
        )
    });
    assert!(!enabled, "a failed enable must leave the switch off");
    assert_eq!(error.as_deref(), Some("DBL-8502"));
}

/// F2: the switch keeps its value while a write is in flight.
#[gpui_kit::test]
fn a_login_write_in_flight_keeps_the_previous_value(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update(cx, |this, _| {
        this.login = Some(Ok(Arc::new(SwitchLoginItem::enabled_failing_disable())
            as Arc<dyn login_item::LoginItemControl>));
    });
    view.update(cx, |this, cx| this.read_login_status(cx));
    settle(cx);

    view.update(cx, |this, cx| {
        // Start the write without letting the background task land.
        this.set_login_item(false, cx);
    });
    let (busy, enabled) = view.read_with(cx, |this, _| {
        let state = this.settings_state();
        (
            matches!(state.login, crate::app::settings_state::LoginState::Busy(_)),
            state.login_enabled(),
        )
    });
    assert!(busy, "the write is in flight");
    assert!(enabled, "Busy must not flicker the switch off");
    settle(cx);
}

fn package_manager_status() -> UpdateStatus {
    UpdateStatus {
        current: "0.8.0".to_owned(),
        available: Some(dblocal_core::service::AvailableUpdateView {
            version: "0.9.0".to_owned(),
            tag: "v0.9.0".to_owned(),
            release_notes: "Notes".to_owned(),
            release_url: "https://github.com/rezkycodes/dblocal/releases/tag/v0.9.0".to_owned(),
            asset_name: "dblocal-linux-x86_64.tar.gz".to_owned(),
            size_bytes: 1024,
            can_self_update: false,
        }),
        install_kind: InstallKind::DistPackage {
            exe_path: "/usr/bin/dblocal".into(),
            hint: dblocal_core::update::PackageManagerHint::Dpkg,
        },
        package_manager_hint: Some(dblocal_core::update::PackageManagerHint::Dpkg.message()),
        cannot_update_reason: Some(dblocal_core::update::PackageManagerHint::Dpkg.message()),
    }
}

/// A login-item seam whose enable always fails, so the dialog must not claim
/// it worked.
struct RefusingLoginItem;

impl login_item::LoginItemControl for RefusingLoginItem {
    fn status(&self) -> Result<login_item::LoginStatus, UserFacingError> {
        Ok(login_item::LoginStatus {
            enabled: false,
            location: "/tmp/fake/login-item".into(),
        })
    }
    fn enable(&self) -> Result<login_item::LoginStatus, UserFacingError> {
        Err(UserFacingError::new("DBL-8502", "systemctl failed"))
    }
    fn disable(&self) -> Result<login_item::LoginStatus, UserFacingError> {
        Ok(login_item::LoginStatus {
            enabled: false,
            location: "/tmp/fake/login-item".into(),
        })
    }
}

/// A seam that reads as enabled and whose `disable` fails, the F2 case.
struct SwitchLoginItem {
    enabled: bool,
    fail_disable: bool,
}

impl SwitchLoginItem {
    fn enabled_failing_disable() -> Self {
        Self {
            enabled: true,
            fail_disable: true,
        }
    }

    fn status(&self) -> login_item::LoginStatus {
        login_item::LoginStatus {
            enabled: self.enabled,
            location: "/tmp/fake/login-item".into(),
        }
    }
}

impl login_item::LoginItemControl for SwitchLoginItem {
    fn status(&self) -> Result<login_item::LoginStatus, UserFacingError> {
        Ok(self.status())
    }
    fn enable(&self) -> Result<login_item::LoginStatus, UserFacingError> {
        Ok(login_item::LoginStatus {
            enabled: true,
            location: "/tmp/fake/login-item".into(),
        })
    }
    fn disable(&self) -> Result<login_item::LoginStatus, UserFacingError> {
        if self.fail_disable {
            // The unit is still enabled, exactly as the failed call leaves it.
            return Err(UserFacingError::new("DBL-8503", "disable failed"));
        }
        Ok(login_item::LoginStatus {
            enabled: false,
            location: "/tmp/fake/login-item".into(),
        })
    }
}
