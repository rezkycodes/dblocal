//! Starts the flagged instances during `--autostart` (F-AUT-02).
//!
//! The runner starts every target independently, with at most
//! [`consts::AUTOSTART_CONCURRENCY`] in flight at once. The properties that
//! matter are all decided here and tested without an engine:
//!
//! - one failure never blocks the others (a failing target is recorded and the
//!   next one is started);
//! - there is no modal dialog on failure (a failed start only becomes the
//!   `failed` status the tray and window already render);
//! - at least two starts run concurrently, so a hanging engine cannot serialize
//!   the rest.
//!
//! The workers are scoped (`std::thread::scope`), so the start closure may
//! borrow the caller's service; nothing escapes the call and no `unsafe`
//! lifetime extension is needed.

use std::sync::Mutex;

use super::error::{AutostartError, AutostartResult};
use super::plan::{AutostartOutcome, AutostartSummary, AutostartTarget};

/// Runs every target with bounded concurrency and returns the summary.
///
/// The returned summary is in the same order as the input so a report is
/// deterministic regardless of completion order.
pub fn run_start_targets<F>(
    targets: &[AutostartTarget],
    start: F,
) -> AutostartResult<AutostartSummary>
where
    F: Fn(&AutostartTarget) -> Result<(), String> + Sync,
{
    if targets.is_empty() {
        return Ok(AutostartSummary::default());
    }
    let workers = consts_workers(targets.len());
    let slots: Mutex<Vec<Option<AutostartOutcome>>> = Mutex::new(vec![None; targets.len()]);
    let next = Mutex::new(0usize);

    std::thread::scope(|scope| -> AutostartResult<()> {
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let worker = std::thread::Builder::new()
                .name("dblocal-autostart".to_owned())
                .spawn_scoped(scope, || {
                    loop {
                        let index = {
                            let mut cursor = lock(&next);
                            if *cursor >= targets.len() {
                                break;
                            }
                            let index = *cursor;
                            *cursor += 1;
                            index
                        };
                        let target = &targets[index];
                        let outcome = match start(target) {
                            Ok(()) => AutostartOutcome::ok(target.clone()),
                            Err(message) => AutostartOutcome::err(target.clone(), message),
                        };
                        lock(&slots)[index] = Some(outcome);
                    }
                })
                .map_err(|err| AutostartError::PlanFailed {
                    detail: err.to_string(),
                })?;
            handles.push(worker);
        }
        for handle in handles {
            if handle.join().is_err() {
                return Err(AutostartError::PlanFailed {
                    detail: "an autostart worker panicked".to_owned(),
                });
            }
        }
        Ok(())
    })?;

    Ok(AutostartSummary {
        outcomes: outcome_vec(&slots, targets),
    })
}

fn outcome_vec(
    slots: &Mutex<Vec<Option<AutostartOutcome>>>,
    targets: &[AutostartTarget],
) -> Vec<AutostartOutcome> {
    lock(slots)
        .iter()
        .enumerate()
        .map(|(index, slot)| {
            slot.clone().unwrap_or_else(|| {
                // Cannot happen unless a worker died before writing; treating it
                // as a failure is safer than silently dropping the instance.
                AutostartOutcome::err(targets[index].clone(), "worker produced no result")
            })
        })
        .collect()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn consts_workers(len: usize) -> usize {
    use super::consts::AUTOSTART_CONCURRENCY;
    AUTOSTART_CONCURRENCY.min(len).max(1)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::model::InstanceId;

    fn target(name: &str) -> AutostartTarget {
        AutostartTarget {
            id: InstanceId::new(),
            name: name.to_owned(),
        }
    }

    #[test]
    fn every_target_is_attempted_in_order() {
        let targets = vec![target("a"), target("b"), target("c")];
        let summary = run_start_targets(&targets, |_| Ok(())).expect("run");
        assert_eq!(summary.outcomes.len(), 3);
        let names: Vec<&str> = summary
            .outcomes
            .iter()
            .map(|o| o.target.name.as_str())
            .collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        assert_eq!(summary.succeeded(), 3);
    }

    #[test]
    fn one_failure_does_not_block_the_others() {
        let targets = vec![target("a"), target("b"), target("c")];
        let summary = run_start_targets(&targets, |t| {
            if t.name == "b" {
                Err("DBL-1005 port dipakai".to_owned())
            } else {
                Ok(())
            }
        })
        .expect("run");
        assert_eq!(summary.succeeded(), 2);
        assert_eq!(summary.failed(), 1);
        let failed = summary
            .outcomes
            .iter()
            .find(|o| o.target.name == "b")
            .expect("b present");
        assert_eq!(
            failed.result.as_ref().err().map(String::as_str),
            Some("DBL-1005 port dipakai")
        );
    }

    #[test]
    fn starts_are_concurrent_not_serial() {
        // Two targets may only finish if both start before either finishes. Each
        // start waits (with a bounded deadline) for the other to be in flight; a
        // serial runner never sees the peer and times out, so the mutation turns
        // this red within seconds instead of deadlocking the test binary.
        let targets = vec![target("peer-a"), target("peer-b")];
        let live = AtomicUsize::new(0);
        let summary = run_start_targets(&targets, |_| {
            live.fetch_add(1, Ordering::SeqCst);
            let deadline = Instant::now() + Duration::from_secs(2);
            while live.load(Ordering::SeqCst) < 2 {
                if Instant::now() > deadline {
                    live.fetch_sub(1, Ordering::SeqCst);
                    return Err("the peer never started: the runner is serial".to_owned());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        })
        .expect("run");
        assert_eq!(
            summary.succeeded(),
            2,
            "both starts must be in flight at once; outcomes: {:?}",
            summary.outcomes
        );
    }

    #[test]
    fn a_slow_first_target_does_not_hold_back_a_fast_second() {
        let targets = vec![target("hung"), target("quick")];
        let quick_done = AtomicUsize::new(0);
        let summary = run_start_targets(&targets, |t| {
            if t.name == "hung" {
                std::thread::sleep(Duration::from_millis(300));
            } else {
                quick_done.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        })
        .expect("run");
        assert_eq!(summary.succeeded(), 2);
        assert_eq!(
            quick_done.load(Ordering::SeqCst),
            1,
            "the quick target must run while the hung one sleeps"
        );
    }

    #[test]
    fn concurrency_is_bounded() {
        let targets: Vec<AutostartTarget> = (0..12).map(|i| target(&format!("t{i}"))).collect();
        let live = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let summary = run_start_targets(&targets, |_| {
            let now = live.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            live.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        })
        .expect("run");
        assert_eq!(summary.outcomes.len(), 12);
        let peak = peak.load(Ordering::SeqCst);
        assert!(
            peak <= crate::autostart::consts::AUTOSTART_CONCURRENCY,
            "peak concurrency {peak} exceeded the bound"
        );
    }

    #[test]
    fn an_empty_plan_is_a_success_with_no_attempt() {
        let summary = run_start_targets(&[], |_| panic!("must not be called")).expect("run");
        assert!(!summary.has_any_attempt());
    }

    #[test]
    fn a_single_target_uses_one_worker() {
        let targets = vec![target("only")];
        let summary = run_start_targets(&targets, |_| Ok(())).expect("run");
        assert_eq!(summary.succeeded(), 1);
    }

    #[test]
    fn worker_count_never_exceeds_the_configured_bound() {
        assert_eq!(consts_workers(0), 1);
        assert_eq!(consts_workers(1), 1);
        assert_eq!(consts_workers(2), 2);
        assert_eq!(
            consts_workers(100),
            crate::autostart::consts::AUTOSTART_CONCURRENCY
        );
    }
}
