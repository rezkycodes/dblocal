//! Vulkan pre-flight.
//!
//! GPUI renders through Vulkan on Linux. Without a usable GPU driver/ICD the
//! process would panic or show a black window (W0-C evidence: `raw-panic` with
//! exit 101). This check fails early with actionable install instructions and a
//! dedicated exit code instead. It never panics.

use std::env;
use std::ffi::OsString;
use std::path::Path;

/// Exit code when Vulkan is unavailable (`DBLOCAL_E_VULKAN`).
pub const EXIT_VULKAN: u8 = 78;

/// Test-only escape hatch: set `DBLOCAL_SPIKE_SKIP_PREFLIGHT=1` to observe
/// GPUI's own behaviour. Never set in a release build.
const ENV_SKIP: &str = "DBLOCAL_SPIKE_SKIP_PREFLIGHT";

const DOCS_URL: &str = "docs/dev/linux-build-deps.md";

/// Returns `Ok(())` when Vulkan looks usable, or an actionable report when it
/// does not.
pub fn preflight() -> Result<(), String> {
    preflight_with_skip(env::var_os(ENV_SKIP).is_some())
}

/// The pre-flight body with the escape hatch decided by the caller, so the
/// skip branch is testable without mutating the process environment (which
/// `unsafe_code = deny` forbids).
pub fn preflight_with_skip(skip: bool) -> Result<(), String> {
    if skip {
        tracing::warn!("Vulkan pre-flight skipped ({ENV_SKIP} set)");
        return Ok(());
    }
    match detect() {
        Some(reason) => Err(report(&reason)),
        None => Ok(()),
    }
}

fn detect() -> Option<String> {
    for var in ["VK_ICD_FILENAMES", "VK_DRIVER_FILES"] {
        if let Some(value) = env::var_os(var)
            && let Some(problem) = invalid_override(var, &value)
        {
            return Some(problem);
        }
    }
    if !icd_manifest_present() {
        return Some(
            "no Vulkan ICD manifest in /usr/share/vulkan/icd.d, /etc/vulkan/icd.d \
             or $XDG_DATA_HOME/vulkan/icd.d"
                .to_owned(),
        );
    }
    probe_vulkan_adapters()
}

fn invalid_override(var: &str, value: &OsString) -> Option<String> {
    let text = value.to_string_lossy();
    if text.trim().is_empty() {
        return Some(format!("{var} is set but empty"));
    }
    let separator = if cfg!(windows) { ';' } else { ':' };
    let mut saw_existing = false;
    let mut missing: Vec<String> = Vec::new();
    for entry in text.split(separator) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if Path::new(entry).is_file() {
            saw_existing = true;
        } else {
            missing.push(entry.to_owned());
        }
    }
    if saw_existing {
        None
    } else if missing.is_empty() {
        Some(format!("{var} does not name any ICD manifest"))
    } else {
        Some(format!(
            "{var} points to missing ICD manifest(s): {}",
            missing.join(", ")
        ))
    }
}

fn icd_manifest_present() -> bool {
    let mut dirs: Vec<std::path::PathBuf> = vec![
        "/usr/share/vulkan/icd.d".into(),
        "/etc/vulkan/icd.d".into(),
        "/usr/local/share/vulkan/icd.d".into(),
    ];
    if let Some(data_home) = env::var_os("XDG_DATA_HOME") {
        dirs.push(std::path::PathBuf::from(data_home).join("vulkan/icd.d"));
    } else if let Some(home) = env::var_os("HOME") {
        dirs.push(std::path::PathBuf::from(home).join(".local/share/vulkan/icd.d"));
    }
    dirs.iter().any(|dir| {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries.flatten().any(|entry| {
                    entry
                        .path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                })
            })
            .unwrap_or(false)
    })
}

/// Enumerates wgpu Vulkan adapters; returns a reason when none is usable.
fn probe_vulkan_adapters() -> Option<String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut future = std::pin::pin!(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    let adapters = match std::future::Future::poll(future.as_mut(), &mut context) {
        std::task::Poll::Ready(adapters) => adapters,
        std::task::Poll::Pending => {
            return Some("wgpu adapter enumeration did not complete synchronously".to_owned());
        }
    };
    if adapters.is_empty() {
        return Some(
            "wgpu enumerated zero Vulkan adapters, so no GPU driver is loaded for this session"
                .to_owned(),
        );
    }
    None
}

fn report(reason: &str) -> String {
    let mut out = String::new();
    out.push_str("DBLocal tidak bisa dijalankan karena Vulkan tidak tersedia di sistem ini.\n\n");
    out.push_str(&format!("Terdeteksi: {reason}\n\n"));
    out.push_str("Sebab: antarmuka DBLocal memakai GPUI, yang membutuhkan driver/ICD Vulkan.\n\n");
    out.push_str("Cara memperbaiki (pasang driver Vulkan untuk GPU Anda):\n");
    out.push_str("  Fedora:  sudo dnf install mesa-vulkan-drivers vulkan-loader\n");
    out.push_str("  Ubuntu:  sudo apt install mesa-vulkan-drivers vulkan-tools\n");
    out.push_str("  Arch:    sudo pacman -S vulkan-icd-loader mesa-vulkan-drivers\n\n");
    out.push_str("Verifikasi setelah memasang:\n  vulkaninfo --summary\n\n");
    out.push_str(&format!("Bantuan lebih lanjut: {DOCS_URL}\n\n"));
    out.push_str(
        "Jika driver sudah terpasang tetapi DBLocal tetap gagal, pastikan ada berkas ICD JSON \
         di /usr/share/vulkan/icd.d dan VK_ICD_FILENAMES/VK_DRIVER_FILES tidak menunjuk path \
         yang salah.\n\n",
    );
    out.push_str(&format!("Kode keluar: {EXIT_VULKAN} (DBLOCAL_E_VULKAN).\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_missing_override_file() {
        let value = OsString::from("/nonexistent/icd.json");
        let reason = invalid_override("VK_ICD_FILENAMES", &value).expect("reason");
        assert!(reason.contains("VK_ICD_FILENAMES"));
        assert!(reason.contains("/nonexistent/icd.json"));
    }

    #[test]
    fn detects_empty_override() {
        let reason = invalid_override("VK_DRIVER_FILES", &OsString::from("")).expect("reason");
        assert!(reason.contains("empty"));
    }

    #[test]
    fn accepts_an_existing_override_file() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(
            invalid_override("VK_ICD_FILENAMES", &OsString::from(manifest.as_os_str())).is_none()
        );
    }

    #[test]
    fn manifest_presence_is_stable_within_a_run() {
        let first = icd_manifest_present();
        let second = icd_manifest_present();
        assert_eq!(first, second);
    }

    #[test]
    fn report_is_actionable_and_names_each_distro() {
        let text = report("no Vulkan ICD manifest found");
        assert!(text.contains("Terdeteksi: no Vulkan ICD manifest found"));
        for needle in ["Fedora", "Ubuntu", "Arch", "vulkaninfo", "vulkan-loader"] {
            assert!(text.contains(needle), "missing {needle}");
        }
        assert!(text.contains(DOCS_URL));
        assert!(text.ends_with(&format!("Kode keluar: {EXIT_VULKAN} (DBLOCAL_E_VULKAN).\n")));
    }

    #[test]
    fn skip_flag_disables_the_check() {
        assert!(preflight_with_skip(true).is_ok());
    }
}
