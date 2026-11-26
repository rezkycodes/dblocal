//! Self-update through `DblService` (`F-PKG-05`).
//!
//! Supports manual checks (`check_update`), cached 6-hour background checks
//! (`check_update_if_due`), auto-check setting persistence in SQLite `settings`,
//! asynchronous cancellable progress-reporting installs (`start_update`),
//! synchronous apply (`apply_update`), and safe restart (`restart_for_update`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::consts;
use crate::download::CancellationToken;
use crate::error::Result;
use crate::paths::DblPaths;
use crate::service::{
    AvailableUpdateView, OperationId, ServiceEvent, ServiceResult, UpdateStatus, UserFacingError,
};
use crate::update::{
    AvailableUpdate, Ed25519ReleaseVerifier, InstallKind, InstallPlan, InstallStep,
    ReleaseVerifier, RestartAction, UpdateCheck, UpdateCheckOptions, UpdateDownloadOptions,
    UpdateError, UpdateLock, UpdateProgress, UpdateProgressSink, UpdateResult, apply_staged_update,
    check, cleanup_stale_staging, download_update_with_progress, execute_restart,
    has_trusted_signing_keys, plan_install, take_last_update_failure,
};

use super::service::Inner;

/// One check's full result: the renderable status plus the domain values an
/// apply needs.
struct Checked {
    status: UpdateStatus,
    available: Option<AvailableUpdate>,
}

/// Cached update check persisted in SQLite `settings`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedUpdateCheck {
    current: String,
    available: Option<AvailableUpdate>,
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn signing_key_refusal_reason() -> Option<&'static str> {
    #[cfg(not(any(test, debug_assertions)))]
    if !has_trusted_signing_keys() {
        return Some(
            "Release signing public key is not configured in this build; automatic self-update is disabled.",
        );
    }
    #[cfg(any(test, debug_assertions))]
    let _ = has_trusted_signing_keys;
    None
}

fn build_status(
    current: &Version,
    available: Option<&AvailableUpdate>,
    kind: InstallKind,
) -> UpdateStatus {
    let signing_block = signing_key_refusal_reason();
    let can_update = kind.can_self_update() && signing_block.is_none();
    let view = available.map(|update| AvailableUpdateView {
        version: update.version().to_string(),
        tag: update.tag().to_owned(),
        release_notes: update.release_notes().to_owned(),
        release_url: update.release_url().to_owned(),
        asset_name: update.asset().name().to_owned(),
        size_bytes: update.asset().size_bytes(),
        can_self_update: can_update,
    });
    let cannot_update_reason = kind
        .cannot_update_reason_with(
            available.map(AvailableUpdate::release_url),
            available.map(|u| u.asset().name()),
        )
        .or_else(|| signing_block.map(str::to_owned));
    UpdateStatus {
        current: current.to_string(),
        available: view,
        package_manager_hint: kind
            .package_manager_hint()
            .map(crate::update::PackageManagerHint::message),
        cannot_update_reason,
        install_kind: kind,
    }
}

/// Runs a fresh update check against the fetcher and persists the cache.
fn run_fresh_check(inner: &Inner) -> std::result::Result<Checked, UpdateError> {
    let current = inner.current_version.clone();
    let kind =
        inner
            .install_kind_or_detect()
            .map_err(|err| UpdateError::UnsupportedInstallKind {
                reason: err.to_string(),
            })?;
    let mut options = UpdateCheckOptions::for_repo(inner.update_repo.clone())
        .with_platform(inner.os, inner.arch)
        .with_install_kind(kind.clone());
    #[cfg(any(test, debug_assertions))]
    if let Some(base) = &inner.update_api_base_url {
        options = options.with_api_base_url(base.clone());
    }

    let check_result =
        inner
            .handle()
            .block_on(check(&current, inner.update_fetcher.as_ref(), &options))?;
    let available = match check_result {
        UpdateCheck::UpToDate { .. } => None,
        UpdateCheck::Available(update) => Some(update),
    };
    save_cache(inner, &current, available.as_ref());
    let status = build_status(&current, available.as_ref(), kind);
    Ok(Checked { status, available })
}

