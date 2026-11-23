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
    view.update(cx, |this, cx| this.open_settings(cx));
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
    view.update(cx, |this, cx| {
        this.open_settings(cx);
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
    view.update(cx, |this, cx| {
        this.open_settings(cx);
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
    view.update(cx, |this, cx| {
        this.open_settings(cx);
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
    view.update(cx, |this, cx| {
        this.open_settings(cx);
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

fn package_manager_status() -> UpdateStatus {
    UpdateStatus {
        current: "0.8.0".to_owned(),
        available: Some(dblocal_core::service::AvailableUpdateView {
            version: "0.9.0".to_owned(),
            tag: "v0.9.0".to_owned(),
            release_notes: "Catatan".to_owned(),
            asset_name: "dblocal-linux-x86_64.tar.gz".to_owned(),
            size_bytes: 1024,
            can_self_update: false,
        }),
        install_kind: InstallKind::DistPackage {
            exe_path: "/usr/bin/dblocal".into(),
            hint: dblocal_core::update::PackageManagerHint::Dpkg,
        },
        package_manager_hint: Some(dblocal_core::update::PackageManagerHint::Dpkg.message()),
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
        Err(UserFacingError::new("DBL-8502", "systemctl gagal"))
    }
    fn disable(&self) -> Result<login_item::LoginStatus, UserFacingError> {
        Ok(login_item::LoginStatus {
            enabled: false,
            location: "/tmp/fake/login-item".into(),
        })
    }
}
