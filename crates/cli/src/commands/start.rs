use std::time::{Duration, Instant};

use dblocal_core::DblError;
use dblocal_core::model::{InstanceId, InstanceStatus};

use crate::cli::{EXIT_ENGINE_FAILED, EXIT_GENERAL, EXIT_NOT_FOUND, EXIT_OK};
use crate::commands::{Ctx, find_instance};
use crate::messages;
use crate::output;

/// How often a cross-process wait re-reads the shared state.
const CROSS_PROCESS_POLL: Duration = Duration::from_millis(crate::consts::CROSS_PROCESS_POLL_MS);

pub fn run(ctx: &mut Ctx<'_>, name: &str, timeout: u64) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let timeout = Duration::from_secs(timeout.max(1));
    if let Some(code) = settle_without_starting(ctx, &view, timeout) {
        return code;
    }

    output::stderr_line(ctx.err, &format!("{} {name}...", messages::MSG_STARTING));
    let mut rx = ctx.service.subscribe();
    let op = match ctx.service.start_instance(&view.id) {
        Ok(op) => op,
        Err(e) => match refused_owner(&e) {
            Refusal::Loser => {
                output::stderr_line(ctx.err, messages::ERR_ALREADY_RUNNING);
                return wait_cross_process(ctx, &view.id, timeout);
            }
            // `stopping` is NOT the same race: nobody is going to reach
            // `running`. Waiting for one would burn the whole timeout and then
            // claim an engine failure, so it is reported immediately instead.
            Refusal::Stopping => return report_stopping_elsewhere(ctx),
            Refusal::Other => return output::report_error(ctx.err, &e),
        },
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

/// Handles the statuses that need no start: already running, another process
/// starting it, or another process stopping it. `None` means "start it".
fn settle_without_starting(
    ctx: &mut Ctx<'_>,
    view: &dblocal_core::service::InstanceView,
    timeout: Duration,
) -> Option<u8> {
    match view.status {
        InstanceStatus::Running => {
            output::stderr_line(ctx.err, messages::ERR_ALREADY_RUNNING);
            Some(EXIT_OK)
        }
        // Another process is already starting it: the loser of the race.
        InstanceStatus::Starting => {
            output::stderr_line(ctx.err, messages::ERR_ALREADY_RUNNING);
            Some(wait_cross_process(ctx, &view.id, timeout))
        }
        InstanceStatus::Stopping => Some(report_stopping_elsewhere(ctx)),
        _ => None,
    }
}

/// Who owns the transition a refused `start` ran into.
#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    /// Another process is starting or running it: the race loser waits.
    Loser,
    /// Another process is stopping it, so `running` will not come.
    Stopping,
    /// Anything else.
    Other,
}

/// Classifies a refused `start`.
///
/// The compare-and-set reports `busy` when another writer claimed the row
/// between our read and our write. The concrete `starting`/`running` means the
/// winner persisted its claim first (F-CLI-02 AC3: the loser waits, exits 0).
fn refused_owner(err: &DblError) -> Refusal {
    match err {
        DblError::InvalidTransition {
            from: "busy" | "starting" | "running",
            ..
        } => Refusal::Loser,
        DblError::InvalidTransition {
            from: "stopping", ..
        } => Refusal::Stopping,
        _ => Refusal::Other,
    }
}

/// Prints the "another process is stopping it" line and returns its exit code.
fn report_stopping_elsewhere(ctx: &mut Ctx<'_>) -> u8 {
    output::stderr_line(ctx.err, &messages::stopping_elsewhere_line());
    EXIT_GENERAL
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
                        // The winner was stopped before it became healthy (or a
                        // sibling stop won): `running` is not coming either.
                        InstanceStatus::Stopping => return report_stopping_elsewhere(ctx),
                        InstanceStatus::Failed => {
                            if let Some(failure) = view.failure {
                                output::stderr_line(
                                    ctx.err,
                                    &messages::code_line(&failure.code, &failure.message),
                                );
                                output::stderr_line(
                                    ctx.err,
                                    &messages::diagnosis_line(failure.diagnosis.as_str()),
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
            output::stderr_line(ctx.err, &messages::timeout_line());
            return EXIT_ENGINE_FAILED;
        }
        std::thread::sleep(CROSS_PROCESS_POLL);
    }
}

/// Every failed start exits 4 (F-CLI-02: "a failed start prints the DBL code,
/// the user message and the diagnosis, exit 4").
fn report_start_failure(ctx: &mut Ctx<'_>, e: &dblocal_core::service::UserFacingError) -> u8 {
    output::stderr_line(ctx.err, &messages::code_line(&e.code, &e.message));
    if let Some(diag) = e.diagnosis {
        output::stderr_line(ctx.err, &messages::diagnosis_line(diag.as_str()));
    }
    if e.code == "DBL-1002" {
        EXIT_NOT_FOUND
    } else if e.code == "DBL-1001" || e.code == "DBL-1004" {
        EXIT_GENERAL
    } else {
        EXIT_ENGINE_FAILED
    }
}
