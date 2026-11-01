//! Tests of the scheduler with builds that stand in for real ones.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use laft_core::Variant;

use crate::BuildError;
use crate::graph::Item;
use crate::scheduler::{Failed, Memory, Scheduler};

/// The item `name`, as a dynamic build.
fn item(name: &str) -> Item {
    Item::new(name.parse().unwrap(), Variant::Dynamic)
}

/// The memory tests that do not talk about memory see: the host holds
/// everything, and no wait is announced.
fn plenty() -> Result<u64, crate::BuildError> {
    Ok(u64::MAX)
}

/// Announces nothing.
fn silent(_: u64, _: u64) {}

/// The scheduler of `order`, whose builds wait for `deps` and whose builds
/// take `durations` seconds before.
fn scheduler(
    order: &[&str],
    deps: &[(&str, &[&str])],
    durations: &[(&str, u64)],
    workers: usize,
) -> Scheduler<'static> {
    scheduler_with(
        order,
        deps,
        durations,
        workers,
        Memory {
            estimates: BTreeMap::new(),
            big: BTreeSet::new(),
            available: &plenty,
            waiting: &silent,
        },
    )
    .memory_wait(Duration::ZERO)
}

/// The scheduler of `order`, whose builds wait for `deps`, whose builds
/// take `durations` seconds before, and whose memory is `memory`.
fn scheduler_with<'a>(
    order: &[&str],
    deps: &[(&str, &[&str])],
    durations: &[(&str, u64)],
    workers: usize,
    memory: Memory<'a>,
) -> Scheduler<'a> {
    let items: Vec<Item> = order.iter().map(|name| item(name)).collect();
    let deps: BTreeMap<Item, std::collections::BTreeSet<Item>> = items
        .iter()
        .map(|entry| {
            let waits = deps
                .iter()
                .filter(|(name, _)| *name == entry.name.as_str())
                .flat_map(|(_, waits)| waits.iter())
                .map(|dep| item(dep))
                .collect();
            (entry.clone(), waits)
        })
        .collect();
    let durations: BTreeMap<Item, u64> = durations
        .iter()
        .map(|(name, seconds)| (item(name), *seconds))
        .collect();
    Scheduler::new(items, deps, durations, workers, memory)
}

#[test]
fn a_build_starts_after_the_builds_it_waits_for() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut scheduler = scheduler(
        &["a", "b", "c"],
        &[("c", &["a", "b"])],
        &[("a", 0), ("b", 0), ("c", 0)],
        3,
    );
    let seen_job = seen.clone();
    scheduler
        .run(
            ["a", "b", "c"].iter().map(|name| item(name)),
            &mut |_: &Item| (),
            &move |item, (): ()| {
                seen_job.lock().unwrap().push(format!("start {item}"));
                thread::sleep(Duration::from_millis(20));
                seen_job.lock().unwrap().push(format!("done {item}"));
                Ok(())
            },
            &mut |_, _| Ok(Vec::new()),
            None,
        )
        .expect("the run builds");
    let seen = seen.lock().unwrap().clone();
    let started = |what: &str, log: &[String]| {
        log.iter()
            .position(|line| line == &format!("start {what}"))
            .expect("every build starts")
    };
    let finished = |what: &str, log: &[String]| {
        log.iter()
            .position(|line| line == &format!("done {what}"))
            .expect("every build finishes")
    };
    assert!(
        started("c", &seen) > finished("a", &seen),
        "c starts after a: {}",
        seen.join(", ")
    );
    assert!(
        started("c", &seen) > finished("b", &seen),
        "c starts after b: {}",
        seen.join(", ")
    );
}

