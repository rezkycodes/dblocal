//! Self-update: check GitHub releases, verify, and plan safe installation.
//!
//! This module owns the logic behind Settings -> Updates. It has no UI and no
//! timer; callers decide when to check and when to apply. Network access is
//! injected through traits so tests run offline against local HTTP servers.

mod check;
mod download;
mod error;
mod install;
mod kind;
mod model;
mod verify;

#[cfg(test)]
mod tests;

pub use check::{
    FetchResponse, MAX_CHECKSUMS_BODY_BYTES, MAX_RELEASE_NOTES_BYTES, MAX_RELEASES_BODY_BYTES,
    ReqwestUpdateFetcher, UpdateCheckOptions, UpdateFetcher, check, sanitize_release_notes,
};
pub use download::{
    ALLOWED_HOSTS, MAX_UPDATE_BYTES, StagedUpdate, UpdateDownloadOptions, download_and_verify,
    download_update, redirect_allowed, validate_url,
};
pub use error::{
    CODE_UPDATE_APPLY_FAILED, CODE_UPDATE_ASSET_MISSING, CODE_UPDATE_CHECKSUM,
    CODE_UPDATE_DOWNGRADE, CODE_UPDATE_MALFORMED, CODE_UPDATE_NETWORK, CODE_UPDATE_RATE_LIMITED,
    CODE_UPDATE_UNSUPPORTED_KIND, CODE_UPDATE_UP_TO_DATE, CODE_UPDATE_VERIFY_FAILED, UpdateError,
    UpdateResult,
};
pub use install::{
    InstallPlan, InstallStep, apply, cleanup_backup, plan_install, restore_backup, smoke_check,
    windows_helper_command,
};
pub use kind::{
    InstallKind, PackageManagerHint, PackageOwnership, detect_install_kind,
    detect_install_kind_with,
};
pub use model::{
    AvailableUpdate, ReleaseAsset, ReleaseChannel, UpdateAssetKind, UpdateCheck,
    platform_asset_tokens,
};
pub use verify::{Checksums, constant_time_eq_hex, sha256_file_hex, verify_sha256_file};
