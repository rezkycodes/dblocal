use std::env;
use std::ffi::OsString;
use std::path::Path;

const DOCS_URL: &str =
    "https://github.com/db-local/dblocal/blob/main/docs/guides/linux-vulkan.md (placeholder)";

/// Pre-flight check for Vulkan availability.
///
/// GPUI renders through Vulkan on Linux. Without a working Vulkan ICD/loader the
/// process would panic or show a black window (see docs/spikes/gpui/README.md for
/// the observed behaviour). This check fails early with an actionable message and
/// a non-zero exit code instead.
pub fn preflight() -> Result<(), String> {
    // Escape hatch used by the spike to record what GPUI itself does when Vulkan is
    // missing (it must be observed, not guessed). Never set in a release build.
    if env::var_os("DBLOCAL_SPIKE_SKIP_PREFLIGHT").is_some() {
        eprintln!("PREFLIGHT skipped (DBLOCAL_SPIKE_SKIP_PREFLIGHT set)");
        return Ok(());
    }
    if let Some(reason) = detect_missing_vulkan() {
        return Err(format_report(&reason));
    }
    Ok(())
}

fn detect_missing_vulkan() -> Option<String> {
    // Honour explicit overrides first: they are the knobs used to simulate a
    // missing Vulkan stack and are what the driver loader itself consults.
    for var in ["VK_ICD_FILENAMES", "VK_DRIVER_FILES"] {
        if let Some(value) = env::var_os(var)
            && let Some(problem) = invalid_icd_override(var, &value)
        {
            return Some(problem);
        }
    }

    // No override names a usable manifest: require at least one on disk.
    if !icd_manifest_present() {
        return Some(
            "no Vulkan ICD manifest found in /usr/share/vulkan/icd.d, \
             /etc/vulkan/icd.d, or $XDG_DATA_HOME/vulkan/icd.d"
                .to_string(),
        );
    }

    // Manifests exist, but the loader/driver can still be broken (stale package,
    // wrong arch, driver crash). Only a real adapter enumeration proves usability.
    probe_vulkan_backends()
}

fn invalid_icd_override(var: &str, value: &OsString) -> Option<String> {
    let text = value.to_string_lossy();
    if text.trim().is_empty() {
        return Some(format!("{var} is set but empty"));
    }

    // Values may be a platform-specific list (':' on Unix, ';' on Windows).
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
            missing.push(entry.to_string());
        }
    }

    if saw_existing {
        None
    } else if missing.is_empty() {
        Some(format!("{var} is set but does not name any ICD manifest"))
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
                entries.flatten().any(|e| {
                    e.path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                })
            })
            .unwrap_or(false)
    })
}

/// Enumerates wgpu adapters (the same loader GPUI uses) and names the failure when
/// Vulkan produces none.
fn probe_vulkan_backends() -> Option<String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    // enumerate_adapters returns a Future on wgpu 30. wgpu-core resolves it in the
    // call, so polling it to completion immediately is sufficient and avoids taking
    // a dependency on a specific blocking executor crate.
    let adapters = std::future::Future::poll(
        std::pin::pin!(instance.enumerate_adapters(wgpu::Backends::VULKAN)),
        &mut std::task::Context::from_waker(std::task::Waker::noop()),
    );
    let adapters = match adapters {
        std::task::Poll::Ready(adapters) => adapters,
        std::task::Poll::Pending => {
            return Some(
                "wgpu adapter enumeration did not complete synchronously (unexpected)".to_string(),
            );
        }
    };
    if !adapters.is_empty() {
        return None;
    }

    // Vulkan produced nothing. wgpu may still fall back to GL, but GPUI panics when
    // that fallback cannot create a surface either, so treat "no Vulkan adapter" as
    // the actionable failure and tell the user what to install.
    Some(
        "wgpu enumerated zero Vulkan adapters, which means no usable GPU driver is \
         loaded for this session"
            .to_string(),
    )
}

