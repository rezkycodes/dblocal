//! `dblocal config show|set|restore|path` (`F-CFG-01`, `F-CFG-02`).
//!
//! stdout is data (the table or JSON, the raw path); progress and the backup
//! notice go to stderr, like the other commands. `set` prints "perlu restart"
//! and the backup path. Exit codes and error printing match the other commands.

use dblocal_core::config::{ConfigView, messages};
use dblocal_core::error::{CODE_INVALID_INPUT, CODE_NOT_FOUND};

use crate::cli::{ConfigCommand, EXIT_BAD_ARGS, EXIT_OK};
use crate::commands::{Ctx, find_instance};
use crate::messages as cli_messages;
use crate::output;

pub fn dispatch(ctx: &mut Ctx<'_>, command: &ConfigCommand) -> u8 {
    match command {
        ConfigCommand::Show { name, json } => show(ctx, name, *json),
        ConfigCommand::Set { name, changes } => set(ctx, name, changes),
        ConfigCommand::Restore { name } => restore(ctx, name),
        ConfigCommand::Path { name } => path(ctx, name),
        ConfigCommand::Open { name } => open(ctx, name),
    }
}

fn show(ctx: &mut Ctx<'_>, name: &str, json: bool) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(view) => view,
        Err(code) => return code,
    };
    let config = match ctx.service.config_view(&view.id) {
        Ok(config) => config,
        Err(err) => return output::report_facing(ctx.err, &err),
    };
    if json {
        output::stdout_line(ctx.out, &format_config_json(&config));
    } else {
        output::stdout_line(ctx.out, &format_config_table(&config));
    }
    EXIT_OK
}

fn set(ctx: &mut Ctx<'_>, name: &str, changes: &[String]) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(view) => view,
        Err(code) => return code,
    };
    if changes.is_empty() {
        return refuse(ctx, messages::ERR_SET_FORMAT);
    }
    let mut parsed: Vec<(String, String)> = Vec::with_capacity(changes.len());
    for change in changes {
        let Some((key, value)) = change.split_once('=') else {
            return refuse(ctx, messages::ERR_SET_FORMAT);
        };
        parsed.push((key.trim().to_owned(), value.to_owned()));
    }
    match ctx.service.config_apply(&view.id, parsed) {
        Ok(applied) => {
            // "perlu restart" and the backup path go to stderr.
            output::stderr_line(ctx.err, messages::MSG_SAVED);
            output::stderr_line(
                ctx.err,
                &format!(
                    "{} {}",
                    messages::MSG_BACKUP_AT,
                    applied.backup.path.display()
                ),
            );
            let _ = applied.restart_required;
            EXIT_OK
        }
        Err(err) => output::report_facing(ctx.err, &err),
    }
}

fn restore(ctx: &mut Ctx<'_>, name: &str) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(view) => view,
        Err(code) => return code,
    };
    match ctx.service.config_restore(&view.id) {
        Ok(()) => {
            output::stderr_line(ctx.err, messages::MSG_RESTORED);
            EXIT_OK
        }
        Err(err) => output::report_facing(ctx.err, &err),
    }
}

fn path(ctx: &mut Ctx<'_>, name: &str) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(view) => view,
        Err(code) => return code,
    };
    match ctx.service.config_raw_path(&view.id) {
        Ok(raw) => {
            output::stdout_line(ctx.out, &raw.display().to_string());
            EXIT_OK
        }
        Err(err) => output::report_facing(ctx.err, &err),
    }
}

/// Opens the raw config file in the OS default handler (`F-CFG-02` AC1).
///
/// On success it prints the path it opened. When the handler could not be
/// started, it prints the path to stdout anyway (so the user can open it by
/// hand) and the typed error to stderr, exiting non-zero.
fn open(ctx: &mut Ctx<'_>, name: &str) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(view) => view,
        Err(code) => return code,
    };
    match ctx.service.config_open_raw(&view.id) {
        Ok(command) => {
            // The handler ran; print the file path (last argv is the path on
            // every OS) so the user and a script both see what was opened.
            if let Some(path) = command
                .args
                .iter()
                .find(|arg| !arg.contains("FileProtocolHandler"))
            {
                output::stdout_line(ctx.out, path);
            }
            EXIT_OK
        }
        Err(err) => {
            // Best effort: still show the path so the user can open it manually.
            if let Ok(raw) = ctx.service.config_raw_path(&view.id) {
                output::stdout_line(ctx.out, &raw.display().to_string());
            }
            output::report_facing(ctx.err, &err)
        }
    }
}

