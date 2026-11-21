use std::time::{Duration, Instant};

use dblocal_core::DblError;
use dblocal_core::model::{InstanceId, InstanceStatus};

use crate::cli::{EXIT_ENGINE_FAILED, EXIT_GENERAL, EXIT_NOT_FOUND, EXIT_OK};
use crate::commands::{Ctx, find_instance};
use crate::messages;
use crate::output;

/// How often a cross-process wait re-reads the shared state.
const CROSS_PROCESS_POLL: Duration = Duration::from_millis(100);

pub fn run(ctx: &mut Ctx<'_>, name: &str, timeout: u64) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let timeout = Duration::from_secs(timeout.max(1));

    if view.status == InstanceStatus::Running {
        output::stderr_line(ctx.err, messages::ERR_ALREADY_RUNNING);
        return EXIT_OK;
    }
    if view.status == InstanceStatus::Starting {
        // Another process is already starting it: the loser of the race.
        output::stderr_line(ctx.err, messages::ERR_ALREADY_RUNNING);
        return wait_cross_process(ctx, &view.id, timeout);
    }

    output::stderr_line(ctx.err, &format!("{} {name}...", messages::MSG_STARTING));
    let mut rx = ctx.service.subscribe();
    let op = match ctx.service.start_instance(&view.id) {
        Ok(op) => op,
        Err(DblError::InvalidTransition { from: "busy", .. }) => {
            // The compare-and-set refused because another writer (another
            // process, or a sibling service) claimed the row between our read
            // and the write. F-CLI-02 AC3: the loser must not start a second
            // engine; it waits for the winner and exits 0.
            output::stderr_line(ctx.err, messages::ERR_ALREADY_RUNNING);
            return wait_cross_process(ctx, &view.id, timeout);
        }
        Err(e) => return output::report_error(ctx.err, &e),
    };

    let wait = output::StatusWait {
        op: &op,
        instance_id: &view.id,
        ok_statuses: &[InstanceStatus::Running],
        timeout,
        show_progress: ctx.stderr_is_tty,
    };
    match output::wait_for_status_rx(&mut rx, ctx.service, wait, ctx.err) {
        Ok(_) => {
            output::stderr_line(ctx.err, messages::MSG_READY);
            EXIT_OK
        }
        Err(e) => report_start_failure(ctx, &e),
    }
}

/// Waits for an instance another process is starting.
///
/// The service's `broadcast` channel is per-process, so a cross-process state
/// change can only be observed by re-reading the store. This is a bounded
/// wait with a fixed interval, not a busy loop, and it exists only for the
/// race case the PRD explicitly requires (F-CLI-02 AC3).
fn wait_cross_process(ctx: &mut Ctx<'_>, id: &InstanceId, timeout: Duration) -> u8 {
    let deadline = Instant::now() + timeout;
    loop {
        match ctx.service.list_instances() {
            Ok(views) => {
                if let Some(view) = views.into_iter().find(|v| &v.id == id) {
                    match view.status {
                        InstanceStatus::Running => {
                            output::stderr_line(ctx.err, messages::MSG_READY);
                            return EXIT_OK;
                        }
                        InstanceStatus::Failed => {
                            if let Some(failure) = view.failure {
                                output::stderr_line(
                                    ctx.err,
                                    &format!("[{}] {}", failure.code, failure.message),
                                );
                                output::stderr_line(
                                    ctx.err,
                                    &format!("diagnosis: {}", failure.diagnosis.as_str()),
                                );
                            }
                            return EXIT_ENGINE_FAILED;
                        }
                        _ => {}
                    }
                } else {
                    return EXIT_NOT_FOUND;
                }
            }
            Err(e) => return output::report_error(ctx.err, &e),
        }
        if Instant::now() >= deadline {
            output::stderr_line(ctx.err, "[DBL-4001] waktu tunggu habis");
            return EXIT_ENGINE_FAILED;
        }
        std::thread::sleep(CROSS_PROCESS_POLL);
    }
}

/// Every failed start exits 4 (F-CLI-02: "a failed start prints the DBL code,
/// the Indonesian message and the diagnosis, exit 4").
fn report_start_failure(ctx: &mut Ctx<'_>, e: &dblocal_core::service::UserFacingError) -> u8 {
    output::stderr_line(ctx.err, &format!("[{}] {}", e.code, e.message));
    if let Some(diag) = e.diagnosis {
        output::stderr_line(ctx.err, &format!("diagnosis: {}", diag.as_str()));
    }
    if e.code == "DBL-1002" {
        EXIT_NOT_FOUND
    } else if e.code == "DBL-1001" || e.code == "DBL-1004" {
        EXIT_GENERAL
    } else {
        EXIT_ENGINE_FAILED
    }
}
