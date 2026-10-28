use std::collections::BTreeSet;

use laft_core::{BootEvent, BootEventKind, BootState, BootTrial, Timestamp};

use super::{Decision, decide, menu_index, set_default_entry};

fn now() -> Timestamp {
    "2026-10-28T14:02:00Z".parse().unwrap()
}

fn on_trial(generation: u64, tries_left: u32) -> BootState {
    BootState {
        good: BTreeSet::from([41]),
        last_good: Some(41),
        trial: Some(BootTrial {
            generation,
            tries_left,
        }),
        ..BootState::default()
    }
}

fn any(_: u64) -> bool {
    true
}

#[test]
fn generation_not_on_trial_changes_nothing() {
    assert_eq!(
        decide(BootState::default(), 42, 3, false, any, now()),
        Decision::Unchanged
    );
    assert_eq!(
        decide(on_trial(43, 2), 42, 3, false, any, now()),
        Decision::Unchanged
    );
}

#[test]
fn notries_and_disabled_counting_change_nothing() {
    assert_eq!(
        decide(on_trial(42, 2), 42, 3, true, any, now()),
        Decision::Unchanged
    );
    assert_eq!(
        decide(on_trial(42, 2), 42, 0, false, any, now()),
        Decision::Unchanged
    );
}

#[test]
fn each_boot_on_trial_consumes_one_try() {
    let Decision::Counted(state) = decide(on_trial(42, 2), 42, 3, false, any, now()) else {
        panic!("a try is consumed");
    };
    assert_eq!(state, on_trial(42, 1));
    let Decision::Counted(state) = decide(state, 42, 3, false, any, now()) else {
        panic!("a try is consumed");
    };
    assert_eq!(state, on_trial(42, 0));
}

#[test]
fn exhausted_trial_falls_back_to_the_last_good_generation() {
    let decision = decide(on_trial(42, 0), 42, 3, false, any, now());
    let mut expected = on_trial(42, 0);
    expected.trial = None;
    expected.bad.insert(42);
    expected.events.push(BootEvent {
        from: 42,
        kind: BootEventKind::Fallback,
        time: now(),
        to: 41,
    });
    assert_eq!(
        decision,
        Decision::Fallback {
            state: expected,
            to: 41
        }
    );
}

#[test]
fn exhausted_trial_without_a_usable_good_generation_boots_anyway() {
    let mut no_good = on_trial(42, 0);
    no_good.last_good = None;
    let Decision::Exhausted(state) = decide(no_good, 42, 3, false, any, now()) else {
        panic!("the trial is exhausted");
    };
    assert_eq!(state.trial, None);
    assert!(state.bad.contains(&42));
    assert!(state.events.is_empty());

    let Decision::Exhausted(_) = decide(on_trial(42, 0), 42, 3, false, |_| false, now()) else {
        panic!("an invalid last good generation is no fallback");
    };
}

#[test]
fn only_the_most_recent_events_are_kept() {
    let mut state = on_trial(42, 0);
    for n in 0..20 {
        state.events.push(BootEvent {
            from: n,
            kind: BootEventKind::Fallback,
            time: Timestamp::from_secs(n),
            to: n + 1,
        });
    }
    let Decision::Fallback { state, .. } = decide(state, 42, 3, false, any, now()) else {
        panic!("falls back");
    };
    assert_eq!(state.events.len(), 20);
    assert_eq!(state.events[0].from, 1);
    assert_eq!(state.events[19].from, 42);
}

#[test]
fn menu_index_is_looked_up_by_generation() {
    let entries = br#"{"default_gen": 42, "entries": [{"gen": 42, "index": 1}, {"gen": 41, "index": 2}], "version": 1}"#;
    assert_eq!(menu_index(entries, 41), Ok(Some(2)));
    assert_eq!(menu_index(entries, 40), Ok(None));
    assert!(menu_index(br#"{"entries": [], "version": 2}"#, 41).is_err());
}

#[test]
fn only_the_default_entry_line_is_replaced() {
    let conf = "# Rendered by laft.\ntimeout: 3\ndefault_entry: 1\nremember_last_entry: no\n\n/Laft 42\n    cmdline: laft.gen=42 default_entry: 1\n";
    assert_eq!(
        set_default_entry(conf, 2).as_deref(),
        Some(
            "# Rendered by laft.\ntimeout: 3\ndefault_entry: 2\nremember_last_entry: no\n\n/Laft 42\n    cmdline: laft.gen=42 default_entry: 1\n"
        )
    );
    assert_eq!(
        set_default_entry("default_entry: 7", 12).as_deref(),
        Some("default_entry: 12")
    );
}

#[test]
fn configuration_without_a_default_entry_is_left_alone() {
    assert_eq!(set_default_entry("timeout: 3\n", 2), None);
    assert_eq!(set_default_entry("default_entry: x\n", 2), None);
    assert_eq!(set_default_entry(" default_entry: 1\n", 2), None);
}
