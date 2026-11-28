//! `dblocal toolchain ...` handlers.
//!
//! Every handler reads/writes only the toolchain tree under the DBLocal home; the
//! state database is never opened, so these work on a machine with no instances
//! and cannot be broken by a store problem. The async install/remove calls run on
//! a small current-thread runtime.
//!
//! Output conventions match `runtime`: data on stdout, diagnostics and progress on
//! stderr, `--json` for machine output, non-zero exit codes with one-line errors,
//! and `--yes` (or the typed confirmation) before anything destructive.

use std::io::Write;

use dblocal_core::paths::DblPaths;
use dblocal_core::toolchain::catalog::{Profile, ToolId, VersionSpec};
use dblocal_core::toolchain::ops::{
    AvailableVersion, InstalledListing, ToolchainService,
};
use dblocal_core::toolchain::path::Finding;
use dblocal_core::toolchain::php_ini::{PhpSettings, SettingKey};
use dblocal_core::DblError;

use crate::cli::{EXIT_BAD_ARGS, EXIT_NOT_FOUND, EXIT_OK, ToolchainCommand};
use crate::messages;
use crate::output;

/// Dispatches one `toolchain` command. Returns the process exit code.
pub fn run(command: &ToolchainCommand) -> u8 {
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    let is_interactive = output::is_interactive_stdin();
    run_with(command, &mut out, &mut err, is_interactive, &read_confirmation)
}

/// The injectable form: writers and the confirmation source are supplied, so tests
/// drive every branch without a terminal.
pub fn run_with(
    command: &ToolchainCommand,
    out: &mut dyn Write,
    err: &mut dyn Write,
    is_interactive: bool,
    confirm: &dyn Fn() -> Option<String>,
) -> u8 {
    let paths = match DblPaths::from_env() {
        Ok(paths) => paths,
        Err(e) => return output::report_error(err, &e),
    };
    let service = match ToolchainService::from_paths(&paths) {
        Ok(service) => service,
        Err(e) => return output::report_error(err, &e),
    };
    dispatch_on(&service, command, out, err, is_interactive, confirm)
}

/// Dispatches against an injected service. Exposed so tests can supply a service
/// with a fixture catalog and platform instead of one built from the environment.
pub fn dispatch_on(
    service: &ToolchainService,
    command: &ToolchainCommand,
    out: &mut dyn Write,
    err: &mut dyn Write,
    is_interactive: bool,
    confirm: &dyn Fn() -> Option<String>,
) -> u8 {
    match command {
        ToolchainCommand::List {
            tool,
            installed,
            json,
        } => list(service, out, err, tool.as_deref(), *installed, *json),
        ToolchainCommand::Install {
            tool,
            spec,
            profile,
            no_activate,
            yes,
        } => install(
            service,
            out,
            err,
            tool,
            spec,
            profile.as_deref(),
            *no_activate,
            *yes,
            is_interactive,
            confirm,
        ),
        ToolchainCommand::Use { tool, spec } => use_version(service, out, err, tool, spec),
        ToolchainCommand::Remove { tool, spec, yes } => {
            remove(service, out, err, tool, spec, *yes, is_interactive, confirm)
        }
        ToolchainCommand::Which { tool } => which(service, out, err, tool),
        ToolchainCommand::Path {
            print,
            apply,
            remove,
        } => path(
            service,
            out,
            err,
            *print,
            *apply,
            *remove,
            is_interactive,
            confirm,
        ),
        ToolchainCommand::Doctor { json } => doctor(service, out, *json),
        ToolchainCommand::PhpSettings {
            action,
            key,
            value,
            json,
        } => php_settings(service, out, err, action, key.as_deref(), value.as_deref(), *json),
    }
}

