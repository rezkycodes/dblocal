//! `dblocal runtime list|remove` (`F-RTM-04`).
//!
//! `list` prints every installed runtime with the instances that use it, so an
//! unused runtime is obvious before it is removed. `remove` deletes one
//! unreferenced runtime and the space it takes; it refuses a runtime that an
//! instance still uses (the core re-checks under a lock) and refuses to run
//! without `--yes` in a non-interactive session, like `delete`.

use dblocal_core::model::EngineKey;

use crate::cli::{EXIT_BAD_ARGS, EXIT_NOT_FOUND, EXIT_OK};
use crate::commands::Ctx;
use crate::messages;
use crate::output;

/// `dblocal runtime list [--json]`.
pub fn list(ctx: &mut Ctx<'_>, json: bool) -> u8 {
    let runtimes = match ctx.service.list_runtimes() {
        Ok(runtimes) => runtimes,
        Err(err) => return output::report_error(ctx.err, &err),
    };
    let instances = match ctx.service.list_instances() {
        Ok(instances) => instances,
        Err(err) => return output::report_error(ctx.err, &err),
    };
    if json {
        output::stdout_line(
            ctx.out,
            &output::format_runtime_list_json(&runtimes, &instances),
        );
    } else {
        output::stdout_line(
            ctx.out,
            &output::format_runtime_list_table(&runtimes, &instances),
        );
    }
    EXIT_OK
}

/// `dblocal runtime remove <engine> <version> [--yes]`.
///
/// The runtime is addressed by engine and version, not by the store's internal
/// id: those are the two values the user sees in `runtime list`, in the dock and
/// in the catalog.
pub fn remove(ctx: &mut Ctx<'_>, engine: &str, version: &str, yes: bool) -> u8 {
    let engine_key = match engine.parse::<EngineKey>() {
        Ok(key) => key,
        Err(_) => {
            output::stderr_line(
                ctx.err,
                &messages::code_line(
                    dblocal_core::error::CODE_INVALID_INPUT,
                    &format!("{}: {engine}", messages::ERR_UNKNOWN_ENGINE),
                ),
            );
            return EXIT_BAD_ARGS;
        }
    };
    let runtimes = match ctx.service.list_runtimes() {
        Ok(runtimes) => runtimes,
        Err(err) => return output::report_error(ctx.err, &err),
    };
    let Some(runtime) = runtimes
        .into_iter()
        .find(|rt| rt.engine() == engine_key && rt.version() == version)
    else {
        output::stderr_line(
            ctx.err,
            &messages::code_line(
                dblocal_core::error::CODE_NOT_FOUND,
                &format!("{}: {engine} {version}", messages::ERR_RUNTIME_NOT_FOUND),
            ),
        );
        return EXIT_NOT_FOUND;
    };

    if !yes && let Err(code) = confirm(ctx, runtime.engine().display_name(), version) {
        return code;
    }

    match ctx.service.remove_runtime(runtime.id()) {
        Ok(()) => {
            output::stderr_line(ctx.err, messages::MSG_RUNTIME_REMOVED);
            EXIT_OK
        }
        Err(err) => {
            let code = output::report_error(ctx.err, &err);
            if let dblocal_core::DblError::RuntimeInUse { users } = &err {
                output::stderr_line(ctx.err, &messages::runtime_used_by(users));
            }
            code
        }
    }
}

/// Requires `--yes` or the exact typed version (mirrors `delete`'s confirmation).
fn confirm(ctx: &mut Ctx<'_>, engine: &str, version: &str) -> Result<(), u8> {
    let refuse = |ctx: &mut Ctx<'_>, message: &str| {
        output::stderr_line(
            ctx.err,
            &messages::code_line(dblocal_core::error::CODE_INVALID_INPUT, message),
        );
        Err(EXIT_BAD_ARGS)
    };
    if !ctx.is_interactive {
        return refuse(ctx, messages::ERR_RUNTIME_REMOVE_NEEDS_YES);
    }
    output::stderr_line(ctx.err, &messages::runtime_remove_prompt(engine, version));
    let Some(input) = (ctx.confirm)() else {
        return refuse(ctx, messages::ERR_CONFIRM_READ);
    };
    if input.trim() != version {
        return refuse(ctx, messages::ERR_RUNTIME_CONFIRMATION);
    }
    Ok(())
}
