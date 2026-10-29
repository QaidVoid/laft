use std::collections::{BTreeMap, BTreeSet};

use laft_core::{BootEvent, BootEventKind, BootState, BootTrial, Timestamp};

use super::{
    Plan, Step, Then, decide, menu_index, menu_position, retarget_menu, set_default_entry,
};
use crate::generation::Generation;

const TRIES: u32 = 3;

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

/// Generation `number` with the kernel and initramfs named `kernel`.
fn generation(number: u64, kernel: &str) -> Generation {
    Generation {
        number,
        backend: "runit".into(),
        exec: vec!["/usr/bin/runit".into()],
        kernel_id: kernel.into(),
        kernel_release: "6.17.5-laft1".into(),
        initramfs: kernel.into(),
    }
}

fn plan(state: BootState, booting: u64, notries: bool, tries: u32) -> Plan {
    let target = generation(41, "k");
    decide(
        state,
        &generation(booting, "k"),
        tries,
        notries,
        |n| {
            if n == 41 {
                Ok(target.clone())
            } else {
                Err("missing".into())
            }
        },
        now(),
    )
}

fn boots(number: u64, steps: Vec<Step>) -> Plan {
    Plan {
        steps,
        then: Then::Boot(generation(number, "k")),
    }
}

/// The state after generation 42 failed and 41 replaced it.
fn fallen_back() -> BootState {
    let mut state = on_trial(42, 0);
    state.trial = None;
    state.bad.insert(42);
    state.push_event(BootEvent {
        from: 42,
        kind: BootEventKind::Fallback,
        time: now(),
        to: 41,
    });
    state
}

#[test]
fn generation_not_on_trial_changes_nothing() {
    assert_eq!(
        plan(BootState::default(), 42, false, TRIES),
        boots(42, vec![])
    );
    assert_eq!(plan(on_trial(43, 2), 42, false, TRIES), boots(42, vec![]));
}

#[test]
fn notries_and_disabled_counting_change_nothing() {
    assert_eq!(plan(on_trial(42, 2), 42, true, TRIES), boots(42, vec![]));
    assert_eq!(plan(on_trial(42, 2), 42, false, 0), boots(42, vec![]));
    assert_eq!(plan(on_trial(42, 0), 42, true, TRIES), boots(42, vec![]));
}

#[test]
fn each_boot_on_trial_consumes_one_try() {
    assert_eq!(
        plan(on_trial(42, 2), 42, false, TRIES),
        boots(42, vec![Step::Save(on_trial(42, 1))])
    );
    assert_eq!(
        plan(on_trial(42, 1), 42, false, TRIES),
        boots(42, vec![Step::Save(on_trial(42, 0))])
    );
}

#[test]
fn the_trial_ends_only_after_the_default_and_the_boot_menu_point_at_the_fallback() {
    assert_eq!(
        plan(on_trial(42, 0), 42, false, TRIES),
        Plan {
            steps: vec![
                Step::SetDefault {
                    to: 41,
                    otherwise: generation(41, "k"),
                },
                Step::SetBootEntry {
                    to: 41,
                    otherwise: generation(41, "k"),
                },
                Step::Save(fallen_back()),
                Step::Warn("generation 42 failed 3 boots; continuing with generation 41".into()),
            ],
            then: Then::Boot(generation(41, "k")),
        }
    );
}

#[test]
fn a_used_up_trial_without_a_good_generation_boots_anyway_and_says_why() {
    let mut no_good = on_trial(42, 0);
    no_good.last_good = None;
    let mut expected = no_good.clone();
    expected.trial = None;
    expected.bad.insert(42);
    assert_eq!(
        plan(no_good, 42, false, TRIES),
        boots(
            42,
            vec![
                Step::Error(
                    "generation 42 failed 3 boots and there is no known good generation; \
                     booting it anyway"
                        .into()
                ),
                Step::Save(expected),
            ]
        )
    );
}

#[test]
fn an_unreadable_fallback_target_boots_the_failed_generation_and_says_why() {
    let mut gone = on_trial(42, 0);
    gone.last_good = Some(40);
    let Plan { steps, then } = plan(gone, 42, false, TRIES);
    assert_eq!(then, Then::Boot(generation(42, "k")));
    let [Step::Error(message), Step::Save(state)] = steps.as_slice() else {
        panic!("logs and saves: {steps:?}");
    };
    assert_eq!(
        message,
        "generation 42 failed 3 boots and the last good generation 40 cannot be booted \
         (missing); booting it anyway"
    );
    assert_eq!(state.trial, None);
    assert!(state.bad.contains(&42) && state.events.is_empty());
}