fn load_fresh_cache(inner: &Inner, current: &Version, kind: &InstallKind) -> Option<Checked> {
    let last_checked = inner
        .db(|store| store.get_setting(consts::SETTING_UPDATE_LAST_CHECKED_AT))
        .ok()
        .flatten()
        .and_then(|s| s.parse::<u64>().ok())?;
    let age = now_unix_secs().saturating_sub(last_checked);
    if age >= consts::UPDATE_CHECK_INTERVAL_SECS {
        return None;
    }
    let raw = inner
        .db(|store| store.get_setting(consts::SETTING_UPDATE_CACHED_RELEASE))
        .ok()
        .flatten()?;
    let cached: CachedUpdateCheck = serde_json::from_str(&raw).ok()?;
    if cached.current != current.to_string() {
        return None;
    }
    // Never serve a stale cached release that is <= current or mismatches kind.
    let available = cached.available.filter(|u| {
        u.version() > current
            && kind
                .expected_asset_kind()
                .is_none_or(|expected| u.asset().kind() == expected)
    });
    let status = build_status(current, available.as_ref(), kind.clone());
    Some(Checked { status, available })
}

fn save_cache(inner: &Inner, current: &Version, available: Option<&AvailableUpdate>) {
    let payload = CachedUpdateCheck {
        current: current.to_string(),
        available: available.cloned(),
    };
    let Ok(json) = serde_json::to_string(&payload) else {
        return;
    };
    let ts = now_unix_secs().to_string();
    let _ = inner.db(|store| {
        store.set_setting(consts::SETTING_UPDATE_LAST_CHECKED_AT, &ts)?;
        store.set_setting(consts::SETTING_UPDATE_CACHED_RELEASE, &json)
    });
}

/// Reads whether automatic update checks are enabled (defaults to `true`).
pub(crate) fn auto_update_check_enabled(inner: &Inner) -> Result<bool> {
    let raw = inner.db(|store| store.get_setting(consts::SETTING_AUTO_UPDATE_CHECK))?;
    Ok(raw.as_deref() != Some("false"))
}

/// Persists whether automatic update checks are enabled.
pub(crate) fn set_auto_update_check(inner: &Inner, enabled: bool) -> Result<()> {
    let val = if enabled { "true" } else { "false" };
    inner.db(|store| store.set_setting(consts::SETTING_AUTO_UPDATE_CHECK, val))
}

/// Manual update check: always queries upstream and updates the cache.
pub(crate) fn check_update(inner: &Inner) -> ServiceResult<UpdateStatus> {
    let checked = run_fresh_check(inner).map_err(to_facing)?;
    emit_checked(inner, &checked.status);
    Ok(checked.status)
}

/// Background/startup update check: respects the auto-check setting, reuses a
/// fresh (< 6h) cache without hitting GitHub, and treats offline / rate-limit
/// errors quietly (`Ok(None)`).
pub(crate) fn check_update_if_due(inner: &Inner) -> ServiceResult<Option<UpdateStatus>> {
    if !auto_update_check_enabled(inner).unwrap_or(true) {
        return Ok(None);
    }
    let current = inner.current_version.clone();
    let kind = inner
        .install_kind_or_detect()
        .map_err(|err| UserFacingError::from(&err))?;
    if let Some(cached) = load_fresh_cache(inner, &current, &kind) {
        emit_checked(inner, &cached.status);
        return Ok(Some(cached.status));
    }
    match run_fresh_check(inner) {
        Ok(checked) => {
            emit_checked(inner, &checked.status);
            Ok(Some(checked.status))
        }
        Err(UpdateError::Network { reason } | UpdateError::RateLimited { reason }) => {
            tracing::debug!(reason = %reason, "background update check skipped quietly");
            Ok(None)
        }
        Err(err) => Err(to_facing(err)),
    }
}

fn emit_checked(inner: &Inner, status: &UpdateStatus) {
    inner.emit(ServiceEvent::UpdateChecked {
        version: status.available.as_ref().map(|a| a.version.clone()),
        can_self_update: status.install_kind.can_self_update(),
    });
}