fn format_report(reason: &str) -> String {
    let mut out = String::new();
    out.push_str("DBLocal cannot start because Vulkan is not available on this system.\n");
    out.push('\n');
    out.push_str(&format!("Detected: {reason}\n"));
    out.push('\n');
    out.push_str("Why: DBLocal's UI uses GPUI, which requires a working Vulkan driver/ICD.\n");
    out.push('\n');
    out.push_str("How to fix (install a Vulkan driver for your GPU):\n");
    out.push_str("  Fedora:  sudo dnf install mesa-vulkan-drivers vulkan-loader\n");
    out.push_str(
        "           # NVIDIA proprietary: install the matching akmod-nvidia / driver package\n",
    );
    out.push_str("  Ubuntu:  sudo apt install mesa-vulkan-drivers vulkan-tools\n");
    out.push_str("  Arch:    sudo pacman -S vulkan-icd-loader mesa-vulkan-drivers\n");
    out.push('\n');
    out.push_str("Verify after installing:\n");
    out.push_str("  vulkaninfo --summary   (or: vkcube)\n");
    out.push('\n');
    out.push_str(&format!("More help: {DOCS_URL}\n"));
    out.push('\n');
    out.push_str(
        "If a driver is installed but DBLocal still cannot start, ensure an ICD JSON exists in \
         /usr/share/vulkan/icd.d and that VK_ICD_FILENAMES/VK_DRIVER_FILES are not set to an \
         invalid path.\n",
    );
    out.push('\n');
    out.push_str("Exit code: 78 (DBLOCAL_E_VULKAN).\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn detects_missing_override_file() {
        let val = OsString::from("/nonexistent/icd.json");
        let reason = invalid_icd_override("VK_ICD_FILENAMES", &val).unwrap();
        assert!(reason.contains("VK_ICD_FILENAMES"));
        assert!(reason.contains("/nonexistent/icd.json"));
    }

    #[test]
    fn detects_empty_override() {
        let val = OsString::from("");
        let reason = invalid_icd_override("VK_DRIVER_FILES", &val).unwrap();
        assert!(reason.contains("empty"));
    }

    #[test]
    fn accepts_existing_override() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let val = OsString::from(manifest.as_os_str());
        assert!(invalid_icd_override("VK_ICD_FILENAMES", &val).is_none());
    }

    #[test]
    fn manifest_presence_is_a_plain_bool() {
        // Deliberately environment-dependent: on a machine with no ICD directory this
        // returns false, otherwise true. The point is that it must not panic or hang.
        let present = icd_manifest_present();
        let again = icd_manifest_present();
        assert_eq!(present, again);
    }

    #[test]
    fn report_echoes_the_detected_reason() {
        let msg = format_report("widget driver exploded");
        assert!(msg.contains("Detected: widget driver exploded"));
        assert!(msg.ends_with("Exit code: 78 (DBLOCAL_E_VULKAN).\n"));
    }

    #[test]
    fn report_names_every_major_distro() {
        let msg = format_report("x");
        for needle in ["Fedora", "Ubuntu", "Arch", "vulkan-loader"] {
            assert!(msg.contains(needle), "missing {needle} in report");
        }
    }

    #[test]
    fn preflight_is_a_no_op_when_the_skip_flag_is_set() {
        // The escape hatch must never produce an error, so the spike can observe GPUI's
        // own behaviour. Safe to set here: this test asserts only on the skip branch.
        // SAFETY: single-threaded test isolation for an env var the process owns.
        unsafe { std::env::set_var("DBLOCAL_SPIKE_SKIP_PREFLIGHT", "1") };
        let result = preflight();
        unsafe { std::env::remove_var("DBLOCAL_SPIKE_SKIP_PREFLIGHT") };
        assert!(result.is_ok());
    }

    #[test]
    fn report_is_actionable() {
        let msg = format_report("no Vulkan ICD manifest found");
        assert!(msg.contains("Vulkan is not available"));
        assert!(msg.contains("mesa-vulkan-drivers"));
        assert!(msg.contains("vulkaninfo"));
        assert!(msg.contains("Exit code: 78"));
        assert!(msg.contains(DOCS_URL));
        assert!(msg.contains("DBLocal cannot start"));
    }
}
