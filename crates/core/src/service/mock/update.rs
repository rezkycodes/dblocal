//! Deterministic update behaviour for [`super::MockService`] (`F-PKG-05`).
//!
//! The mock never reaches the network. It reports a configurable newer version
//! for the current platform and refuses `apply_update` for a package-manager
//! install with the same typed error the real service uses, so a GUI built
//! against the mock exercises every branch offline.

use std::sync::Arc;

use crate::service::{
    AvailableUpdateView, ServiceEvent, ServiceResult, UpdateStatus, UserFacingError,
};
use crate::update::{InstallKind, UpdateError};

use super::{Inner, lock};

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
    let available = if current == MOCK_AVAILABLE_VERSION {
        None
    } else {
        Some(AvailableUpdateView {
            version: MOCK_AVAILABLE_VERSION.to_owned(),
            tag: format!("v{MOCK_AVAILABLE_VERSION}"),
            release_notes: "Catatan rilis contoh dari mock.".to_owned(),
            asset_name: "dblocal-linux-x86_64.tar.gz".to_owned(),
            size_bytes: 42 * 1024 * 1024,
            can_self_update: kind.can_self_update(),
        })
    };
    let package_manager_hint = kind
        .package_manager_hint()
        .map(crate::update::PackageManagerHint::message);
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
    })
}

pub(crate) fn apply_update(inner: &Arc<std::sync::Mutex<Inner>>) -> ServiceResult<()> {
    let kind = install_kind(inner);
    if !kind.can_self_update() {
        let err = UpdateError::UnsupportedInstallKind {
            reason: kind.refusal_reason().unwrap_or_else(|| {
                "Pembaruan otomatis tidak tersedia untuk cara pemasangan ini.".to_owned()
            }),
        };
        return Err(UserFacingError::new(err.code(), err.user_message()));
    }
    Ok(())
}