fn list(
    service: &ToolchainService,
    out: &mut dyn Write,
    err: &mut dyn Write,
    tool: Option<&str>,
    installed_only: bool,
    json: bool,
) -> u8 {
    let tool_id = match tool {
        Some(text) => match text.parse::<ToolId>() {
            Ok(id) => Some(id),
            Err(_) => {
                output::stderr_line(err, &messages::toolchain_unknown_tool(text));
                return EXIT_BAD_ARGS;
            }
        },
        None => None,
    };

    if installed_only {
        let rows = service.installed_listing();
        if json {
            output::stdout_line(out, &format_installed_json(&rows));
        } else {
            output::stdout_line(out, &format_installed_table(&rows));
        }
        return EXIT_OK;
    }

    let rows = service.list(tool_id, false);
    if json {
        output::stdout_line(out, &format_available_json(&rows));
    } else {
        output::stdout_line(out, &format_available_table(service, &rows));
    }
    EXIT_OK
}

#[allow(clippy::too_many_arguments)]
fn install(
    service: &ToolchainService,
    out: &mut dyn Write,
    err: &mut dyn Write,
    tool: &str,
    spec: &str,
    profile: Option<&str>,
    no_activate: bool,
    yes: bool,
    is_interactive: bool,
    confirm: &dyn Fn() -> Option<String>,
) -> u8 {
    let tool_id = match tool.parse::<ToolId>() {
        Ok(id) => id,
        Err(_) => {
            output::stderr_line(err, &messages::toolchain_unknown_tool(tool));
            return EXIT_BAD_ARGS;
        }
    };
    let version_spec = match VersionSpec::parse(spec) {
        Ok(spec) => spec,
        Err(e) => return output::report_error(err, &e),
    };
    let profile_value = match profile {
        Some(text) => match text.parse::<Profile>() {
            Ok(p) => Some(p),
            Err(e) => return output::report_error(err, &e),
        },
        None => None,
    };

    // Installing needs consent on a non-interactive session, unless --yes.
    if !yes && !confirm_action(is_interactive, confirm, err, || {
        messages::toolchain_install_prompt(tool_id.as_str(), version_spec.as_str())
    }) {
        return EXIT_BAD_ARGS;
    }

    output::stderr_line(
        err,
        &messages::toolchain_installing(tool_id.as_str(), version_spec.as_str()),
    );

    let report = match block_on(service.install(
        tool_id,
        &version_spec,
        profile_value,
        !no_activate,
    )) {
        Ok(Ok(report)) => report,
        Ok(Err(e)) | Err(e) => return output::report_error(err, &e),
    };

    output::stdout_line(
        out,
        &messages::toolchain_installed(
            report.tool.as_str(),
            &report.version,
            report.profile.as_deref(),
            report.already_installed,
            report.activated,
        ),
    );
    EXIT_OK
}

fn use_version(
    service: &ToolchainService,
    out: &mut dyn Write,
    err: &mut dyn Write,
    tool: &str,
    spec: &str,
) -> u8 {
    let tool_id = match tool.parse::<ToolId>() {
        Ok(id) => id,
        Err(_) => {
            output::stderr_line(err, &messages::toolchain_unknown_tool(tool));
            return EXIT_BAD_ARGS;
        }
    };
    let version_spec = match VersionSpec::parse(spec) {
        Ok(spec) => spec,
        Err(e) => return output::report_error(err, &e),
    };
    match service.activate(tool_id, &version_spec) {
        Ok(activation) => {
            output::stdout_line(
                out,
                &messages::toolchain_activated(
                    tool_id.as_str(),
                    activation.active.version(),
                    activation.active.profile(),
                ),
            );
            EXIT_OK
        }
        Err(e) => output::report_error(err, &e),
    }
}

