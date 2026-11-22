//! `dblocal db list|create|drop` (`F-DBU-01`).
//!
//! stdout is data: `list` prints the aligned table (or one JSON array per
//! `--json`); `create`/`drop` print nothing on success (progress goes to stderr).
//! `drop` requires the database name to be typed again, from `--confirm` or, in
//! an interactive session, a prompt (`F-DBU-01` AC3).

use crate::cli::{DbCommand, EXIT_BAD_ARGS, EXIT_OK};
use crate::commands::{Ctx, find_instance};
use crate::messages;
use crate::output;

pub fn dispatch(ctx: &mut Ctx<'_>, command: &DbCommand) -> u8 {
    match command {
        DbCommand::List { instance, json } => list(ctx, instance, *json),
        DbCommand::Create { instance, name } => create(ctx, instance, name),
        DbCommand::Drop {
            instance,
            name,
            yes,
            confirm,
        } => drop(ctx, instance, name, *yes, confirm.as_deref()),
    }
}

fn list(ctx: &mut Ctx<'_>, instance: &str, json: bool) -> u8 {
    let view = match find_instance(ctx, instance) {
        Ok(view) => view,
        Err(code) => return code,
    };
    let databases = match ctx.service.list_databases(&view.id) {
        Ok(databases) => databases,
        Err(err) => return output::report_facing(ctx.err, &err),
    };
    if json {
        output::stdout_line(ctx.out, &output::format_databases_json(&databases));
    } else {
        output::stdout_line(ctx.out, &output::format_database_table(&databases));
    }
    EXIT_OK
}

fn create(ctx: &mut Ctx<'_>, instance: &str, name: &str) -> u8 {
    let view = match find_instance(ctx, instance) {
        Ok(view) => view,
        Err(code) => return code,
    };
    match ctx.service.create_database(&view.id, name) {
        Ok(()) => {
            output::stderr_line(ctx.err, messages::MSG_DB_CREATED);
            EXIT_OK
        }
        Err(err) => output::report_facing(ctx.err, &err),
    }
}

/// Drops a database after a typed-name confirmation (`F-DBU-01` AC3).
fn drop(ctx: &mut Ctx<'_>, instance: &str, name: &str, yes: bool, confirm: Option<&str>) -> u8 {
    let view = match find_instance(ctx, instance) {
        Ok(view) => view,
        Err(code) => return code,
    };
    if !yes {
        if let Err(code) = confirm_name(ctx, name, confirm) {
            return code;
        }
    }
    match ctx.service.drop_database(&view.id, name, false) {
        Ok(()) => {
            output::stderr_line(ctx.err, messages::MSG_DB_DROPPED);
            EXIT_OK
        }
        Err(err) => output::report_facing(ctx.err, &err),
    }
}

/// Accepts only an exact typed name: `--confirm`, or the interactive prompt.
fn confirm_name(ctx: &mut Ctx<'_>, name: &str, confirm: Option<&str>) -> Result<(), u8> {
    if let Some(typed) = confirm {
        if typed.trim() == name {
            return Ok(());
        }
        return Err(refuse(ctx, messages::ERR_DB_CONFIRM_MISMATCH));
    }
    if !ctx.is_interactive {
        return Err(refuse(ctx, messages::ERR_DB_NEEDS_CONFIRM));
    }
    output::stderr_line(ctx.err, messages::PROMPT_DB_CONFIRM);
    let Some(input) = (ctx.confirm)() else {
        return Err(refuse(ctx, messages::ERR_CONFIRM_READ));
    };
    if input.trim() == name {
        Ok(())
    } else {
        Err(refuse(ctx, messages::ERR_DB_CONFIRM_MISMATCH))
    }
}

fn refuse(ctx: &mut Ctx<'_>, message: &str) -> u8 {
    output::stderr_line(
        ctx.err,
        &messages::code_line(dblocal_core::error::CODE_INVALID_INPUT, message),
    );
    EXIT_BAD_ARGS
}
