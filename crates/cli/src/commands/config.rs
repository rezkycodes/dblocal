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

/// Prints a validation refusal with the module's invalid-input code.
fn refuse(ctx: &mut Ctx<'_>, message: &str) -> u8 {
    output::stderr_line(
        ctx.err,
        &cli_messages::code_line(CODE_INVALID_INPUT, message),
    );
    let _ = CODE_NOT_FOUND;
    EXIT_BAD_ARGS
}

/// Aligned settings table: key, current value, default, unit, restart.
pub fn format_config_table(view: &ConfigView) -> String {
    let mut rows: Vec<[String; 4]> = Vec::new();
    for setting in &view.settings {
        let current = setting
            .current
            .clone()
            .unwrap_or_else(|| messages::DEFAULT_LABEL.to_owned());
        let unit = if setting.unit == "bytes" {
            "bytes"
        } else {
            setting.unit.as_str()
        };
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
