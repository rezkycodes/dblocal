//! Detect how DBLocal was installed, which determines whether self-update is
//! allowed and what apply strategy to use.

use std::path::{Path, PathBuf};

use crate::platform::Os;

use super::error::{UpdateError, UpdateResult};
use super::model::UpdateAssetKind;

/// How the running binary was installed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstallKind {
    /// Linux AppImage (`$APPIMAGE` points at the mounted file).
    AppImage { appimage_path: PathBuf },
    /// Portable tarball or per-user `install.sh` binary on Linux.
    PortableTarball { exe_path: PathBuf },
    /// Windows zip/portable install; binary can be swapped in place or via helper.
    WindowsZip { exe_path: PathBuf },
    /// Windows MSI install (`%LOCALAPPDATA%\DBLocal` is per-user and self-updatable;
    /// `Program Files` is per-machine and refused without elevation).
    WindowsMsi { exe_path: PathBuf },
    /// macOS `.app` bundle (self-update deferred).
    MacApp { bundle_path: PathBuf },
    /// Installed by a distro package manager (`/usr/bin/dblocal`).
    DistPackage {
        exe_path: PathBuf,
        hint: PackageManagerHint,
    },
    /// Installed by Homebrew.
    Homebrew { exe_path: PathBuf },
    /// Could not determine, development build, or read-only location; self-update refused.
    Unknown { exe_path: PathBuf, reason: String },
}

impl InstallKind {
    /// Returns true when this install can be self-updated by DBLocal without elevation.
    pub fn can_self_update(&self) -> bool {
        match self {
            Self::AppImage { .. } | Self::PortableTarball { .. } | Self::WindowsZip { .. } => true,
            Self::WindowsMsi { exe_path } => is_per_user_windows_msi(exe_path),
            Self::MacApp { .. }
            | Self::DistPackage { .. }
            | Self::Homebrew { .. }
            | Self::Unknown { .. } => false,
        }
    }

    /// Returns the preferred release asset kind matching this install method.
    pub fn expected_asset_kind(&self) -> Option<UpdateAssetKind> {
        match self {
            Self::AppImage { .. } => Some(UpdateAssetKind::AppImage),
            Self::PortableTarball { .. } => Some(UpdateAssetKind::TarGz),
            Self::WindowsZip { .. } => Some(UpdateAssetKind::Zip),
            Self::WindowsMsi { .. } => Some(UpdateAssetKind::Msi),
            Self::MacApp { .. } => Some(UpdateAssetKind::Dmg),
            Self::DistPackage { hint, .. } => match hint {
                PackageManagerHint::Dpkg => Some(UpdateAssetKind::Deb),
                PackageManagerHint::Rpm => Some(UpdateAssetKind::Rpm),
                _ => None,
            },
            Self::Homebrew { .. } | Self::Unknown { .. } => None,
        }
    }

    /// Returns the package-manager hint if self-update is disabled by manager.
    pub fn package_manager_hint(&self) -> Option<&PackageManagerHint> {
        match self {
            Self::DistPackage { hint, .. } => Some(hint),
            Self::Homebrew { .. } => Some(&PackageManagerHint::Homebrew),
            _ => None,
        }
    }

    /// Returns the executable or bundle path relevant to this kind.
    pub fn path(&self) -> &Path {
        match self {
            Self::AppImage { appimage_path } => appimage_path,
            Self::PortableTarball { exe_path }
            | Self::WindowsZip { exe_path }
            | Self::WindowsMsi { exe_path }
            | Self::DistPackage { exe_path, .. }
            | Self::Homebrew { exe_path }
            | Self::Unknown { exe_path, .. } => exe_path,
            Self::MacApp { bundle_path } => bundle_path,
        }
    }

    /// Human-readable refusal reason for install kinds that cannot self-update.
    pub fn refusal_reason(&self) -> Option<String> {
        match self {
            Self::DistPackage { hint, .. } => Some(hint.message()),
            Self::Homebrew { .. } => Some(PackageManagerHint::Homebrew.message()),
            Self::MacApp { .. } => Some(
                "macOS self-update is not supported yet. Download the latest release from GitHub."
                    .to_owned(),
            ),
            Self::WindowsMsi { exe_path } if !is_per_user_windows_msi(exe_path) => Some(
                "This per-machine Windows MSI install requires administrator privileges. Download and run the MSI installer manually."
                    .to_owned(),
            ),
            Self::Unknown { reason, .. } => Some(format!(
                "Automatic updates are disabled for this install location: {reason}"
            )),
            _ => None,
        }
    }