/// How one simulated boot ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    Booted(u64),
    Rebooted,
    PowerLost,
}

/// The persistent parts of a machine that boot counting touches.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Machine {
    state: BootState,
    default_link: u64,
    /// The generation the default boot menu entry starts with `laft.gen=`.
    menu: u64,
    menu_writable: bool,
    link_writable: bool,
    gens: BTreeMap<u64, Generation>,
}

impl Machine {
    /// Generation 42 has used up its trial and 41 is the last good one.
    /// Their kernels are `old` and `new`.
    fn failing(old: &str, new: &str) -> Self {
        Self {
            state: on_trial(42, 0),
            default_link: 42,
            menu: 42,
            menu_writable: true,
            link_writable: true,
            gens: BTreeMap::from([(41, generation(41, old)), (42, generation(42, new))]),
        }
    }

    /// Boots the default menu entry and carries out the plan, losing power
    /// once `power_lost_after` steps are done.
    fn boot(&mut self, power_lost_after: Option<usize>) -> Ended {
        let booting = self.gens[&self.menu].clone();
        let gens = self.gens.clone();
        let plan = decide(
            self.state.clone(),
            &booting,
            TRIES,
            false,
            |n| gens.get(&n).cloned().ok_or_else(|| "missing".to_owned()),
            now(),
        );
        for (done, step) in plan.steps.into_iter().enumerate() {
            if power_lost_after == Some(done) {
                return Ended::PowerLost;
            }
            match step {
                Step::SetDefault { to, otherwise } => {
                    if !self.link_writable {
                        return Ended::Booted(otherwise.number);
                    }
                    self.default_link = to;
                }
                Step::SetBootEntry { to, otherwise } => {
                    if !self.menu_writable {
                        return Ended::Booted(otherwise.number);
                    }
                    self.menu = to;
                }
                Step::Save(state) => self.state = state,
                Step::Warn(_) | Step::Error(_) => {}
            }
        }
        if power_lost_after.is_some() {
            return Ended::PowerLost;
        }
        match plan.then {
            Then::Boot(generation) => Ended::Booted(generation.number),
            Then::Reboot => Ended::Rebooted,
        }
    }

    /// Boots until a generation runs, at most three times.
    fn boot_to_the_end(&mut self) -> u64 {
        for _ in 0..3 {
            if let Ended::Booted(n) = self.boot(None) {
                return n;
            }
        }
        panic!("the machine never finishes booting: {self:?}");
    }

    fn fell_back(&self) -> bool {
        self.state == fallen_back() && self.default_link == 41 && self.menu == 41
    }
}

#[test]
fn a_fallback_in_place_lasts_beyond_the_next_boot() {
    let mut machine = Machine::failing("k", "k");
    assert_eq!(machine.boot(None), Ended::Booted(41));
    assert!(machine.fell_back(), "{machine:?}");
    assert_eq!(machine.boot(None), Ended::Booted(41));
    assert!(machine.fell_back(), "{machine:?}");
}

#[test]
fn a_fallback_to_another_kernel_reboots_into_it_and_stays() {
    let mut machine = Machine::failing("old", "new");
    assert_eq!(machine.boot(None), Ended::Rebooted);
    assert!(machine.fell_back(), "{machine:?}");
    assert_eq!(machine.boot(None), Ended::Booted(41));
    assert_eq!(machine.boot(None), Ended::Booted(41));
    assert!(machine.fell_back(), "{machine:?}");
}

#[test]
fn a_failed_boot_menu_rewrite_leaves_the_fallback_to_the_next_boot() {
    for (old, new, now) in [("k", "k", 41), ("old", "new", 42)] {
        let mut machine = Machine::failing(old, new);
        machine.menu_writable = false;
        assert_eq!(machine.boot(None), Ended::Booted(now));
        assert_eq!(machine.state, on_trial(42, 0), "the trial is still used up");
        assert_eq!(machine.menu, 42);

        machine.menu_writable = true;
        assert_eq!(machine.boot_to_the_end(), 41);
        assert!(machine.fell_back(), "{machine:?}");
    }
}

