use std::fs;
use std::os::unix::fs::symlink;

use laft_core::{BootEvent, BootEventKind, BootState, BootTrial, MAX_BOOT_EVENTS, Timestamp};

use super::*;
use crate::test_support::Env;

fn state(env: &Env) -> BootState {
    BootState::parse(&fs::read(env.path("laft/state/boot.json")).unwrap()).unwrap()
}

fn trial(generation: u64, tries_left: u32) -> Option<BootTrial> {
    Some(BootTrial {
        generation,
        tries_left,
    })
}

#[test]
fn a_new_default_starts_a_trial_once() {
    let env = Env::new();
    assert_eq!(start_trial(&env.root, 42, 3).unwrap(), trial(42, 3));
    assert_eq!(state(&env).trial, trial(42, 3));

    let mut used = state(&env);
    used.trial = trial(42, 1);
    fs::write(env.path("laft/state/boot.json"), used.to_json().unwrap()).unwrap();
    assert_eq!(
        start_trial(&env.root, 42, 3).unwrap(),
        trial(42, 1),
        "installing the same default again keeps the tries it used"
    );
    assert_eq!(start_trial(&env.root, 43, 2).unwrap(), trial(43, 2));
}

#[test]
fn a_good_default_or_disabled_counting_needs_no_trial() {
    let env = Env::new();
    start_trial(&env.root, 42, 3).unwrap();
    let lock = env.lock();
    mark_good(&lock, 41).unwrap();
    assert_eq!(start_trial(&env.root, 41, 3).unwrap(), None);
    assert_eq!(state(&env).trial, None);
    assert_eq!(start_trial(&env.root, 43, 0).unwrap(), None);
    assert_eq!(state(&env).trial, None);
}

#[test]
fn marking_good_ends_its_trial_and_records_it() {
    let env = Env::new();
    let lock = env.lock();
    start_trial(&env.root, 42, 3).unwrap();
    let mut failed = state(&env);
    failed.bad.insert(42);
    fs::write(env.path("laft/state/boot.json"), failed.to_json().unwrap()).unwrap();

    let marked = mark_good(&lock, 41).unwrap();
    assert_eq!(
        marked.trial,
        trial(42, 3),
        "another generation's trial stays"
    );
    let marked = mark_good(&lock, 42).unwrap();
    assert_eq!(marked, state(&env));
    assert_eq!(marked.trial, None);
    assert_eq!(marked.last_good, Some(42));
    assert!(marked.good.contains(&41) && marked.good.contains(&42));
    assert!(marked.bad.is_empty(), "a generation that booted is not bad");
}

#[test]
fn the_booted_generation_is_marked_good() {
    let env = Env::new();
    let lock = env.lock();
    fs::create_dir_all(env.path("run/laft")).unwrap();
    symlink("mounts/41", env.path("run/laft/booted")).unwrap();
    assert_eq!(mark_booted_good(&lock).unwrap(), 41);
    assert_eq!(state(&env).last_good, Some(41));
}

#[test]
fn events_are_recorded_up_to_the_limit() {
    let env = Env::new();
    let lock = env.lock();
    for n in 0..=MAX_BOOT_EVENTS as u64 {
        record_event(
            &lock,
            BootEvent {
                from: n + 1,
                kind: BootEventKind::Fallback,
                time: Timestamp::from_secs(n),
                to: n,
            },
        )
        .unwrap();
    }
    let events = state(&env).events;
    assert_eq!(events.len(), MAX_BOOT_EVENTS);
    assert_eq!(events[0].from, 2);
}

#[test]
fn an_unreadable_state_is_refused_and_left_alone() {
    let env = Env::new();
    env.write("laft/state/boot.json", r#"{"version":2}"#);
    assert!(start_trial(&env.root, 42, 3).is_err());
    assert!(mark_good(&env.lock(), 42).is_err());
    assert_eq!(env.read("laft/state/boot.json"), r#"{"version":2}"#);
}
