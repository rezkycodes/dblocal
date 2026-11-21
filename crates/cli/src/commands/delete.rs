use dblocal_core::model::DeleteMode;

use crate::cli::{EXIT_BAD_ARGS, EXIT_GENERAL, EXIT_OK};
use crate::commands::{Ctx, find_instance};
use crate::messages;
use crate::output;

pub fn run(ctx: &mut Ctx<'_>, name: &str, yes: bool, purge: bool) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(v) => v,
        Err(code) => return code,
    };

    if !yes && let Err(code) = confirm(ctx, name) {
        return code;
    }

    let mode = if purge {
        DeleteMode::PurgeData
    } else {
        DeleteMode::ArchiveData
    };
    let mut rx = ctx.service.subscribe();
    let op = match ctx.service.delete_instance(&view.id, mode) {
        Ok(op) => op,
        Err(e) => return output::report_error(ctx.err, &e),
    };

    match output::wait_operation_rx(
        &mut rx,
        &op,
        ctx.service,
        output::Target::Id(&view.id),
        output::Completion::Gone,
        ctx.err,
        ctx.stderr_is_tty,
    ) {
        Ok(()) => {
            report_deleted(ctx, name, purge);
            EXIT_OK
        }
        Err(e) => {
            output::stderr_line(ctx.err, &messages::code_line(&e.code, &e.message));
            EXIT_GENERAL
        }
    }
}

/// Reports the delete outcome: purged, or archived and where.
fn report_deleted(ctx: &mut Ctx<'_>, name: &str, purge: bool) {
    if purge {
        output::stderr_line(ctx.err, messages::MSG_DELETED);
        return;
    }
    // Best effort: the archive path is a convenience, not the outcome.
    let archive = ctx.service.list_archives().ok().and_then(|archives| {
        archives
            .into_iter()
            .find(|a| a.instance_name().as_str() == name)
            .map(|a| a.path().display().to_string())
    });
    match archive {
        Some(path) => {
            output::stderr_line(ctx.err, &format!("{} {path}", messages::MSG_ARCHIVED_TO))
        }
        None => output::stderr_line(ctx.err, messages::MSG_ARCHIVED_TO),
    }
}

/// Requires `--yes` or the exact typed instance name (F-CLI-03 AC3).
fn confirm(ctx: &mut Ctx<'_>, name: &str) -> Result<(), u8> {
    let refuse = |ctx: &mut Ctx<'_>, message: &str| {
        output::stderr_line(
            ctx.err,
            &messages::code_line(dblocal_core::error::CODE_INVALID_INPUT, message),
        );
        Err(EXIT_BAD_ARGS)
    };
    if !ctx.is_interactive {
        return refuse(ctx, messages::ERR_DELETE_NEEDS_YES);
    }
    output::stderr_line(ctx.err, messages::PROMPT_DELETE);
    let Some(input) = (ctx.confirm)() else {
        return refuse(ctx, messages::ERR_CONFIRM_READ);
    };
    if input.trim() != name {
        return refuse(ctx, messages::ERR_NAME_CONFIRMATION);
    }
    Ok(())
}