fn remove(
    service: &ToolchainService,
    out: &mut dyn Write,
    err: &mut dyn Write,
    tool: &str,
    spec: &str,
    yes: bool,
    is_interactive: bool,
    confirm: &dyn Fn() -> Option<String>,
) -> u8 {
    let tool_id = match tool.parse::<ToolId>() {
        Ok(id) => id,
        Err(_) => {
            output::stderr_line(err, &messages::toolchain_unknown_tool(tool));
            return EXIT_BAD_ARGS;
        }
    };
    let version_spec = match VersionSpec::parse(spec) {
        Ok(spec) => spec,
        Err(e) => return output::report_error(err, &e),
    };
    if !yes && !confirm_action(is_interactive, confirm, err, || {
        messages::toolchain_remove_prompt(tool_id.as_str(), version_spec.as_str())
    }) {
        return EXIT_BAD_ARGS;
    }
    match block_on(service.remove(tool_id, &version_spec)) {
        Ok(Ok(report)) => {
            output::stdout_line(
                out,
                &messages::toolchain_removed(report.tool.as_str(), &report.version),
            );
            if let Some(cleared) = &report.cleared {
                output::stdout_line(out, &messages::toolchain_cleared_active(cleared.tool().as_str()));
                if report.others_remain {
                    output::stdout_line(
                        out,
                        &messages::toolchain_others_remain(cleared.tool().as_str()),
                    );
                }
            }
            EXIT_OK
        }
        Ok(Err(e)) | Err(e) => output::report_error(err, &e),
    }
}

fn which(service: &ToolchainService, out: &mut dyn Write, err: &mut dyn Write, tool: &str) -> u8 {
    let tool_id = match tool.parse::<ToolId>() {
        Ok(id) => id,
        Err(_) => {
            output::stderr_line(err, &messages::toolchain_unknown_tool(tool));
            return EXIT_BAD_ARGS;
        }
    };
    let Some(active) = service.active(tool_id) else {
        output::stderr_line(err, &messages::toolchain_no_active(tool_id.as_str()));
        return EXIT_NOT_FOUND;
    };
    let dir = service.paths().install_dir(tool_id, active.version(), active.profile());
    // Resolve to the executable the shim runs.
    let target = match tool_id {
        ToolId::Php => dir.join("bin").join(bin_name("php")),
        ToolId::Node => dir.join("bin").join(bin_name("node")),
        ToolId::Composer => dir.join(dblocal_core::toolchain::catalog::php_phar_file()),
    };
    output::stdout_line(out, &target.display().to_string());
    EXIT_OK
}

fn bin_name(stem: &str) -> String {
    dblocal_core::toolchain::catalog::executable_name(stem, dblocal_core::platform::Os::current())
}

fn path(
    service: &ToolchainService,
    out: &mut dyn Write,
    err: &mut dyn Write,
    print: bool,
    apply: bool,
    remove: bool,
    is_interactive: bool,
    confirm: &dyn Fn() -> Option<String>,
) -> u8 {
    let selected = [print, apply, remove].iter().filter(|v| **v).count();
    if selected > 1 {
        output::stderr_line(err, messages::ERR_TOOLCHAIN_PATH_FLAGS);
        return EXIT_BAD_ARGS;
    }
    if print {
        // Always allowed: it writes nothing.
        output::stdout_line(out, &service.path_block_preview());
        return EXIT_OK;
    }
    if apply {
        // Editing a shell file needs explicit consent on a non-interactive run.
        if !confirm_action(is_interactive, confirm, err, || {
            messages::toolchain_path_apply_prompt()
        }) {
            return EXIT_BAD_ARGS;
        }
        // Show the exact lines first, as the design requires.
        output::stderr_line(err, &service.path_block_preview());
        return match service.apply_path() {
            Ok(report) => {
                report_path_edit(out, err, &report);
                EXIT_OK
            }
            Err(e) => output::report_error(err, &e),
        };
    }
    if remove {
        return match service.remove_path() {
            Ok(report) => {
                report_path_edit(out, err, &report);
                EXIT_OK
            }
            Err(e) => output::report_error(err, &e),
        };
    }
    // Default: report the state without changing it.
    output::stdout_line(
        out,
        &messages::toolchain_path_state(service.path_applied(), &service.paths().shims_dir().display().to_string()),
    );
    EXIT_OK
}