/// Starts downloading, verifying, and installing an available update
/// asynchronously, emitting [`ServiceEvent::UpdateProgress`] and
/// [`ServiceEvent::OperationFinished`].
pub(crate) fn start_update(inner: &Arc<Inner>) -> ServiceResult<OperationId> {
    let checked = resolve_for_apply(inner)?;
    let kind = checked.status.install_kind.clone();
    if !kind.can_self_update() || signing_key_refusal_reason().is_some() {
        let reason = checked
            .status
            .cannot_update_reason
            .or_else(|| kind.refusal_reason())
            .or_else(|| signing_key_refusal_reason().map(str::to_owned))
            .unwrap_or_else(|| {
                "Automatic updates are not available for this install method.".to_owned()
            });
        return Err(UserFacingError::new(
            crate::update::CODE_UPDATE_UNSUPPORTED_KIND,
            reason,
        ));
    }
    let Some(update) = checked.available else {
        return Err(to_facing(UpdateError::DowngradeRefused {
            reason: "no newer version is available".to_owned(),
        }));
    };

    let staging_dir = inner.paths.root().join(consts::DIR_UPDATE_STAGING);
    let lock = UpdateLock::acquire(&staging_dir).map_err(to_facing)?;

    let op_id = inner.ops.next_id();
    let cancel = CancellationToken::new();
    let _ = inner.ops.register(&op_id, cancel.clone());

    let inner_bg = Arc::clone(inner);
    let op_bg = op_id.clone();
    inner.handle().spawn(async move {
        let _lock = lock;
        let inner_sink = Arc::clone(&inner_bg);
        let op_sink = op_bg.clone();
        let sink: UpdateProgressSink = Arc::new(move |progress: UpdateProgress| {
            inner_sink.emit(ServiceEvent::UpdateProgress {
                id: op_sink.clone(),
                progress,
            });
        });

        let installer = Arc::clone(&inner_bg.update_installer);
        let update_bg = update.clone();
        let kind_bg = kind.clone();
        let cancel_bg = cancel.clone();
        let sink_bg = Arc::clone(&sink);
        let outcome = tokio::task::spawn_blocking(move || {
            installer.install_with_progress(&update_bg, &kind_bg, &cancel_bg, Some(&sink_bg))
        })
        .await
        .map_err(|err| UpdateError::ApplyFailed {
            reason: format!("update task panicked: {err}"),
        })
        .and_then(|res| res);

        match outcome {
            Ok(action) => {
                let target = action.restart_target().to_path_buf();
                if let Ok(mut slot) = inner_bg.pending_restart.lock() {
                    *slot = Some(action);
                }
                inner_bg.emit(ServiceEvent::UpdateProgress {
                    id: op_bg.clone(),
                    progress: UpdateProgress::ReadyToRestart {
                        version: update.version().to_string(),
                        restart_target: target,
                    },
                });
                inner_bg.ops.finish(&op_bg, Ok(()));
                inner_bg.emit(ServiceEvent::OperationFinished {
                    id: op_bg,
                    result: Ok(()),
                });
            }
            Err(UpdateError::Cancelled) => {
                let facing = to_facing(UpdateError::Cancelled);
                inner_bg.emit(ServiceEvent::UpdateProgress {
                    id: op_bg.clone(),
                    progress: UpdateProgress::Cancelled,
                });
                inner_bg.ops.finish(&op_bg, Err(facing.clone()));
                inner_bg.emit(ServiceEvent::OperationFinished {
                    id: op_bg,
                    result: Err(facing),
                });
            }
            Err(err) => {
                let facing = to_facing(err);
                inner_bg.emit(ServiceEvent::UpdateProgress {
                    id: op_bg.clone(),
                    progress: UpdateProgress::Failed {
                        code: facing.code.clone(),
                        message: facing.message.clone(),
                    },
                });
                inner_bg.ops.finish(&op_bg, Err(facing.clone()));
                inner_bg.emit(ServiceEvent::OperationFinished {
                    id: op_bg,
                    result: Err(facing),
                });
            }
        }
    });

    Ok(op_id)
}

