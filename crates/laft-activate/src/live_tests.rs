use std::collections::BTreeSet;
use std::os::unix::fs::symlink;

use laft_store::{Lock, Outcome};

use super::*;
use crate::test_support::{Env, build_generation, config_with, link};
use crate::{activate_boot, fault};

/// Generation mounts kept in memory.
#[derive(Default)]
struct FakeMounts {
    mounted: BTreeSet<u64>,
    refuse: bool,
}

impl Mounts for FakeMounts {
    fn mount(&mut self, number: u64) -> std::result::Result<(), String> {
        if self.refuse {
            return Err("no such device".into());
        }
        self.mounted.insert(number);
        Ok(())
    }

    fn unmount(&mut self, keep: &[u64]) -> std::result::Result<(), String> {
        self.mounted.retain(|n| keep.contains(n));
        Ok(())
    }
}

/// A running system on generation 1, booted from it, with generation 2
/// built next to it. Generation 2 has `/etc/motd` and version 3 of the
/// view `dev`.
fn running(env: &Env) -> (Lock, FakeMounts) {
    let lock = env.lock();
    build_generation(&lock, 1, &config_with(&[("/etc/issue", "one\n")]));
    build_generation(&lock, 2, &config_with(&[("/etc/motd", "two\n")]));
    let gen_json = env.path("laft/gens/2/gen.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&gen_json).expect("readable")).expect("JSON");
    value["views"]["dev"] = 3.into();
    fs::write(&gen_json, serde_json::to_vec(&value).expect("JSON")).expect("writable");
    fs::create_dir_all(env.path("laft/views/dev")).expect("creates the view");
    symlink("2", env.path("laft/views/dev/current")).expect("links");
    reconcile_etc(&lock, 1, &BTreeSet::new(), OnConflict::Stop).expect("reconciles");
    apply_accounts(&lock, 1).expect("applies");
    fs::create_dir_all(env.path("run/laft")).expect("creates run");
    symlink("mounts/1", env.path("run/laft/system")).expect("links");
    symlink("mounts/1", env.path("run/laft/booted")).expect("links");
    lock.set_default(1).expect("generation 1 exists");
    let mounts = FakeMounts {
        mounted: BTreeSet::from([1]),
        refuse: false,
    };
    (lock, mounts)
}

fn switch(lock: &Lock, mounts: &mut FakeMounts) -> Result<LiveActivation> {
    let activation = lock.activate("switch", 2, Some(1), LIVE_STEPS)?;
    activate_live(lock, activation, &BTreeSet::new(), mounts)
}

fn assert_on(env: &Env, lock: &Lock, mounts: &FakeMounts, generation: u64) {
    let root = lock.root();
    assert_eq!(
        link(env, "run/laft/system"),
        Some(format!("mounts/{generation}"))
    );
    assert_eq!(
        root.default_generation().expect("readable"),
        Some(generation)
    );
    assert!(lock.interrupted_activation().expect("readable").is_none());
    assert!(lock.recover().expect("recovers").is_empty());
    let mounted: Vec<u64> = mounts.mounted.iter().copied().collect();
    if generation == 2 {
        assert_eq!(mounted, [1, 2], "the booted generation stays mounted");
        assert_eq!(link(env, "etc/motd").as_deref(), Some("/etc/static/motd"));
        assert!(link(env, "etc/issue").is_none());
        assert_eq!(link(env, "laft/views/dev/current").as_deref(), Some("3"));
    } else {
        assert_eq!(mounted, [1]);
        assert_eq!(link(env, "etc/issue").as_deref(), Some("/etc/static/issue"));
        assert!(link(env, "etc/motd").is_none());
        assert_eq!(link(env, "laft/views/dev/current").as_deref(), Some("2"));
    }
}

#[test]
fn a_live_switch_runs_every_step_and_makes_the_generation_the_default() {
    let env = Env::new();
    let (lock, mut mounts) = running(&env);
    let done = switch(&lock, &mut mounts).expect("switches");
    assert!(done.active);
    assert_eq!(done.done, LIVE_STEPS);
    assert!(done.failed.is_empty(), "{:?}", done.failed);
    assert_eq!(
        done.services,
        Some(Services::Skipped {
            backend: "runit".into()
        })
    );
    assert_on(&env, &lock, &mounts, 2);
}

#[test]
fn an_interrupted_live_switch_finishes_or_rolls_back_by_how_far_it_came() {
    for step in LIVE_STEPS {
        let point = step_point(step);
        let env = Env::new();
        let (lock, mut mounts) = running(&env);
        fault::fail_at(Some(point));
        let result = switch(&lock, &mut mounts);
        fault::fail_at(None);
        assert!(
            matches!(result, Err(Error::Injected(p)) if p == point),
            "{point}: {result:?}"
        );

        let recovered = lock.recover().expect("recovers");
        assert_eq!(
            recovered[0].outcome,
            Outcome::ActivationInterrupted,
            "{point}"
        );
        let resumed = resume_live(&lock, &mut mounts)
            .expect("resumes")
            .expect("an activation was interrupted");
        if *step == "mount" {
            assert!(!resumed.active, "{point}");
            assert!(resumed.done.is_empty());
            assert_on(&env, &lock, &mounts, 1);
        } else {
            assert!(resumed.active, "{point}");
            assert!(resumed.failed.is_empty(), "{point}: {:?}", resumed.failed);
            assert_on(&env, &lock, &mounts, 2);
        }
        assert!(resume_live(&lock, &mut mounts).expect("reads").is_none());
    }
}

#[test]
fn a_switch_that_moved_the_link_but_was_not_recorded_finishes() {
    let env = Env::new();
    let (lock, mut mounts) = running(&env);
    let mut activation = lock
        .activate("switch", 2, Some(1), LIVE_STEPS)
        .expect("starts");
    mounts.mount(2).expect("mounts");
    activation.step_finished().expect("records");
    replace_link(&env.path("run/laft/system"), "mounts/2").expect("switches");
    drop(activation);
    let resumed = resume_live(&lock, &mut mounts)
        .expect("resumes")
        .expect("waits");
    assert!(resumed.active);
    assert_on(&env, &lock, &mounts, 2);
}

#[test]
fn a_generation_that_cannot_be_mounted_is_rolled_back_at_once() {
    let env = Env::new();
    let (lock, mut mounts) = running(&env);
    mounts.refuse = true;
    let error = switch(&lock, &mut mounts).expect_err("the mount fails");
    assert!(
        matches!(&error, Error::Live { step: "mount", reason } if reason == "no such device"),
        "{error}"
    );
    assert_on(&env, &lock, &mounts, 1);
}

#[test]
fn boot_abandons_a_live_switch_of_the_previous_boot() {
    let env = Env::new();
    let (lock, mut mounts) = running(&env);
    fault::fail_at(Some("live.etc"));
    let _ = switch(&lock, &mut mounts);
    fault::fail_at(None);
    let activation = activate_boot(&lock).expect("activates");
    assert_eq!(activation.generation, 1);
    assert!(lock.interrupted_activation().expect("readable").is_none());
    assert_eq!(lock.root().default_generation().expect("readable"), Some(1));
    assert!(link(&env, "etc/motd").is_none());
    assert_eq!(
        link(&env, "etc/issue").as_deref(),
        Some("/etc/static/issue")
    );
}