fn report_path_edit(
    out: &mut dyn Write,
    err: &mut dyn Write,
    report: &dblocal_core::toolchain::path::PathEditReport,
) {
    for path in &report.changed {
        output::stdout_line(out, &messages::toolchain_path_changed(&path.display().to_string()));
    }
    for (path, reason) in &report.failures {
        output::stderr_line(
            err,
            &messages::code_line(
                dblocal_core::error::CODE_IO,
                &messages::toolchain_path_failed(&path.display().to_string(), reason),
            ),
        );
    }
    if report.changed.is_empty() && report.failures.is_empty() {
        output::stderr_line(out, messages::MSG_TOOLCHAIN_PATH_UNCHANGED);
    }
    if !report.backups.is_empty() {
        output::stderr_line(
            out,
            &messages::toolchain_backups_written(report.backups.len()),
        );
    }
}

fn doctor(service: &ToolchainService, out: &mut dyn Write, json: bool) -> u8 {
    let path_var = std::env::var_os("PATH")
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_default();
    let findings = service.doctor(&path_var);
    if json {
        output::stdout_line(out, &format_doctor_json(&findings));
    } else {
        output::stdout_line(out, &format_doctor_table(&findings));
    }
    EXIT_OK
}

fn php_settings(
    service: &ToolchainService,
    out: &mut dyn Write,
    err: &mut dyn Write,
    action: &str,
    key: Option<&str>,
    value: Option<&str>,
    json: bool,
) -> u8 {
    match action {
        "get" => {
            let settings = service.php_settings();
            if json {
                output::stdout_line(out, &format_settings_json(&settings));
            } else {
                output::stdout_line(out, &settings.render_ini());
            }
            EXIT_OK
        }
        "set" => {
            let (Some(key), Some(value)) = (key, value) else {
                output::stderr_line(err, messages::ERR_TOOLCHAIN_SET_USAGE);
                return EXIT_BAD_ARGS;
            };
            let key = match key.parse::<SettingKey>() {
                Ok(key) => key,
                Err(e) => return output::report_error(err, &e),
            };
            let parsed = match PhpSettings::parse_value(key, value) {
                Ok(parsed) => parsed,
                Err(e) => return output::report_error(err, &e),
            };
            if service.active(ToolId::Php).is_none() {
                output::stderr_line(err, &messages::toolchain_no_active(ToolId::Php.as_str()));
                return EXIT_NOT_FOUND;
            }
            match service.set_php_settings(&[(key, parsed)]) {
                Ok(path) => {
                    output::stdout_line(
                        out,
                        &messages::toolchain_settings_written(&path.display().to_string()),
                    );
                    EXIT_OK
                }
                Err(e) => output::report_error(err, &e),
            }
        }
        "reset" => {
            if service.active(ToolId::Php).is_none() {
                output::stderr_line(err, &messages::toolchain_no_active(ToolId::Php.as_str()));
                return EXIT_NOT_FOUND;
            }
            match service.reset_php_settings() {
                Ok(path) => {
                    output::stdout_line(
                        out,
                        &messages::toolchain_settings_written(&path.display().to_string()),
                    );
                    EXIT_OK
                }
                Err(e) => output::report_error(err, &e),
            }
        }
        "open" => {
            // Print the managed dir and the ini file; do not launch a GUI editor
            // from the CLI (the GUI part owns "open in editor").
            let dir = service.php_settings_dir();
            let ini = service.paths().php_managed_ini();
            output::stdout_line(out, &dir.display().to_string());
            output::stdout_line(out, &ini.display().to_string());
            EXIT_OK
        }
        other => {
            output::stderr_line(err, &messages::toolchain_unknown_action(other));
            EXIT_BAD_ARGS
        }
    }
}

/// Asks for confirmation interactively or reports the `--yes` requirement.
fn confirm_action(
    is_interactive: bool,
    confirm: &dyn Fn() -> Option<String>,
    err: &mut dyn Write,
    prompt: impl FnOnce() -> String,
) -> bool {
    if !is_interactive {
        output::stderr_line(err, &messages::code_line(dblocal_core::error::CODE_INVALID_INPUT, messages::ERR_TOOLCHAIN_NEEDS_YES));
        return false;
    }
    output::stderr_line(err, &prompt());
    match confirm() {
        Some(answer) if answer.trim().eq_ignore_ascii_case("y") || answer.trim().eq_ignore_ascii_case("yes") => true,
        _ => {
            output::stderr_line(err, messages::ERR_TOOLCHAIN_CANCELLED);
            false
        }
    }
}

