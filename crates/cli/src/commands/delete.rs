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

    if !yes {
        if !ctx.is_interactive {
            output::stderr_line(
                ctx.err,
                &format!("[DBL-1001] {}", messages::ERR_DELETE_NEEDS_YES),
            );
            return EXIT_BAD_ARGS;
        }
        output::stderr_line(ctx.err, messages::PROMPT_DELETE);
        let Some(input) = (ctx.confirm)() else {
            output::stderr_line(ctx.err, "[DBL-1001] gagal membaca konfirmasi");
            return EXIT_BAD_ARGS;
        };
        if input.trim() != name {
            output::stderr_line(
                ctx.err,
                &format!("[DBL-1001] {}", messages::ERR_NAME_CONFIRMATION),
            );
            return EXIT_BAD_ARGS;
        }
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

    match output::wait_operation_rx(&mut rx, &op, ctx.err, ctx.stderr_is_tty) {
        Ok(()) => {
            if purge {
                output::stderr_line(ctx.err, messages::MSG_DELETED);
            } else {
                // find archive path best-effort
                match ctx.service.list_archives() {
                    Ok(archives) => {
                        if let Some(a) =
                            archives.iter().find(|a| a.instance_name().as_str() == name)
                        {
                            output::stderr_line(
                                ctx.err,
                                &format!("{} {}", messages::MSG_ARCHIVED_TO, a.path().display()),
                            );
                        } else {
                            output::stderr_line(ctx.err, messages::MSG_ARCHIVED_TO);
                        }
                    }
                    Err(_) => {
                        output::stderr_line(ctx.err, messages::MSG_ARCHIVED_TO);
                    }
                }
            }
            EXIT_OK
        }
        Err(e) => {
            output::stderr_line(ctx.err, &format!("[{}] {}", e.code, e.message));
            EXIT_GENERAL
        }
    }
}