/// Prints a validation refusal with the module's invalid-input code.
fn refuse(ctx: &mut Ctx<'_>, message: &str) -> u8 {
    output::stderr_line(
        ctx.err,
        &cli_messages::code_line(CODE_INVALID_INPUT, message),
    );
    let _ = CODE_NOT_FOUND;
    EXIT_BAD_ARGS
}

/// Maps a machine-readable unit to what the user reads and types.
///
/// A size setting's value column shows `128MB`, so its unit says the size words,
/// not the storage primitive `bytes`; `seconds` becomes the Indonesian `detik`.
/// `--json` keeps the raw unit.
fn human_unit(unit: &str) -> String {
    match unit {
        "bytes" => messages::UNIT_SIZE.to_owned(),
        "seconds" => messages::UNIT_SECONDS.to_owned(),
        other => other.to_owned(),
    }
}

/// Aligned settings table: key, current value, default, unit, restart.
pub fn format_config_table(view: &ConfigView) -> String {
    let mut rows: Vec<[String; 4]> = Vec::new();
    for setting in &view.settings {
        let current = setting
            .current
            .clone()
            .unwrap_or_else(|| messages::DEFAULT_LABEL.to_owned());
        let unit = human_unit(&setting.unit);
        let restart = if setting.readonly {
            messages::READONLY_LABEL.to_owned()
        } else {
            messages::YES.to_owned()
        };
        rows.push([
            setting.key.clone(),
            current,
            setting.default.clone(),
            format!("{unit} {restart}").trim().to_owned(),
        ]);
    }
    let header = [
        messages::HEADER_SETTING.to_owned(),
        messages::HEADER_CURRENT.to_owned(),
        messages::HEADER_DEFAULT.to_owned(),
        format!("{} {}", messages::HEADER_UNIT, messages::HEADER_RESTART),
    ];
    let mut widths = [0usize; 4];
    for (index, cell) in header.iter().enumerate() {
        widths[index] = cell.chars().count();
    }
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    let render = |cells: &[String; 4]| {
        let mut line = String::new();
        for (index, cell) in cells.iter().enumerate() {
            if index + 1 == cells.len() {
                line.push_str(cell);
            } else {
                line.push_str(&format!("{cell:<width$}  ", width = widths[index]));
            }
        }
        line
    };
    let mut out = String::new();
    out.push_str(&render(&header));
    out.push('\n');
    for row in rows {
        out.push_str(&render(&row));
        out.push('\n');
    }
    if let Some(backup) = &view.backup_available {
        out.push_str(&format!(
            "\n{} {} ({})\n",
            messages::MSG_BACKUP_AT,
            backup.path.display(),
            backup.taken_at
        ));
    }
    out.trim_end().to_owned()
}

/// JSON array of the settings, plus backup and path metadata.
pub fn format_config_json(view: &ConfigView) -> String {
    let settings: Vec<serde_json::Value> = view
        .settings
        .iter()
        .map(|s| {
            serde_json::json!({
                "key": s.key,
                "current": s.current,
                "default": s.default,
                "unit": s.unit,
                "restart_required": s.restart_required,
                "readonly": s.readonly,
                "description": s.description,
            })
        })
        .collect();
    let backup = view.backup_available.as_ref().map(
        |b| serde_json::json!({ "taken_at": b.taken_at, "path": b.path.display().to_string() }),
    );
    serde_json::json!({
        "engine": view.engine.as_str(),
        "settings": settings,
        "backup_available": backup,
        "raw_path": view.raw_path.display().to_string(),
        "restart_required": view.restart_required,
    })
    .to_string()
}