fn read_confirmation() -> Option<String> {
    use std::io::BufRead as _;
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(_) if !line.is_empty() => Some(line),
        _ => None,
    }
}

/// Runs an async toolchain operation on a small current-thread runtime.
fn block_on<F, T>(future: F) -> dblocal_core::Result<T>
where
    F: std::future::Future<Output = T>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|source| DblError::Io { source })?;
    Ok(runtime.block_on(future))
}

// --- formatting -------------------------------------------------------------

fn format_available_json(rows: &[AvailableVersion]) -> String {
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "tool": row.tool.as_str(),
                "version": row.version,
                "series": row.series,
                "lts": row.lts,
                "installed": row.installed,
                "active": row.active,
                "available_here": row.available_here,
            })
        })
        .collect();
    serde_json::Value::Array(items).to_string()
}

fn format_available_table(service: &ToolchainService, rows: &[AvailableVersion]) -> String {
    let headers = ["TOOL", "VERSION", "SERIES", "STATUS"];
    let rendered: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let status = if row.active {
                "active"
            } else if row.installed {
                "installed"
            } else if row.available_here {
                "available"
            } else {
                "unavailable here"
            };
            vec![
                row.tool.as_str().to_owned(),
                row.version.clone(),
                row.series.clone(),
                status.to_owned(),
            ]
        })
        .collect();
    let mut table = output::render_table(headers.iter().map(|h| (*h).to_owned()).collect(), rendered);
    let _ = service;
    while table.ends_with('\n') {
        table.pop();
    }
    table
}

fn format_installed_json(rows: &[InstalledListing]) -> String {
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "tool": row.tool.as_str(),
                "version": row.version,
                "profile": row.profile,
                "size_bytes": row.size_bytes,
                "active": row.active,
            })
        })
        .collect();
    serde_json::Value::Array(items).to_string()
}

fn format_installed_table(rows: &[InstalledListing]) -> String {
    let headers = ["TOOL", "VERSION", "PROFILE", "SIZE", "ACTIVE"];
    let rendered: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.tool.as_str().to_owned(),
                row.version.clone(),
                row.profile.clone().unwrap_or_else(|| "-".to_owned()),
                output::human_size_u64(row.size_bytes),
                if row.active { "yes" } else { "" }.to_owned(),
            ]
        })
        .collect();
    let mut table = output::render_table(headers.iter().map(|h| (*h).to_owned()).collect(), rendered);
    while table.ends_with('\n') {
        table.pop();
    }
    table
}

fn format_doctor_json(findings: &[Finding]) -> String {
    let items: Vec<serde_json::Value> = findings
        .iter()
        .map(|finding| {
            serde_json::json!({
                "code": finding.code(),
                "message": finding.message(),
            })
        })
        .collect();
    serde_json::Value::Array(items).to_string()
}

fn format_doctor_table(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return messages::MSG_TOOLCHAIN_DOCTOR_OK.to_owned();
    }
    let mut out = String::new();
    for finding in findings {
        out.push_str(&format!("- [{}] {}\n", finding.code(), finding.message()));
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

fn format_settings_json(settings: &PhpSettings) -> String {
    let mut map = serde_json::Map::new();
    for key in SettingKey::all() {
        map.insert(
            key.as_str().to_owned(),
            serde_json::Value::String(settings.get(key).render()),
        );
    }
    serde_json::Value::Object(map).to_string()
}

/// Maps a toolchain error onto a CLI exit code (tests import this path).
pub fn code_for(err: &DblError) -> u8 {
    output::code_for(err)
}

#[cfg(test)]
mod tests;