    /// Formats a refusal reason enriched with the release page URL and matching asset name
    /// when self-update is unavailable.
    pub fn cannot_update_reason_with(
        &self,
        release_url: Option<&str>,
        asset_name: Option<&str>,
    ) -> Option<String> {
        let base = self.refusal_reason()?;
        let mut out = base;
        if let Some(asset) = asset_name.filter(|s| !s.is_empty()) {
            out.push_str(&format!(" Matching asset: {asset}."));
        }
        if let Some(url) = release_url.filter(|s| !s.is_empty()) {
            out.push_str(&format!(" Release page: {url}"));
        }
        Some(out)
    }
}

/// Returns true when a Windows MSI path is a per-user install under `%LOCALAPPDATA%\DBLocal`.
pub fn is_per_user_windows_msi(exe_path: &Path) -> bool {
    let lower = path_to_lower(exe_path);
    lower.contains("\\appdata\\local\\dblocal\\") || lower.contains("/appdata/local/dblocal/")
}

/// User-facing hint for package-manager-managed installs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PackageManagerHint {
    /// Debian/Ubuntu apt/dpkg.
    Dpkg,
    /// Fedora/RHEL rpm.
    Rpm,
    /// Homebrew.
    Homebrew,
    /// Generic package manager.
    Other { manager: String },
}

impl PackageManagerHint {
    /// Hint telling the user where to update.
    pub fn message(&self) -> String {
        match self {
            Self::Dpkg => {
                "Updates are managed by the package manager (dpkg/apt). Update DBLocal through your package manager.".to_owned()
            }
            Self::Rpm => {
                "Updates are managed by the package manager (rpm/dnf/yum). Update DBLocal through your package manager.".to_owned()
            }
            Self::Homebrew => {
                "Updates are managed by Homebrew. Update DBLocal through Homebrew.".to_owned()
            }
            Self::Other { manager } => format!(
                "Updates are managed by the package manager ({manager}). Update DBLocal through your package manager."
            ),
        }
    }
}

/// Abstraction over package-database lookups for testability.
pub trait PackageOwnership {
    /// Returns the package manager owning `path`, if any.
    fn owner_of(&self, path: &Path) -> Option<PackageManagerHint>;
}

/// Production ownership probe: checks env and platform package databases.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemPackageOwnership;

impl PackageOwnership for SystemPackageOwnership {
    fn owner_of(&self, path: &Path) -> Option<PackageManagerHint> {
        detect_package_manager(path)
    }
}

/// Detect install kind for the current process.
pub fn detect_install_kind() -> UpdateResult<InstallKind> {
    let ownership = SystemPackageOwnership;
    detect_install_kind_with(current_exe_path()?, &ownership)
}

/// Detect install kind using an injected executable path and ownership probe.
pub fn detect_install_kind_with(
    exe_path: PathBuf,
    ownership: &dyn PackageOwnership,
) -> UpdateResult<InstallKind> {
    if let Some(kind) = detect_from_env(&exe_path) {
        return Ok(kind);
    }
    if let Some(hint) = ownership.owner_of(&exe_path) {
        return Ok(InstallKind::DistPackage { exe_path, hint });
    }
    Ok(detect_from_path(&exe_path))
}

/// Pure OS-parameterized path detection for cross-platform tests.
pub fn detect_for_os(exe_path: &Path, os: Os) -> InstallKind {
    match os {
        Os::Windows => detect_windows(exe_path),
        Os::Macos => detect_macos(exe_path),
        Os::Linux => detect_linux(exe_path),
    }
}

fn current_exe_path() -> UpdateResult<PathBuf> {
    std::env::current_exe().map_err(|source| UpdateError::ApplyFailed {
        reason: format!("could not determine the current executable path: {source}"),
    })
}

