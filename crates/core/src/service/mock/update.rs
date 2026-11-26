//! Deterministic update behaviour for [`super::MockService`] (`F-PKG-05`).
//!
//! The mock never reaches the network. It reports a configurable newer version
//! for the current platform and refuses `apply_update` / `start_update` for a
//! package-manager install with the same typed error the real service uses, so
//! a GUI built against the mock exercises every branch offline.

use std::sync::Arc;

use crate::service::{
    AvailableUpdateView, OperationId, ServiceEvent, ServiceResult, UpdateStatus, UserFacingError,
};
use crate::update::{InstallKind, UpdateError, UpdateProgress};

use super::{Inner, OpKind, lock};

/// The version the mock reports as available, when it reports one.
const MOCK_AVAILABLE_VERSION: &str = "0.9.0";

fn install_kind(inner: &Arc<std::sync::Mutex<Inner>>) -> InstallKind {
    let g = lock(inner);
    g.install_kind
        .clone()
        .unwrap_or_else(|| InstallKind::PortableTarball {
            exe_path: std::path::PathBuf::from("/opt/dblocal/dblocal"),
        })
}

pub(crate) fn check_update(inner: &Arc<std::sync::Mutex<Inner>>) -> ServiceResult<UpdateStatus> {
    let kind = install_kind(inner);
    let current = {
        let g = lock(inner);
        g.current_version.to_string()
    };
    let release_url = format!(
        "https://github.com/{}/releases/tag/v{MOCK_AVAILABLE_VERSION}",
        crate::consts::UPDATE_REPO
    );
    let asset_name = "dblocal-linux-x86_64.tar.gz".to_owned();
    let available = if current == MOCK_AVAILABLE_VERSION {
        None
    } else {
        Some(AvailableUpdateView {
            version: MOCK_AVAILABLE_VERSION.to_owned(),
            tag: format!("v{MOCK_AVAILABLE_VERSION}"),
            release_notes: "Sample release notes from the mock.".to_owned(),
            release_url: release_url.clone(),
            asset_name: asset_name.clone(),
            size_bytes: 42 * 1024 * 1024,
            can_self_update: kind.can_self_update(),
        })
    };
    let package_manager_hint = kind
        .package_manager_hint()
        .map(crate::update::PackageManagerHint::message);
    let cannot_update_reason = kind.cannot_update_reason_with(
        available.as_ref().map(|a| a.release_url.as_str()),
        available.as_ref().map(|a| a.asset_name.as_str()),
    );
    {
        let g = lock(inner);
        g.emit(ServiceEvent::UpdateChecked {
            version: available.as_ref().map(|a| a.version.clone()),
            can_self_update: kind.can_self_update(),
        });
    }
    Ok(UpdateStatus {
        current,
        available,
        install_kind: kind,
        package_manager_hint,
        cannot_update_reason,
    })
}

pub(crate) fn check_update_if_due(
    inner: &Arc<std::sync::Mutex<Inner>>,
) -> ServiceResult<Option<UpdateStatus>> {
    check_update(inner).map(Some)
}

pub(crate) fn start_update(inner: &Arc<std::sync::Mutex<Inner>>) -> ServiceResult<OperationId> {
    let kind = install_kind(inner);
    if !kind.can_self_update() {
        let err = UpdateError::UnsupportedInstallKind {
            reason: kind.refusal_reason().unwrap_or_else(|| {
                "Automatic updates are not available for this install method.".to_owned()
            }),
        };
        return Err(UserFacingError::new(err.code(), err.user_message()));
    }
    let op_id = {
        let mut g = lock(inner);
        let id = g.alloc_op(OpKind::Update);
        g.emit(ServiceEvent::UpdateProgress {
            id: id.clone(),
            progress: UpdateProgress::Downloading {
                received_bytes: 42 * 1024 * 1024,
                total_bytes: Some(42 * 1024 * 1024),
            },
        });
        g.emit(ServiceEvent::UpdateProgress {
            id: id.clone(),
            progress: UpdateProgress::Verifying,
        });
        g.emit(ServiceEvent::UpdateProgress {
            id: id.clone(),
            progress: UpdateProgress::Installing,
        });
        g.emit(ServiceEvent::UpdateProgress {
            id: id.clone(),
            progress: UpdateProgress::ReadyToRestart {
                version: MOCK_AVAILABLE_VERSION.to_owned(),
                restart_target: kind.path().to_path_buf(),
            },
        });
        g.emit(ServiceEvent::OperationFinished {
            id: id.clone(),
            result: Ok(()),
        });
        id
    };
    Ok(op_id)
}

pub(crate) fn apply_update(inner: &Arc<std::sync::Mutex<Inner>>) -> ServiceResult<()> {
    let kind = install_kind(inner);
    if !kind.can_self_update() {
        let err = UpdateError::UnsupportedInstallKind {
            reason: kind.refusal_reason().unwrap_or_else(|| {
                "Automatic updates are not available for this install method.".to_owned()
            }),
        };
        return Err(UserFacingError::new(err.code(), err.user_message()));
    }
    Ok(())
}