#[test]
fn no_more_builds_run_than_workers() {
    let open = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let mut scheduler = scheduler(
        &["a", "b", "c", "d"],
        &[],
        &[("a", 0), ("b", 0), ("c", 0), ("d", 0)],
        2,
    );
    let open_job = open.clone();
    let most_job = most.clone();
    let built = scheduler
        .run(
            ["a", "b", "c", "d"].iter().map(|name| item(name)),
            &mut |_: &Item| (),
            &move |item, (): ()| {
                let now = open_job.fetch_add(1, Ordering::SeqCst) + 1;
                most_job.fetch_max(now, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(30));
                open_job.fetch_sub(1, Ordering::SeqCst);
                Ok(format!("built {item}"))
            },
            &mut |_, _| Ok(Vec::new()),
            None,
        )
        .expect("the run builds");
    assert_eq!(built.len(), 4);
    assert!(
        most.load(Ordering::SeqCst) <= 2,
        "no more than two builds run side by side"
    );
}

#[test]
fn the_build_with_the_longest_chain_ahead_starts_first() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut scheduler = scheduler(
        &["a", "b", "c", "d"],
        &[("b", &["a"]), ("c", &["b"])],
        &[("a", 1), ("b", 1), ("c", 1), ("d", 2)],
        1,
    );
    let seen_job = seen.clone();
    scheduler
        .run(
            ["a", "b", "c", "d"].iter().map(|name| item(name)),
            &mut |_: &Item| (),
            &move |item, (): ()| {
                seen_job.lock().unwrap().push(item.to_string());
                Ok(())
            },
            &mut |_, _| Ok(Vec::new()),
            None,
        )
        .expect("the run builds");
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["a", "b", "d", "c"],
    );
}

#[test]
fn a_panicking_prepare_fails_the_run_and_wakes_the_workers() {
    let mut scheduler = scheduler(&["a", "b"], &[], &[("a", 0), ("b", 0)], 2);
    let mut message = Some(String::from("the plan went away"));
    let failed = scheduler
        .run(
            ["a", "b"].iter().map(|name| item(name)),
            &mut |_: &Item| {
                if let Some(message) = message.take() {
                    panic!("{message}");
                }
            },
            &|item, (): ()| Ok(format!("built {item}")),
            &mut |_, _| Ok(Vec::new()),
            None,
        )
        .expect_err("the run fails instead of hanging");
    let Failed::Build { item, error } = failed else {
        panic!("the run fails with a build, not {failed:?}");
    };
    assert_eq!(
        item,
        Item::new("a".parse().unwrap(), Variant::Dynamic),
        "the first build that had not finished"
    );
    assert!(error.to_string().contains("the plan went away"), "{error}");
}

#[test]
fn a_failure_stops_the_builds_that_wait_for_it() {
    let built = Arc::new(Mutex::new(Vec::new()));
    let mut scheduler = scheduler(
        &["a", "b", "c"],
        &[("c", &["b"])],
        &[("a", 0), ("b", 0), ("c", 0)],
        2,
    );
    let built_job = built.clone();
    let failed = scheduler
        .run(
            ["a", "b", "c"].iter().map(|name| item(name)),
            &mut |_: &Item| (),
            &move |item, (): ()| {
                built_job.lock().unwrap().push(item.to_string());
                match item.name.as_str() {
                    "b" => Err(BuildError::problem("B040", "b failed")),
                    _ => Ok(format!("built {item}")),
                }
            },
            &mut |_, _| Ok(Vec::new()),
            None,
        )
        .expect_err("the run fails");
    let Failed::Build { item, error } = failed else {
        panic!("the run fails with a build, not {failed:?}");
    };
    assert_eq!(
        item,
        Item::new("b".parse().unwrap(), Variant::Dynamic),
        "{error}"
    );
    assert!(error.to_string().contains("b failed"), "{error}");
    assert!(
        !built.lock().unwrap().iter().any(|name| name == "c"),
        "c never built: {:?}",
        built.lock().unwrap()
    );
}