fn detect_from_env(exe_path: &Path) -> Option<InstallKind> {
    detect_from_env_with(exe_path, |k| std::env::var(k).ok())
}

fn detect_from_env_with(
    exe_path: &Path,
    lookup: impl Fn(&str) -> Option<String>,
) -> Option<InstallKind> {
    if let Some(appimage) = lookup("APPIMAGE") {
        let path = PathBuf::from(appimage);
        if !path.as_os_str().is_empty()
            && lookup("APPDIR").is_some()
            && exe_path_inside_appdir(exe_path, lookup("APPDIR").as_deref())
            && path.is_file()
        {
            return Some(InstallKind::AppImage {
                appimage_path: path,
            });
        }
    }
    if is_homebrew_path(exe_path) {
        return Some(InstallKind::Homebrew {
            exe_path: exe_path.to_path_buf(),
        });
    }
    None
}

fn exe_path_inside_appdir(exe_path: &Path, appdir: Option<&str>) -> bool {
    let Some(appdir) = appdir else {
        return false;
    };
    let appdir = PathBuf::from(appdir);
    if appdir.as_os_str().is_empty() {
        return false;
    }
    let exe = canonical_or_original(exe_path);
    let dir = canonical_or_original(&appdir);
    exe.starts_with(&dir)
}

fn detect_from_path(exe_path: &Path) -> InstallKind {
    detect_for_os(exe_path, Os::current())
}

fn detect_linux(exe_path: &Path) -> InstallKind {
    if let Some(reason) = linux_deny_reason(exe_path) {
        return InstallKind::Unknown {
            exe_path: exe_path.to_path_buf(),
            reason: reason.message(),
        };
    }
    if !looks_like_portable_linux(exe_path) {
        return InstallKind::Unknown {
            exe_path: exe_path.to_path_buf(),
            reason: "the Linux executable location is not a recognized portable install".to_owned(),
        };
    }
    InstallKind::PortableTarball {
        exe_path: exe_path.to_path_buf(),
    }
}

/// Managed system prefixes: a copy of DBLocal living here is owned by the
/// distribution, so self-update must not replace it. Matched at the start of
/// the canonical path only, so a portable copy under `~/opt` or `~/work/app`
/// stays updatable.
const DENIED_PREFIXES: [&str; 5] = ["/nix/store/", "/snap/", "/app/", "/usr/", "/opt/"];
/// Build/tooling suffixes: a binary inside a build tree or a cargo bin dir is
/// never the installed copy. Allowed to match anywhere in the path.
const DENIED_SUFFIXES: [&str; 6] = [
    "/.cargo/bin/",
    "/target/debug/",
    "/target/release/",
    "/debug/deps/",
    "/release/deps/",
    "/dblocal-target",
];

/// Why a Linux executable location cannot self-update. Typed so each rule can
/// be asserted on its own instead of only through "some refusal".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DenyReason {
    /// Path starts with a managed system prefix.
    ManagedPrefix { prefix: &'static str },
    /// Path contains a build/tooling directory suffix.
    BuildTooling { suffix: &'static str },
    /// The executable's directory is not writable by the current user.
    NotWritable,
}

impl DenyReason {
    /// Explanation naming the matched rule input.
    fn message(self) -> String {
        match self {
            Self::ManagedPrefix { prefix } => {
                format!("path is in a managed location: {prefix}")
            }
            Self::BuildTooling { suffix } => {
                format!("path is in a build/tooling location: {suffix}")
            }
            Self::NotWritable => {
                "the executable directory is not writable by the current user".to_owned()
            }
        }
    }
}

/// Linux self-update deny-list: paths where in-place binary replacement is not
/// safe or not the right delivery mechanism. Anything not recognised becomes
/// Unknown.
fn linux_deny_reason(exe_path: &Path) -> Option<DenyReason> {
    if let Some(reason) = path_rule_deny_reason(&canonical_or_original(exe_path)) {
        return Some(reason);
    }
    if !exe_path.parent().is_some_and(directory_is_writable) {
        return Some(DenyReason::NotWritable);
    }
    None
}

/// Pure prefix/suffix rules, applied to the canonicalised path.
fn path_rule_deny_reason(path: &Path) -> Option<DenyReason> {
    let lower = path_to_lower(path);
    if let Some(prefix) = DENIED_PREFIXES.into_iter().find(|p| lower.starts_with(p)) {
        return Some(DenyReason::ManagedPrefix { prefix });
    }
    DENIED_SUFFIXES
        .into_iter()
        .find(|s| lower.contains(s))
        .map(|suffix| DenyReason::BuildTooling { suffix })
}

/// Asks the OS whether `dir` is writable by the current user without creating
/// anything inside it. Detection must not leave a probe file in the user's
/// `bin` directory.
pub(super) fn directory_is_writable(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        nix::unistd::access(dir, nix::unistd::AccessFlags::W_OK).is_ok()
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(dir).is_ok_and(|meta| meta.is_dir() && !meta.permissions().readonly())
    }
}

