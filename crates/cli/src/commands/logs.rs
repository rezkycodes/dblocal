use std::time::Duration;

use dblocal_core::model::InstanceId;
use dblocal_core::service::DblService;

use crate::cli::EXIT_OK;
use crate::commands::{Ctx, find_instance};
use crate::output;

/// How often the follower asks the service for the current tail.
const FOLLOW_INTERVAL: Duration = Duration::from_millis(crate::consts::LOG_FOLLOW_INTERVAL_MS);
/// How many lines the follower re-reads each time. A window this size is enough
/// to see an append between polls; the first print already showed the history.
const FOLLOW_WINDOW: usize = crate::consts::LOG_FOLLOW_WINDOW;

pub fn run(ctx: &mut Ctx<'_>, name: &str, follow: bool, lines: usize) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let initial = match ctx.service.tail_logs(&view.id, lines) {
        Ok(chunk) => chunk,
        Err(e) => return output::report_error(ctx.err, &e),
    };
    for line in &initial {
        output::stdout_line(ctx.out, line);
    }
    if !follow {
        return EXIT_OK;
    }
    let _ = ctx.out.flush();

    follow_tail(ctx, &view.id, initial)
}

/// Follows an instance's log through the service's own reader until SIGINT.
///
/// The service exposes `tail_logs`, not a streaming reader for the CLI, and its
/// live-log event requires enabling a watcher for the GUI's benefit. Re-reading
/// the tail on a fixed interval and printing only the lines past the previous
/// snapshot keeps this on the service path (no ad-hoc file reads; rotation and
/// truncation are handled by `process::logs`) without a busy loop.
///
/// Ctrl-C is caught so the command exits 0 as the spec requires; the future
/// waits on the signal and the interval rather than polling a flag.
fn follow_tail(ctx: &mut Ctx<'_>, id: &InstanceId, mut previous: Vec<String>) -> u8 {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            output::stderr_line(ctx.err, &format!("[DBL-4001] runtime: {e}"));
            return crate::cli::EXIT_GENERAL;
        }
    };
    runtime.block_on(async {
        let mut ticker = tokio::time::interval(FOLLOW_INTERVAL);
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => return EXIT_OK,
                _ = ticker.tick() => {}
            }
            let Some(next) = poll_tail(ctx.service, id) else {
                return EXIT_OK;
            };
            let skip = overlap(&previous, &next);
            for line in next.iter().skip(skip) {
                output::stdout_line(ctx.out, line);
            }
            let _ = ctx.out.flush();
            previous = next;
        }
    })
}

fn poll_tail(service: &dyn DblService, id: &InstanceId) -> Option<Vec<String>> {
    match service.list_instances() {
        Ok(views) if !views.iter().any(|v| &v.id == id) => None,
        _ => service.tail_logs(id, FOLLOW_WINDOW).ok(),
    }
}

/// Largest `k` such that the last `k` lines of `old` equal the first `k` of `new`.
fn overlap(old: &[String], new: &[String]) -> usize {
    let max = old.len().min(new.len());
    (0..=max)
        .rev()
        .find(|k| old[old.len() - k..] == new[..*k])
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::overlap;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn overlap_finds_the_shared_boundary() {
        let old = lines(&["a", "b", "c"]);
        assert_eq!(overlap(&old, &lines(&["b", "c", "d"])), 2);
        assert_eq!(overlap(&old, &lines(&["a", "b", "c"])), 3);
        assert_eq!(overlap(&old, &lines(&["x"])), 0);
        assert_eq!(overlap(&[], &lines(&["x"])), 0);
        assert_eq!(overlap(&old, &[]), 0);
    }

    #[test]
    fn overlap_prefers_the_longest_match() {
        // A repeated line must not make an earlier window look like the boundary.
        let old = lines(&["x", "a", "x"]);
        assert_eq!(overlap(&old, &lines(&["x", "y"])), 1);
    }
}