/// Applies an available update synchronously, refusing a managed install (`F-PKG-05` AC2).
pub(crate) fn apply_update(inner: &Inner) -> ServiceResult<()> {
    let checked = run_fresh_check(inner).map_err(to_facing)?;
    let kind = &checked.status.install_kind;
    if !kind.can_self_update() || signing_key_refusal_reason().is_some() {
        let reason = checked
            .status
            .cannot_update_reason
            .or_else(|| kind.refusal_reason())
            .or_else(|| signing_key_refusal_reason().map(str::to_owned))
            .unwrap_or_else(|| {
                "Automatic updates are not available for this install method.".to_owned()
            });
        return Err(UserFacingError::new(
            crate::update::CODE_UPDATE_UNSUPPORTED_KIND,
            reason,
        ));
    }
    let Some(update) = checked.available else {
        return Err(to_facing(UpdateError::DowngradeRefused {
            reason: "no newer version is available".to_owned(),
        }));
    };
    let staging_dir = inner.paths.root().join(consts::DIR_UPDATE_STAGING);
    let _lock = UpdateLock::acquire(&staging_dir).map_err(to_facing)?;
    let cancel = CancellationToken::new();
    let action = inner
        .update_installer
        .install_with_progress(&update, kind, &cancel, None)
        .map_err(to_facing)?;
    if let Ok(mut slot) = inner.pending_restart.lock() {
        *slot = Some(action);
    }
    Ok(())
}

/// Reads and clears any failure message persisted by the detached Windows helper.
pub(crate) fn take_pending_update_failure(inner: &Inner) -> Result<Option<String>> {
    let staging = inner.paths.root().join(consts::DIR_UPDATE_STAGING);
    Ok(take_last_update_failure(&staging))
}

/// Cleans up leftover `run-*` staging dirs from previous updates when no update
/// lock is currently held.
pub(crate) fn cleanup_stale_update_staging(paths: &DblPaths) {
    let staging = paths.root().join(consts::DIR_UPDATE_STAGING);
    if !staging.is_dir() {
        return;
    }
    if let Ok(_lock) = UpdateLock::acquire(&staging) {
        cleanup_stale_staging(&staging);
    }
}

/// Spawns the updated binary (or Windows helper) so the caller can exit cleanly.
pub(crate) fn restart_for_update(inner: &Inner) -> ServiceResult<()> {
    let pending = inner
        .pending_restart
        .lock()
        .ok()
        .and_then(|mut slot| slot.take());
    let action = match pending {
        Some(action) => action,
        None => {
            let kind = inner
                .install_kind_or_detect()
                .map_err(|err| UserFacingError::from(&err))?;
            RestartAction::Relaunch {
                exe_path: kind.path().to_path_buf(),
            }
        }
    };
    execute_restart(&action).map_err(to_facing)
}

fn resolve_for_apply(inner: &Inner) -> ServiceResult<Checked> {
    let current = inner.current_version.clone();
    let kind = inner
        .install_kind_or_detect()
        .map_err(|err| UserFacingError::from(&err))?;
    if let Some(cached) = load_fresh_cache(inner, &current, &kind)
        && cached.available.is_some()
    {
        return Ok(cached);
    }
    run_fresh_check(inner).map_err(to_facing)
}

/// Keeps an update module's stable code instead of flattening it into a generic
/// `DblError` code.
fn to_facing(err: UpdateError) -> UserFacingError {
    UserFacingError::new(err.code(), err.user_message())
}

/// Seam for the download/verify/apply tail of [`apply_update`] and [`start_update`].
///
/// Production replaces it with [`DownloadInstaller`]; tests inject a fake so the
/// service logic is provable offline.
pub trait UpdateInstaller: Send + Sync {
    /// Downloads, verifies and applies `update` for `kind`.
    fn install(&self, update: &AvailableUpdate, kind: &InstallKind) -> UpdateResult<()>;