#[test]
fn the_first_failure_in_plan_order_is_reported() {
    let mut scheduler = scheduler(&["a", "b"], &[], &[("a", 0), ("b", 0)], 2);
    let failed = scheduler
        .run(
            ["a", "b"].iter().map(|name| item(name)),
            &mut |_: &Item| (),
            &|item, (): ()| {
                if item.name.as_str() == "a" {
                    thread::sleep(Duration::from_millis(30));
                }
                Err(BuildError::problem("B040", format!("{item} failed")))
            },
            &mut |_: &Item, _: &()| Ok(Vec::new()),
            None,
        )
        .expect_err("the run fails");
    let Failed::Build { item, error } = failed else {
        panic!("the run fails with a build");
    };
    let first = Item::new("a".parse().unwrap(), Variant::Dynamic);
    assert_eq!(item, first, "b failed first, a comes first in the plan");
    assert!(error.to_string().contains("a failed"), "{error}");
}

#[test]
fn a_finished_build_makes_its_dependents_join_the_run() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut scheduler = scheduler(&["x", "y"], &[("y", &["x"])], &[("x", 0), ("y", 0)], 1);
    let seen_job = seen.clone();
    let built = scheduler
        .run(
            vec![item("x")],
            &mut |_: &Item| (),
            &move |item, (): ()| {
                seen_job.lock().unwrap().push(item.to_string());
                Ok(())
            },
            &mut |item, _: &()| {
                if item.name.as_str() == "x" {
                    Ok(vec![Item::new("y".parse().unwrap(), Variant::Dynamic)])
                } else {
                    Ok(Vec::new())
                }
            },
            None,
        )
        .expect("the run builds");
    let x = Item::new("x".parse().unwrap(), Variant::Dynamic);
    let y = Item::new("y".parse().unwrap(), Variant::Dynamic);
    assert_eq!(built, vec![(x, ()), (y, ())]);
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["x", "y"],
    );
}

#[test]
fn a_big_build_waits_for_the_memory_of_an_idle_host() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_look = calls.clone();
    let waits = Arc::new(AtomicUsize::new(0));
    let waits_announce = waits.clone();
    let look = move || {
        // The host frees the memory after the first waiting line.
        Ok(match calls_look.fetch_add(1, Ordering::SeqCst) {
            0 | 1 => 0,
            _ => 150,
        })
    };
    let announce = move |_, _| {
        waits_announce.fetch_add(1, Ordering::SeqCst);
    };
    let mut scheduler = scheduler_with(
        &["a"],
        &[],
        &[("a", 0)],
        1,
        Memory {
            estimates: BTreeMap::from([(item("a"), 100)]),
            big: BTreeSet::from([item("a")]),
            available: &look,
            waiting: &announce,
        },
    )
    .memory_wait(Duration::ZERO);
    scheduler
        .run(
            ["a"].iter().map(|name| item(name)),
            &mut |_: &Item| (),
            &|_, (): ()| Ok(()),
            &mut |_, _| Ok(Vec::new()),
            None,
        )
        .expect("the run builds");
    assert_eq!(waits.load(Ordering::SeqCst), 1, "the build waits once");
}

#[test]
fn a_big_build_that_is_deferred_again_waits_before_it_looks() {
    // The memory of the host oscillates between the reads, so the wait
    // sees it free and the fill defers the build again. The dispatcher
    // then waits before it reads the host again instead of spinning
    // between the two reads.
    let reads = [0u64, u64::MAX, 0, u64::MAX, 0, u64::MAX, u64::MAX];
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_look = calls.clone();
    let look = move || {
        let at = calls_look.fetch_add(1, Ordering::SeqCst);
        Ok(reads[at.min(reads.len() - 1)])
    };
    let silent = |_: u64, _: u64| {};
    let mut scheduler = scheduler_with(
        &["a"],
        &[],
        &[("a", 0)],
        1,
        Memory {
            estimates: BTreeMap::from([(item("a"), 100)]),
            big: BTreeSet::from([item("a")]),
            available: &look,
            waiting: &silent,
        },
    )
    .memory_wait(Duration::from_millis(100));
    let started = std::time::Instant::now();
    let built = scheduler
        .run(
            ["a"].iter().map(|name| item(name)),
            &mut |_: &Item| (),
            &|_, (): ()| Ok(()),
            &mut |_, _| Ok(Vec::new()),
            None,
        )
        .expect("the run builds");
    assert_eq!(built.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 7, "every read happened");
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "the dispatcher waits on the re-defer path"
    );
}

