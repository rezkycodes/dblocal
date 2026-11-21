use std::time::Duration;

use dblocal_core::DblError;
use dblocal_core::model::{InstanceId, InstanceStatus};

use crate::cli::{EXIT_GENERAL, EXIT_OK};
use crate::commands::{Ctx, find_instance};
use crate::messages;
use crate::output;

pub fn run(ctx: &mut Ctx<'_>, name: &str) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(v) => v,
        Err(code) => return code,
    };
    match view.status {
        InstanceStatus::Stopped => {
            output::stderr_line(ctx.err, messages::MSG_ALREADY_STOPPED);
            return EXIT_OK;
        }
        // Another process is already stopping it. That is exactly what the
        // user asked for, so this is the idempotent success path, not an error.
        InstanceStatus::Stopping => return already_being_stopped(ctx, &view.id),
        _ => {}
    }
    output::stderr_line(ctx.err, &format!("{} {name}...", messages::MSG_STOPPING));
    let mut rx = ctx.service.subscribe();
    let op = match ctx.service.stop_instance(&view.id) {
        Ok(op) => op,
        // The compare-and-set refused because another writer moved the row
        // between our read and the write.
        Err(DblError::InvalidTransition {
            from: "stopped", ..
        }) => {
            output::stderr_line(ctx.err, messages::MSG_ALREADY_STOPPED);
            return EXIT_OK;
        }
        Err(DblError::InvalidTransition {
            from: "stopping", ..
        }) => return already_being_stopped(ctx, &view.id),
        Err(e) => return output::report_error(ctx.err, &e),
    };
    let wait = output::StatusWait {
        op: &op,
        instance_id: &view.id,
        ok_statuses: &[InstanceStatus::Stopped],
        timeout: Duration::from_secs(crate::consts::STOP_TIMEOUT_SECS),
        show_progress: ctx.stderr_is_tty,
    };
    match output::wait_for_status_rx(&mut rx, ctx.service, wait, ctx.err) {
        Ok(_) => {
            output::stderr_line(ctx.err, messages::MSG_READY);
            EXIT_OK
        }
        Err(e) => {
            output::stderr_line(ctx.err, &messages::code_line(&e.code, &e.message));
            EXIT_GENERAL
        }
    }
}

/// Waits for another process's stop to land, then reports it as success.
///
/// The event stream is per process, so this re-reads the shared state on the
/// cross-process interval. Both outcomes the caller can act on are success: the
/// row reaches `Stopped`, or it disappears (a delete raced the stop). Only a
/// failed stop or an exhausted wait is an error.
fn already_being_stopped(ctx: &mut Ctx<'_>, id: &InstanceId) -> u8 {
    output::stderr_line(ctx.err, &messages::stopping_elsewhere_line());
    let deadline =
        std::time::Instant::now() + Duration::from_secs(crate::consts::STOP_TIMEOUT_SECS);
    loop {
        match ctx.service.list_instances() {
            Ok(views) => match views.into_iter().find(|v| &v.id == id) {
                Some(view) => match view.status {
                    InstanceStatus::Stopped => {
                        output::stderr_line(ctx.err, messages::MSG_READY);
                        return EXIT_OK;
                    }
                    InstanceStatus::Failed => {
                        if let Some(failure) = view.failure {
                            output::stderr_line(
                                ctx.err,
                                &messages::code_line(&failure.code, &failure.message),
                            );
                        }
                        return EXIT_GENERAL;
                    }
                    _ => {}
                },
                None => {
                    output::stderr_line(ctx.err, messages::MSG_READY);
                    return EXIT_OK;
                }
            },
            Err(e) => return output::report_error(ctx.err, &e),
        }
        if std::time::Instant::now() >= deadline {
            output::stderr_line(ctx.err, &messages::timeout_line());
            return EXIT_GENERAL;
        }
        std::thread::sleep(Duration::from_millis(crate::consts::CROSS_PROCESS_POLL_MS));
    }
}
