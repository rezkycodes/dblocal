//! Self-update through `DblService` (`F-PKG-05`).
//!
//! `check_update` is the only place an update check runs, and only because a
//! caller asked: there is no timer and no startup check. The network goes
//! through the injected [`UpdateFetcher`], so tests stay offline.
//!
//! `apply_update` refuses a package-manager-owned install before it touches the
//! network, which is `F-PKG-05` AC2. The download/verify/apply tail is a single
//! injected [`UpdateInstaller`] so the service logic — the refusal, the
//! up-to-date answer, the event — is provable without a real GitHub release.

use std::path::PathBuf;

use semver::Version;

use crate::consts;
use crate::paths::DblPaths;
use crate::service::{
    AvailableUpdateView, ServiceEvent, ServiceResult, UpdateStatus, UserFacingError,
};
use crate::update::{
    AvailableUpdate, InstallKind, InstallPlan, InstallStep, UpdateCheck, UpdateCheckOptions,
    UpdateDownloadOptions, UpdateError, UpdateFetcher, UpdateResult, apply, check, download_update,
    plan_install,
};

use super::service::Inner;

/// One check's full result: the renderable status plus the domain values an
/// apply needs.
struct Checked {
    status: UpdateStatus,
    available: Option<AvailableUpdate>,
}

/// Runs the update check and maps every failure to its module-local code.
fn run_check(inner: &Inner) -> ServiceResult<Checked> {
    let current = inner.current_version.clone();
    let options =
        UpdateCheckOptions::for_repo(inner.update_repo.clone()).with_platform(inner.os, inner.arch);
    let kind = inner.install_kind_or_detect()?;
    let check_result = inner
        .handle()
        .block_on(check(&current, inner.update_fetcher.as_ref(), &options))
        .map_err(to_facing)?;
    let (available, view) = match check_result {
        UpdateCheck::UpToDate { .. } => (None, None),
        UpdateCheck::Available(update) => {
            let view = AvailableUpdateView {
                version: update.version().to_string(),
                tag: update.tag().to_owned(),
                release_notes: update.release_notes().to_owned(),
                asset_name: update.asset().name().to_owned(),
                size_bytes: update.asset().size_bytes(),
                can_self_update: kind.can_self_update(),
            };
            (Some(update), Some(view))
        }
    };
    let status = UpdateStatus {
        current: current.to_string(),
        available: view,
        package_manager_hint: kind
            .package_manager_hint()
            .map(crate::update::PackageManagerHint::message),
        install_kind: kind,
    };
    Ok(Checked { status, available })
}

pub(crate) fn check_update(inner: &Inner) -> ServiceResult<UpdateStatus> {
    let checked = run_check(inner)?;
    inner.emit(ServiceEvent::UpdateChecked {
        version: checked.status.available.as_ref().map(|a| a.version.clone()),
        can_self_update: checked.status.install_kind.can_self_update(),
    });
    Ok(checked.status)
}

/// Applies an available update, refusing a managed install (`F-PKG-05` AC2).
pub(crate) fn apply_update(inner: &Inner) -> ServiceResult<()> {
    let checked = run_check(inner)?;
    let kind = &checked.status.install_kind;
    if !kind.can_self_update() {
        // The hint names the package manager, so the user knows what to run.
        let reason = kind.refusal_reason().unwrap_or_else(|| {
            "Pembaruan otomatis tidak tersedia untuk cara pemasangan ini.".to_owned()
        });
        return Err(to_facing(UpdateError::UnsupportedInstallKind { reason }));
    }
    let Some(update) = checked.available else {
        return Err(to_facing(UpdateError::DowngradeRefused {
            reason: "tidak ada versi yang lebih baru".to_owned(),
        }));
    };
    inner
        .update_installer
        .install(&update, kind)
        .map_err(to_facing)
}

/// Keeps an update module's stable code instead of flattening it into a generic
/// `DblError` code.
fn to_facing(err: UpdateError) -> UserFacingError {
    UserFacingError::new(err.code(), err.user_message())
}

/// Seam for the download/verify/apply tail of [`apply_update`].
///
/// Production replaces it with [`DownloadInstaller`]; tests inject a fake so the
/// service logic is provable offline.
pub trait UpdateInstaller: Send + Sync {
    /// Downloads, verifies and applies `update` for `kind`.
    fn install(&self, update: &AvailableUpdate, kind: &InstallKind) -> UpdateResult<()>;
}

/// Production installer: stage into the home, then plan and apply.
pub struct DownloadInstaller {
    paths: DblPaths,
    handle: tokio::runtime::Handle,
    current_exe: PathBuf,
    staging: PathBuf,
}

impl DownloadInstaller {
    /// Builds an installer that stages under `paths`.
    pub fn new(paths: DblPaths, handle: tokio::runtime::Handle, current_exe: PathBuf) -> Self {
        let staging = paths.root().join(consts::DIR_UPDATE_STAGING);
        Self {
            paths,
            handle,
            current_exe,
            staging,
        }
    }

    /// Refuses a managed install before any byte is downloaded.
    fn preflight(&self, update: &AvailableUpdate, kind: &InstallKind) -> UpdateResult<()> {
        let placeholder = self.staging.join("dblocal-update.bin");
        let plan: InstallPlan =
            plan_install(kind, &placeholder, &self.current_exe, update.version())?;
        if plan
            .steps()
            .iter()
            .any(|step| matches!(step, InstallStep::Refuse { .. }))
        {
            return Err(UpdateError::UnsupportedInstallKind {
                reason: kind
                    .refusal_reason()
                    .unwrap_or_else(|| "jenis instalasi tidak mendukung self-update".to_owned()),
            });
        }
        Ok(())
    }
}

impl UpdateInstaller for DownloadInstaller {
    fn install(&self, update: &AvailableUpdate, kind: &InstallKind) -> UpdateResult<()> {
        let _ = &self.paths;
        self.preflight(update, kind)?;
        let staging = self.staging.clone();
        let options = UpdateDownloadOptions::default();
        let staged = self
            .handle
            .block_on(download_update(update, &staging, &options))?;
        let plan = plan_install(
            kind,
            staged.staged_path(),
            &self.current_exe,
            update.version(),
        )?;
        apply(&plan)
    }
}

/// The running version, read from the crate metadata.
pub(crate) fn running_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|_| Version::new(0, 0, 0))
}

/// The update fetcher's type name is used only in the service's `Debug`; this
/// helper keeps the import meaningful when the fetcher is the production one.
pub(crate) fn _fetcher_is_object_safe(_: &dyn UpdateFetcher) {}