#[test]
fn only_one_big_build_runs_at_a_time() {
    let open = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let plenty = || Ok(u64::MAX);
    let silent = |_: u64, _: u64| ();
    let mut scheduler = scheduler_with(
        &["a", "b"],
        &[],
        &[("a", 0), ("b", 0)],
        2,
        Memory {
            estimates: BTreeMap::from([(item("a"), 0), (item("b"), 0)]),
            big: BTreeSet::from([item("a"), item("b")]),
            available: &plenty,
            waiting: &silent,
        },
    )
    .memory_wait(Duration::ZERO);
    let open_job = open.clone();
    let most_job = most.clone();
    let built = scheduler
        .run(
            ["a", "b"].iter().map(|name| item(name)),
            &mut |_: &Item| (),
            &move |item, (): ()| {
                let now = open_job.fetch_add(1, Ordering::SeqCst) + 1;
                most_job.fetch_max(now, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(20));
                open_job.fetch_sub(1, Ordering::SeqCst);
                Ok(format!("built {item}"))
            },
            &mut |_, _| Ok(Vec::new()),
            None,
        )
        .expect("the run builds");
    assert_eq!(built.len(), 2);
    assert_eq!(most.load(Ordering::SeqCst), 1, "the big builds take turns");
}

#[test]
fn the_next_build_that_fits_starts_instead() {
    let free = Arc::new(AtomicU64::new(0));
    let free_look = free.clone();
    let free_build = free.clone();
    let look = move || Ok(free_look.load(Ordering::SeqCst));
    let silent = |_: u64, _: u64| ();
    let mut scheduler = scheduler_with(
        &["a", "b"],
        &[],
        &[("a", 100), ("b", 1)],
        2,
        Memory {
            estimates: BTreeMap::from([(item("a"), 1000), (item("b"), 0)]),
            big: BTreeSet::from([item("a")]),
            available: &look,
            waiting: &silent,
        },
    )
    .memory_wait(Duration::ZERO);
    let built = scheduler
        .run(
            ["a", "b"].iter().map(|name| item(name)),
            &mut |_: &Item| (),
            &move |item, (): ()| {
                // Building b frees the memory that a waits for.
                free_build.fetch_add(2000, Ordering::SeqCst);
                Ok(item.name.to_string())
            },
            &mut |_, _| Ok(Vec::new()),
            None,
        )
        .expect("the run builds");
    let names: Vec<String> = built
        .into_iter()
        .map(|(item, _)| item.name.to_string())
        .collect();
    assert_eq!(names, ["b", "a"], "a waits, b starts instead");
}

#[test]
fn memory_that_vanishes_during_the_wait_does_not_end_the_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_look = calls.clone();
    let waits = Arc::new(AtomicUsize::new(0));
    let waits_announce = waits.clone();
    let look = move || {
        // The host holds the memory while the run waits for it, loses it
        // to another tenant right before the build starts, and holds it
        // again on the next wait.
        Ok(match calls_look.fetch_add(1, Ordering::SeqCst) {
            0 | 1 | 3 | 4 => 0,
            _ => 150,
        })
    };
    let announce = move |_, _| {
        waits_announce.fetch_add(1, Ordering::SeqCst);
    };
    let mut scheduler = scheduler_with(
        &["a"],
        &[],
        &[("a", 0)],
        1,
        Memory {
            estimates: BTreeMap::from([(item("a"), 100)]),
            big: BTreeSet::from([item("a")]),
            available: &look,
            waiting: &announce,
        },
    )
    .memory_wait(Duration::ZERO);
    let built = scheduler
        .run(
            ["a"].iter().map(|name| item(name)),
            &mut |_: &Item| (),
            &|_, (): ()| Ok(()),
            &mut |_, _| Ok(Vec::new()),
            None,
        )
        .expect("the run builds instead of ending as interrupted");
    assert_eq!(built.len(), 1);
    assert_eq!(waits.load(Ordering::SeqCst), 2, "the build waits twice");
}
