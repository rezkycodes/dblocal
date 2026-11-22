use std::time::Duration;

use dblocal_core::DblError;
use dblocal_core::model::InstanceId;
use dblocal_core::service::DblService;

use crate::cli::EXIT_OK;
use crate::commands::{Ctx, find_instance};
use crate::messages;
use crate::output;

/// How often the follower asks the service for the current tail.
const FOLLOW_INTERVAL: Duration = Duration::from_millis(crate::consts::LOG_FOLLOW_INTERVAL_MS);
/// How many lines the follower re-reads each time.
///
/// Every snapshot after the first is exactly this window from the end of the
/// file, so two consecutive snapshots overlap by everything that was appended
/// in between (unless more than the window arrived, in which case the gap is
/// unavoidable without timestamps).
const FOLLOW_WINDOW: usize = crate::consts::LOG_FOLLOW_WINDOW;

pub fn run(ctx: &mut Ctx<'_>, name: &str, follow: bool, lines: usize) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(v) => v,
        Err(code) => return code,
    };

    // ONE snapshot, taken with the follow window so the first diff is against
    // the same window the ticker reads next. Only its last `lines` lines are
    // history; the rest is context for the diff and never printed.
    let window = if follow {
        lines.max(FOLLOW_WINDOW)
    } else {
        lines
    };
    let snapshot = match ctx.service.tail_logs(&view.id, window) {
        Ok(chunk) => chunk,
        Err(e) => return output::report_error(ctx.err, &e),
    };
    let follower = Follower::new(snapshot);
    for line in follower.history(lines) {
        output::stdout_line(ctx.out, line);
    }
    if !follow {
        return EXIT_OK;
    }
    let _ = ctx.out.flush();

    follow_tail(ctx, &view.id, follower)
}

/// Follows an instance's log through the service's own reader until SIGINT.
///
/// The service exposes `tail_logs`, not a streaming reader for the CLI, and its
/// live-log event requires enabling a watcher for the GUI's benefit. Re-reading
/// the tail on a fixed interval and printing only the lines past the previous
/// snapshot keeps this on the service path (no ad-hoc file reads; rotation and
/// truncation are handled by `process::logs`) without a busy loop.
///
/// The SIGINT future is created once, before the loop, so a signal that arrives
/// while a read is in flight is still delivered to the next `select!` instead of
/// being re-armed (and lost) after every tick.
///
/// A read that fails transiently keeps the previous snapshot and retries on the
/// next tick; only a missing instance ends the loop.
fn follow_tail(ctx: &mut Ctx<'_>, id: &InstanceId, mut follower: Follower) -> u8 {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            output::stderr_line(ctx.err, &messages::runtime_line(&e.to_string()));
            return crate::cli::EXIT_GENERAL;
        }
    };
    runtime.block_on(async {
        let mut ticker = tokio::time::interval(FOLLOW_INTERVAL);
        let mut ctrl_c = std::pin::pin!(tokio::signal::ctrl_c());
        loop {
            tokio::select! {
                _ = &mut ctrl_c => return EXIT_OK,
                _ = ticker.tick() => {}
            }
            let lines = match follower.poll(ctx.service, id) {
                Poll::Gone => return EXIT_OK,
                Poll::Retry => continue,
                Poll::Lines(lines) => lines,
            };
            print_lines(ctx, &lines);
        }
    })
}

/// Prints new lines and flushes, so a piped reader sees them without delay.
fn print_lines(ctx: &mut Ctx<'_>, lines: &[String]) {
    for line in lines {
        output::stdout_line(ctx.out, line);
    }
    let _ = ctx.out.flush();
}

/// The follower's state: the last snapshot, plus the diff needed to print only
/// what is new.
pub struct Follower {
    previous: Vec<String>,
}

/// Outcome of one follow poll.
#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    /// New lines to print, oldest first. Empty when nothing was appended.
    Lines(Vec<String>),
    /// The read failed for a reason a retry can fix; keep the snapshot.
    Retry,
    /// The instance no longer exists, so there is nothing left to follow.
    Gone,
}

impl Follower {
    /// Starts from a snapshot already read by the caller.
    pub fn new(previous: Vec<String>) -> Self {
        Self { previous }
    }

    /// The history to print once, before following: the last `lines` lines of
    /// the first snapshot. `--lines 0` prints nothing, but the snapshot is still
    /// kept so the first tick diffs against real content.
    pub fn history(&self, lines: usize) -> &[String] {
        let skip = self.previous.len().saturating_sub(lines);
        &self.previous[skip..]
    }

    /// Reads one window and advances the snapshot.
    ///
    /// A transient failure is reported as [`Poll::Retry`] and leaves `previous`
    /// untouched, so the next successful read is diffed against the last
    /// snapshot that worked rather than against an empty one.
    pub fn poll(&mut self, service: &dyn DblService, id: &InstanceId) -> Poll {
        match service.tail_logs(id, FOLLOW_WINDOW) {
            Ok(next) => {
                let fresh = new_lines(&self.previous, &next).to_vec();
                self.previous = next;
                Poll::Lines(fresh)
            }
            Err(DblError::NotFound { .. }) => Poll::Gone,
            Err(_) => Poll::Retry,
        }
    }
}

/// Lines of `next` that were not in `previous`.
///
/// Both slices are windows onto the end of the same growing file, so the
/// boundary is the longest suffix of `previous` that is also a prefix of
/// `next`. Rotation, truncation and a window that rotated past `previous` all
/// fall back to "no overlap", which means the whole new window is new content
/// and is printed once.
///
/// Limitation, from having no timestamps: when the window is completely full
/// and every line in it is the same text, an appended copy of that text is
/// indistinguishable from the existing ones and is swallowed. Everything else,
/// including a short run of duplicates, is reported correctly.
pub fn new_lines<'a>(previous: &[String], next: &'a [String]) -> &'a [String] {
    &next[overlap(previous, next)..]
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
mod tests;