    /// Progress-reporting, cancellable install returning the [`RestartAction`].
    fn install_with_progress(
        &self,
        update: &AvailableUpdate,
        kind: &InstallKind,
        cancel: &CancellationToken,
        on_progress: Option<&UpdateProgressSink>,
    ) -> UpdateResult<RestartAction> {
        let _ = (cancel, on_progress);
        self.install(update, kind)?;
        Ok(RestartAction::Relaunch {
            exe_path: kind.path().to_path_buf(),
        })
    }
}

/// Production installer: stage into a private dir under the home, verify, extract,
/// smoke-check, and atomically apply.
pub struct DownloadInstaller {
    handle: tokio::runtime::Handle,
    current_exe: PathBuf,
    staging: PathBuf,
    download_options: UpdateDownloadOptions,
    verifier: Arc<dyn ReleaseVerifier>,
    data_home_override: Option<PathBuf>,
}

impl DownloadInstaller {
    /// Builds an installer that stages under `paths`, enforcing Ed25519 release
    /// signature verification by default.
    pub fn new(paths: DblPaths, handle: tokio::runtime::Handle, current_exe: PathBuf) -> Self {
        let staging = paths.root().join(consts::DIR_UPDATE_STAGING);
        Self {
            handle,
            current_exe,
            staging,
            download_options: UpdateDownloadOptions::default(),
            verifier: Arc::new(Ed25519ReleaseVerifier::production()),
            data_home_override: None,
        }
    }

    /// Configures a custom [`ReleaseVerifier`] (for tests injecting throwaway Ed25519 keys).
    #[cfg(any(test, debug_assertions))]
    pub fn with_verifier(mut self, verifier: Arc<dyn ReleaseVerifier>) -> Self {
        self.verifier = verifier;
        self
    }

    /// Configures download options (used by debug/test harnesses for loopback servers).
    #[cfg(any(test, debug_assertions))]
    pub fn with_download_options(mut self, options: UpdateDownloadOptions) -> Self {
        self.download_options = options;
        self
    }

    /// Overrides `$XDG_DATA_HOME` for desktop/icon installation in tests.
    #[cfg(any(test, debug_assertions))]
    pub fn with_data_home_override(mut self, dir: PathBuf) -> Self {
        self.data_home_override = Some(dir);
        self
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
                    .unwrap_or_else(|| "this install kind does not support self-update".to_owned()),
            });
        }
        Ok(())
    }
}

impl UpdateInstaller for DownloadInstaller {
    fn install(&self, update: &AvailableUpdate, kind: &InstallKind) -> UpdateResult<()> {
        let cancel = CancellationToken::new();
        self.install_with_progress(update, kind, &cancel, None)?;
        Ok(())
    }

    fn install_with_progress(
        &self,
        update: &AvailableUpdate,
        kind: &InstallKind,
        cancel: &CancellationToken,
        on_progress: Option<&UpdateProgressSink>,
    ) -> UpdateResult<RestartAction> {
        self.preflight(update, kind)?;
        let staged = self.handle.block_on(download_update_with_progress(
            update,
            &self.staging,
            &self.download_options,
            self.verifier.as_ref(),
            cancel,
            on_progress,
        ))?;

        if cancel.is_cancelled() {
            staged.cleanup();
            return Err(UpdateError::Cancelled);
        }

        if let Some(sink) = on_progress {
            sink(UpdateProgress::Installing);
        }

        let res = apply_staged_update(
            kind,
            &staged,
            &self.current_exe,
            self.data_home_override.as_deref(),
        );
        match res {
            Ok(action) => {
                // Keep the staged file only when a deferred Windows helper needs it after exit,
                // and mark the run dir pending so startup cleanup in other processes protects it.
                if matches!(action, RestartAction::Relaunch { .. }) {
                    staged.cleanup();
                } else {
                    crate::update::mark_staging_pending(staged.run_dir());
                }
                Ok(action)
            }
            Err(err) => {
                staged.cleanup();
                Err(err)
            }
        }
    }
}

/// The running version, read from the crate metadata.
pub(crate) fn running_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|_| Version::new(0, 0, 0))
}