#[test]
fn a_failed_default_link_update_leaves_the_fallback_to_the_next_boot() {
    for (old, new, now) in [("k", "k", 41), ("old", "new", 42)] {
        let mut machine = Machine::failing(old, new);
        machine.link_writable = false;
        assert_eq!(machine.boot(None), Ended::Booted(now));
        assert_eq!(machine.state, on_trial(42, 0), "the trial is still used up");
        assert_eq!(machine.default_link, 42);

        machine.link_writable = true;
        assert_eq!(machine.boot_to_the_end(), 41);
        assert!(machine.fell_back(), "{machine:?}");
    }
}

#[test]
fn power_loss_at_any_step_of_a_fallback_still_ends_in_the_fallback() {
    for (old, new) in [("k", "k"), ("old", "new")] {
        let steps = plan(on_trial(42, 0), 42, false, TRIES).steps.len();
        for done in 0..=steps {
            let mut machine = Machine::failing(old, new);
            assert_eq!(machine.boot(Some(done)), Ended::PowerLost);
            assert_eq!(
                machine.boot_to_the_end(),
                41,
                "power lost after {done} steps"
            );
            assert!(
                machine.fell_back(),
                "power lost after {done} steps: {machine:?}"
            );
            assert_eq!(machine.boot(None), Ended::Booted(41));
            assert!(machine.fell_back(), "{machine:?}");
        }
    }
}

#[test]
fn a_boot_of_the_last_good_generation_finishes_a_used_up_trial() {
    let mut machine = Machine::failing("old", "new");
    machine.menu = 41;
    assert_eq!(machine.boot(None), Ended::Booted(41));
    assert!(machine.fell_back(), "{machine:?}");
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

const MENU: &str = "timeout: 3\ndefault_entry: 1\n\n/Laft 43\n    comment: new\n    cmdline: laft.gen=43\n\n/Laft 42 (failed to boot)\n    cmdline: laft.gen=42\n//Laft 7\n    comment: a nested entry is not a position\n\n/Laft 41\n    cmdline: laft.gen=41\n";

#[test]
fn the_menu_position_is_verified_against_the_entry_title() {
    assert_eq!(menu_position(MENU, Some(1), 43), Some(1));
    assert_eq!(menu_position(MENU, Some(3), 41), Some(3));
}

#[test]
fn a_stale_index_is_corrected_by_looking_the_generation_up() {
    assert_eq!(menu_position(MENU, Some(1), 42), Some(2));
    assert_eq!(menu_position(MENU, Some(9), 41), Some(3));
    assert_eq!(menu_position(MENU, None, 41), Some(3));
}

#[test]
fn a_generation_without_an_entry_has_no_position() {
    assert_eq!(menu_position(MENU, Some(2), 40), None);
    assert_eq!(menu_position(MENU, Some(1), 7), None);
}

#[test]
fn a_failed_listing_leaves_the_boot_state_unpruned() {
    let state = fallen_back();
    let (kept, warning) = super::prune_listed(state.clone(), Err("EIO".into()));
    assert_eq!(kept, state);
    assert!(warning.is_some_and(|w| w.contains("EIO")));

    let (pruned, warning) = super::prune_listed(state, Ok(BTreeSet::from([41])));
    assert!(pruned.bad.is_empty() && pruned.good.contains(&41));
    assert_eq!(warning, None);
}

#[test]
fn a_menu_without_the_target_entry_cannot_be_retargeted() {
    let entries = br#"{"entries": [{"gen": 41, "index": 3}], "version": 1}"#;
    let error = retarget_menu(entries, MENU, 40).unwrap_err();
    assert!(error.contains("no entry for generation 40"), "{error}");
    assert!(retarget_menu(entries, "timeout: 3\n\n/Laft 41\n", 41).is_err());
}

#[test]
fn retargeting_the_menu_sets_the_entry_and_reports_a_stale_index() {
    let entries =
        br#"{"entries": [{"gen": 41, "index": 3}, {"gen": 42, "index": 1}], "version": 1}"#;
    let (conf, warning) = retarget_menu(entries, MENU, 41).unwrap();
    assert!(conf.starts_with("timeout: 3\ndefault_entry: 3\n"), "{conf}");
    assert_eq!(warning, None);
    let (conf, warning) = retarget_menu(entries, MENU, 42).unwrap();
    assert!(conf.contains("default_entry: 2\n"), "{conf}");
    assert!(warning.is_some_and(|w| w.contains("stale")));
}