fn looks_like_portable_linux(exe_path: &Path) -> bool {
    let Some(name) = exe_path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name != "dblocal" {
        return false;
    }
    exe_path.is_file()
}

fn canonical_or_original(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

const WINDOWS_DEV_MARKERS: [&str; 6] = [
    "\\.cargo\\bin\\",
    "\\target\\debug\\",
    "\\target\\release\\",
    "\\debug\\deps\\",
    "\\release\\deps\\",
    "\\dblocal-target",
];

fn detect_windows(exe_path: &Path) -> InstallKind {
    let lower = path_to_lower(exe_path).replace('/', "\\");
    if let Some(marker) = WINDOWS_DEV_MARKERS.into_iter().find(|m| lower.contains(m)) {
        return InstallKind::Unknown {
            exe_path: exe_path.to_path_buf(),
            reason: format!("path is in a development build location: {marker}"),
        };
    }
    if lower.contains("\\program files\\")
        || lower.contains("\\program files (x86)\\")
        || lower.contains("\\appdata\\local\\dblocal\\")
    {
        return InstallKind::WindowsMsi {
            exe_path: exe_path.to_path_buf(),
        };
    }
    InstallKind::WindowsZip {
        exe_path: exe_path.to_path_buf(),
    }
}

fn detect_macos(exe_path: &Path) -> InstallKind {
    if let Some(bundle) = mac_app_bundle_path(exe_path) {
        return InstallKind::MacApp {
            bundle_path: bundle,
        };
    }
    InstallKind::Unknown {
        exe_path: exe_path.to_path_buf(),
        reason: "macOS self-update is not supported yet".to_owned(),
    }
}

fn mac_app_bundle_path(exe_path: &Path) -> Option<PathBuf> {
    let mut current = exe_path;
    while let Some(parent) = current.parent() {
        if parent.extension().and_then(|e| e.to_str()) == Some("app") {
            return Some(parent.to_path_buf());
        }
        current = parent;
    }
    None
}

fn is_homebrew_path(path: &Path) -> bool {
    let lower = path_to_lower(path);
    lower.contains("/homebrew/")
        || lower.contains("/cellar/")
        || lower.contains("/opt/homebrew/")
        || lower.contains("/linuxbrew/")
}

fn path_to_lower(path: &Path) -> String {
    path.to_string_lossy().to_ascii_lowercase()
}

fn detect_package_manager(path: &Path) -> Option<PackageManagerHint> {
    match Os::current() {
        Os::Linux => detect_linux_package_manager(path),
        Os::Macos => None,
        Os::Windows => None,
    }
}

fn detect_linux_package_manager(path: &Path) -> Option<PackageManagerHint> {
    if command_reports_ownership("dpkg-query", &["-S", &path.to_string_lossy()]) {
        return Some(PackageManagerHint::Dpkg);
    }
    if command_reports_ownership("rpm", &["-qf", &path.to_string_lossy()]) {
        return Some(PackageManagerHint::Rpm);
    }
    None
}

fn command_reports_ownership(cmd: &str, args: &[&str]) -> bool {
    let Ok(output) = std::process::Command::new(cmd).args(args).output() else {
        return false;
    };
    output.status.success()
}

#[cfg(test)]
#[path = "kind/tests.rs"]
mod tests;
