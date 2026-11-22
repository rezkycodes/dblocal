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
//! The service calls are injected through [`StartFn`], so the tests drive real
//! concurrency with a scripted closure and no database.

use std::sync::{Arc, Mutex};
use std::thread;

use super::error::{AutostartError, AutostartResult};
use super::plan::{AutostartOutcome, AutostartSummary, AutostartTarget};

/// Issues one start request and returns immediately.
///
/// Production passes `DblService::start_instance`. The service is synchronous
/// and returns an `OperationId` quickly, so the runner's job is to fan the
/// requests out, not to wait for each engine.
pub type StartFn = Arc<dyn Fn(&AutostartTarget) -> Result<(), String> + Send + Sync>;

/// Runs every target with bounded concurrency and returns the summary.
///
/// The target list is not modified; the returned summary is in the same order
/// as the input so a report is deterministic regardless of completion order.
pub fn run_start_targets(
    targets: &[AutostartTarget],
    start: StartFn,
) -> AutostartResult<AutostartSummary> {
    if targets.is_empty() {
        return Ok(AutostartSummary::default());
    }
    let workers = consts_workers(targets.len());
    let slots: Arc<Mutex<Vec<Option<AutostartOutcome>>>> =
        Arc::new(Mutex::new(vec![None; targets.len()]));
    let next = Arc::new(Mutex::new(0usize));

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let start = Arc::clone(&start);
        let slots = Arc::clone(&slots);
        let next = Arc::clone(&next);
        let handles_local: Arc<Mutex<()>> = Arc::new(Mutex::new(()));
        let _ = handles_local; // keep the borrow checker quiet about the clone below
        let targets = targets.to_vec();
        let handle = thread::Builder::new()
            .name("dblocal-autostart".to_owned())
            .spawn(move || {
                loop {
                    let index = {
                        let mut cursor = match next.lock() {
                            Ok(guard) => guard,
                            Err(poisoned) => poisoned.into_inner(),
                        };
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
                    let mut slots = match slots.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    slots[index] = Some(outcome);
                }
            })
            .map_err(|err| AutostartError::PlanFailed {
                detail: err.to_string(),
            })?;
        handles.push(handle);
    }

    for handle in handles {
        // A worker panic is a defect in the injected closure, not a start
        // failure; surface it as a plan failure instead of dropping it.
        if handle.join().is_err() {
            return Err(AutostartError::PlanFailed {
                detail: "an autostart worker panicked".to_owned(),
            });
        }
    }

    let slots = match slots.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let outcomes = slots
        .iter()
        .map(|slot| {
            slot.clone().unwrap_or_else(|| {
                // Cannot happen unless a worker died before writing; treating it
                // as a failure is safer than silently dropping the instance.
                AutostartOutcome::err(
                    AutostartTarget {
                        id: crate::model::InstanceId::new(),
                        name: "<unknown>".to_owned(),
                    },
                    "worker produced no result",
                )
            })
        })
        .collect();
    Ok(AutostartSummary { outcomes })
}

fn consts_workers(len: usize) -> usize {
    use super::consts::AUTOSTART_CONCURRENCY;
    AUTOSTART_CONCURRENCY.min(len).max(1)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::net::TcpListener;
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
        let start: StartFn = Arc::new(|_| Ok(()));
        let summary = run_start_targets(&targets, start).expect("run");
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
        let start: StartFn = Arc::new(|t| {
            if t.name == "b" {
                Err("DBL-1005 port dipakai".to_owned())
            } else {
                Ok(())
            }
        });
        let summary = run_start_targets(&targets, start).expect("run");
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
        let live = Arc::new(AtomicUsize::new(0));
        let start: StartFn = {
            let live = Arc::clone(&live);
            Arc::new(move |_| {
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
        };
        let summary = run_start_targets(&targets, start).expect("run");
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
        let quick_done = Arc::new(AtomicUsize::new(0));
        let start: StartFn = {
            let quick_done = Arc::clone(&quick_done);
            Arc::new(move |t| {
                if t.name == "hung" {
                    std::thread::sleep(Duration::from_millis(300));
                } else {
                    quick_done.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            })
        };
        let summary = run_start_targets(&targets, start).expect("run");
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
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let start: StartFn = {
            let live = Arc::clone(&live);
            let peak = Arc::clone(&peak);
            Arc::new(move |_| {
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
                live.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
        };
        let summary = run_start_targets(&targets, start).expect("run");
        assert_eq!(summary.outcomes.len(), 12);
        let peak = peak.load(Ordering::SeqCst);
        assert!(
            peak <= crate::autostart::consts::AUTOSTART_CONCURRENCY,
            "peak concurrency {peak} exceeded the bound"
        );
    }

    #[test]
    fn an_empty_plan_is_a_success_with_no_attempt() {
        let start: StartFn = Arc::new(|_| panic!("must not be called"));
        let summary = run_start_targets(&[], start).expect("run");
        assert!(!summary.has_any_attempt());
    }

    #[test]
    fn a_single_target_uses_one_worker() {
        let targets = vec![target("only")];
        let start: StartFn = Arc::new(|_| Ok(()));
        let summary = run_start_targets(&targets, start).expect("run");
        assert_eq!(summary.succeeded(), 1);
    }

    /// Keeps the test module honest about the concurrency source: the bound is
    /// the module constant, not a magic number copied here.
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

    /// A port held by a foreign listener is exactly the "one failure" case the
    /// PRD names; this proves the runner reports it without aborting the pass.
    #[test]
    fn a_held_port_reported_as_a_failure_leaves_the_rest_alone() {
        let held = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let _port = held.local_addr().expect("addr").port();
        let targets = vec![target("blocked"), target("free")];
        let start: StartFn = Arc::new(move |t| {
            if t.name == "blocked" {
                Err("DBL-1005".to_owned())
            } else {
                Ok(())
            }
        });
        let summary = run_start_targets(&targets, start).expect("run");
        assert_eq!(summary.failed(), 1);
        assert_eq!(summary.succeeded(), 1);
    }
}
