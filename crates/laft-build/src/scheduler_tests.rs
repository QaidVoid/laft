//! Tests of the scheduler with builds that stand in for real ones.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use laft_core::Variant;

use crate::BuildError;
use crate::graph::Item;
use crate::scheduler::{Failed, Scheduler};

/// The item `name`, as a dynamic build.
fn item(name: &str) -> Item {
    Item::new(name.parse().unwrap(), Variant::Dynamic)
}

/// The scheduler of `order`, whose builds wait for `deps` and whose builds
/// take `durations` seconds before.
fn scheduler(
    order: &[&str],
    deps: &[(&str, &[&str])],
    durations: &[(&str, u64)],
    workers: usize,
) -> Scheduler {
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
    Scheduler::new(items, deps, durations, workers)
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
        )
        .expect("the run builds");
    assert_eq!(built.len(), 4);
    assert_eq!(
        most.load(Ordering::SeqCst),
        2,
        "two builds run side by side"
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
fn a_failure_stops_the_builds_that_wait_for_it() {
    let mut scheduler = scheduler(
        &["a", "b", "c"],
        &[("c", &["b"])],
        &[("a", 0), ("b", 0), ("c", 0)],
        2,
    );
    let built = scheduler
        .run(
            ["a", "b", "c"].iter().map(|name| item(name)),
            &mut |_: &Item| (),
            &|item, (): ()| match item.name.as_str() {
                "b" => Err(BuildError::problem("B040", "b failed")),
                _ => Ok(format!("built {item}")),
            },
            &mut |_, _| Ok(Vec::new()),
        )
        .expect_err("the run fails");
    let Failed::Build { item, error } = built else {
        panic!("the run fails with a build, not {built:?}");
    };
    assert_eq!(
        item,
        Item::new("b".parse().unwrap(), Variant::Dynamic),
        "{error}"
    );
    assert!(error.to_string().contains("b failed"), "{error}");
    // `c` never started, and the run holds no outcome for it.
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
