//! `dblocal update check|apply` (`F-PKG-05`).
//!
//! `check` queries GitHub Releases and prints the result (plain text or JSON),
//! including the release page URL and why one-click update is unavailable when
//! managed by a package manager or unsupported.
//!
//! `apply` streams progress (`downloading`, `verifying`, `installing`,
//! `ready to restart`) to stderr and refuses package-manager installs with
//! `DBL-8008` and the matching asset/release hint.

use dblocal_core::service::ServiceEvent;
use dblocal_core::update::UpdateProgress;

use crate::cli::{EXIT_BAD_ARGS, EXIT_GENERAL, EXIT_OK, UpdateCommand};
use crate::commands::Ctx;
use crate::messages;
use crate::output;

pub fn dispatch(ctx: &mut Ctx<'_>, command: &UpdateCommand) -> u8 {
    match command {
        UpdateCommand::Check { json } => check(ctx, *json),
        UpdateCommand::Apply { yes } => apply(ctx, *yes),
    }
}

fn check(ctx: &mut Ctx<'_>, json: bool) -> u8 {
    let status = match ctx.service.check_update() {
        Ok(status) => status,
        Err(err) => return output::report_facing(ctx.err, &err),
    };
    if json {
        output::stdout_line(ctx.out, &output::format_update_json(&status));
    } else if let Some(available) = &status.available {
        let mut line = format!("{} {}", messages::MSG_UPDATE_AVAILABLE, available.version);
        if !available.release_url.is_empty() {
            line.push_str(&format!(" ({})", available.release_url));
        }
        output::stdout_line(ctx.out, &line);
    } else {
        output::stdout_line(ctx.out, messages::MSG_UPDATE_UP_TO_DATE);
    }
    if let Some(reason) = &status.cannot_update_reason {
        output::stderr_line(ctx.err, reason);
    } else if let Some(hint) = &status.package_manager_hint {
        output::stderr_line(ctx.err, hint);
    }
    EXIT_OK
}

fn apply(ctx: &mut Ctx<'_>, yes: bool) -> u8 {
    if !yes && !ctx.is_interactive {
        output::stderr_line(
            ctx.err,
            &messages::code_line(
                dblocal_core::error::CODE_INVALID_INPUT,
                messages::ERR_UPDATE_NEEDS_YES,
            ),
        );
        return EXIT_BAD_ARGS;
    }

    let mut rx = ctx.service.subscribe();
    let op_id = match ctx.service.start_update() {
        Ok(id) => id,
        Err(err) => {
            let code = output::report_facing(ctx.err, &err);
            if err.code == dblocal_core::update::CODE_UPDATE_UNSUPPORTED_KIND {
                output::stderr_line(ctx.err, messages::MSG_UPDATE_PACKAGE_MANAGER);
            }
            return code;
        }
    };

    let mut last_pct: Option<u64> = None;
    loop {
        match rx.blocking_recv() {
            Ok(ServiceEvent::UpdateProgress { id, progress }) if id == op_id => {
                render_progress(ctx.err, &progress, &mut last_pct);
            }
            Ok(ServiceEvent::OperationFinished { id, result }) if id == op_id => match result {
                Ok(()) => return EXIT_OK,
                Err(err) => {
                    let code = output::report_facing(ctx.err, &err);
                    if err.code == dblocal_core::update::CODE_UPDATE_UNSUPPORTED_KIND {
                        output::stderr_line(ctx.err, messages::MSG_UPDATE_PACKAGE_MANAGER);
                    }
                    return code;
                }
            },
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                output::stderr_line(
                    ctx.err,
                    &messages::code_line(
                        dblocal_core::update::CODE_UPDATE_APPLY_FAILED,
                        "Update event stream closed unexpectedly.",
                    ),
                );
                return EXIT_GENERAL;
            }
        }
    }
}

fn render_progress(
    err: &mut dyn std::io::Write,
    progress: &UpdateProgress,
    last_pct: &mut Option<u64>,
) {
    match progress {
        UpdateProgress::Downloading {
            received_bytes,
            total_bytes,
        } => {
            if let Some(total) = total_bytes.filter(|&t| t > 0) {
                let pct = ((*received_bytes).saturating_mul(100) / total).min(100);
                // Throttle noisy identical percentages while always printing 0% and 100%.
                if *last_pct != Some(pct) {
                    *last_pct = Some(pct);
                    output::stderr_line(
                        err,
                        &messages::download_progress(pct, *received_bytes, total),
                    );
                }
            } else if last_pct.is_none() {
                *last_pct = Some(0);
                output::stderr_line(err, &messages::stage_line("downloading"));
            }
        }
        UpdateProgress::Verifying => {
            output::stderr_line(err, &messages::stage_line("verifying"));
        }
        UpdateProgress::Installing => {
            output::stderr_line(err, &messages::stage_line("installing"));
        }
        UpdateProgress::ReadyToRestart { version, .. } => {
            output::stderr_line(err, &messages::update_applied_version(version));
        }
        UpdateProgress::Cancelled | UpdateProgress::Failed { .. } => {}
        _ => {}
    }
}
