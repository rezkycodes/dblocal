//! Self-update: check GitHub releases, verify, and apply safely.
//!
//! This module owns the logic behind in-app and CLI self-updates. Network
//! access and release verification are injected through traits so tests run
//! offline against local HTTP servers and fake binaries.

mod check;
mod download;
mod error;
mod extract;
mod install;
mod kind;
mod lock;
mod model;
mod verify;

#[cfg(test)]
mod e2e_tests;
#[cfg(test)]
mod tests;

pub use check::{
    FetchResponse, MAX_CHECKSUMS_BODY_BYTES, MAX_RELEASE_NOTES_BYTES, MAX_RELEASES_BODY_BYTES,
    ReqwestUpdateFetcher, UpdateCheckOptions, UpdateFetcher, check, sanitize_release_notes,
};
pub use download::{
    ALLOWED_HOSTS, MAX_UPDATE_BYTES, StagedUpdate, UpdateDownloadOptions, UpdateProgressSink,
    download_and_verify, download_update, download_update_with_progress, redirect_allowed,
    validate_url,
};
pub use error::{
    CODE_UPDATE_APPLY_FAILED, CODE_UPDATE_ASSET_MISSING, CODE_UPDATE_CANCELLED,
    CODE_UPDATE_CHECKSUM, CODE_UPDATE_DISK_SPACE, CODE_UPDATE_DOWNGRADE, CODE_UPDATE_IN_PROGRESS,
    CODE_UPDATE_MALFORMED, CODE_UPDATE_NETWORK, CODE_UPDATE_RATE_LIMITED,
    CODE_UPDATE_SOURCE_NOT_FOUND, CODE_UPDATE_UNSAFE_ARCHIVE, CODE_UPDATE_UNSUPPORTED_KIND,
    CODE_UPDATE_UP_TO_DATE, CODE_UPDATE_VERIFY_FAILED, UpdateError, UpdateResult,
};
pub use extract::{
    DESKTOP_ENTRY_REL, ExtractedLinuxTarball, ICON_SIZES, MAX_UPDATE_ARCHIVE_ENTRIES,
    MAX_UPDATE_EXTRACTED_BYTES, SVG_ICON_REL, extract_linux_tarball, extract_windows_zip,
};
pub use install::{
    InstallPlan, InstallStep, WindowsHelperArgs, apply, apply_staged_update, cleanup_backup,
    execute_restart, plan_install, restore_backup, run_windows_apply_helper, smoke_check,
    windows_helper_command, windows_msi_helper_command,
};
pub use kind::{
    InstallKind, PackageManagerHint, PackageOwnership, detect_for_os, detect_install_kind,
    detect_install_kind_with, is_per_user_windows_msi,
};
pub use lock::UpdateLock;
pub use model::{
    AvailableUpdate, ReleaseAsset, ReleaseChannel, RestartAction, UpdateAssetKind, UpdateCheck,
    UpdateProgress, platform_asset_tokens,
};
pub use verify::{
    Checksums, ReleaseVerifier, Sha256ReleaseVerifier, constant_time_eq_hex, sha256_file_hex,
    verify_sha256_file,
};
