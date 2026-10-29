use crate::journal::Journal;
use crate::test_support::{Env, Pkg, assert_consistent, commit, gen_input, plan};
use crate::{Error, Outcome, State, Transaction, fault};

const STEPS: &[&str] = &["mount", "switch", "views"];

/// Builds generation 2 on top of a committed generation 1 and starts its
/// activation, with a failure injected at `point`.
fn activate_new(env: &Env, point: Option<&'static str>) -> crate::Result<u64> {
    let tool = Pkg::new("tool", "1")
        .file("usr/bin/tool", b"tool")
        .build(env);
    let lock = env.lock();
    fault::fail_at(point);
    let result = (|| {
        let mut tx = Transaction::begin(&lock, &plan(&env.root, &[]))?;
        tx.add(&tool.path, &tool.id)?;
        tx.assemble_generation(&gen_input(&[&tool], &[]))?;
        let mut activation = tx.activate(STEPS)?;
        for _ in STEPS {
            activation.step_finished()?;
        }
        activation.finish()
    })();
    fault::fail_at(None);
    result
}

fn base(env: &Env) {
    let base = Pkg::new("base", "1")
        .file("usr/lib/libbase.so.1", b"b")
        .build(env);
    assert_eq!(commit(env, &[&base]), 1);
}

#[test]
fn a_finished_activation_makes_its_generation_the_default() {
    let env = Env::new();
    base(&env);
    assert_eq!(activate_new(&env, None).expect("activates"), 2);
    assert_eq!(env.root.default_generation().expect("readable"), Some(2));
    assert_consistent(&env.root);
}

#[test]
fn an_interrupted_activation_resumes_where_it_stopped() {
    for &point in fault::ACTIVATION_POINTS {
        let env = Env::new();
        base(&env);
        let result = activate_new(&env, Some(point));
        assert!(
            matches!(result, Err(Error::Injected(p)) if p == point),
            "{point}: {result:?}"
        );
        let lock = env.lock();
        let recovered = lock.recover().expect("recovers");
        let activated = matches!(point, "activated" | "default.linked" | "complete");
        if activated {
            assert_eq!(recovered[0].outcome, Outcome::MadeDefault, "{point}");
            assert!(lock.interrupted_activation().expect("readable").is_none());
            assert_eq!(env.root.default_generation().expect("readable"), Some(2));
            assert_consistent(&env.root);
            continue;
        }
        assert_eq!(
            recovered[0].outcome,
            Outcome::ActivationInterrupted,
            "{point}"
        );
        let mut activation = lock
            .interrupted_activation()
            .expect("readable")
            .expect("the activation waits");
        assert_eq!(activation.generation(), 2);
        assert_eq!(activation.previous(), Some(1));
        assert_eq!(activation.steps(), STEPS);
        let finished = usize::from(point == "activate.step");
        assert_eq!(activation.finished(), finished, "{point}");
        while activation.finished() < STEPS.len() {
            activation.step_finished().expect("records");
        }
        assert!(activation.step_finished().is_err(), "no step is left");
        activation.finish().expect("finishes");
        assert_eq!(env.root.default_generation().expect("readable"), Some(2));
        assert_consistent(&env.root);
    }
}

#[test]
fn an_abandoned_activation_leaves_the_default_alone() {
    let env = Env::new();
    base(&env);
    let _ = activate_new(&env, Some("activate.step"));
    let lock = env.lock();
    lock.recover().expect("recovers");
    let activation = lock
        .interrupted_activation()
        .expect("readable")
        .expect("waits");
    activation.abandon().expect("abandons");
    assert_eq!(env.root.default_generation().expect("readable"), Some(1));
    assert_eq!(env.root.generations().expect("lists"), [1, 2]);
    assert!(lock.recover().expect("recovers").is_empty());
    assert_consistent(&env.root);
}

#[test]
fn an_activation_from_an_earlier_boot_is_not_resumed() {
    let env = Env::new();
    base(&env);
    let _ = activate_new(&env, Some("activate.step"));
    let mut journal = Journal::all(&env.root)
        .expect("readable")
        .pop()
        .expect("one");
    journal.boot_id = "an earlier boot".into();
    journal.save(&env.root).expect("writable");
    let lock = env.lock();
    assert!(lock.interrupted_activation().expect("readable").is_none());
    let recovered = lock.recover().expect("recovers");
    assert_eq!(recovered[0].outcome, Outcome::RebootedDuringActivation);
    assert_eq!(env.root.default_generation().expect("readable"), Some(1));
    assert_consistent(&env.root);
}

#[test]
fn an_existing_generation_is_activated_step_by_step() {
    let env = Env::new();
    let base = Pkg::new("base", "1")
        .file("usr/lib/libbase.so.1", b"b")
        .build(&env);
    assert_eq!(commit(&env, &[&base]), 1);
    assert_eq!(commit(&env, &[&base]), 2);
    let lock = env.lock();
    lock.set_default(1).expect("generation 1 exists");
    assert!(
        lock.activate("switch", 7, Some(1), STEPS).is_err(),
        "7 does not exist"
    );
    assert!(
        lock.activate("switch", 2, Some(1), &[]).is_err(),
        "no steps"
    );

    let mut activation = lock.activate("switch", 2, Some(1), STEPS).expect("starts");
    activation.step_finished().expect("records");
    drop(activation);
    assert!(matches!(
        lock.activate("switch", 2, Some(1), STEPS),
        Err(Error::Pending {
            state: State::Activating,
            ..
        })
    ));
    let resumed = lock
        .interrupted_activation()
        .expect("readable")
        .expect("waits");
    assert_eq!(resumed.finished(), 1);
    assert_eq!(resumed.generation(), 2);
    let mut resumed = resumed;
    resumed.step_finished().expect("records");
    resumed.step_finished().expect("records");
    resumed.finish().expect("finishes");
    assert_eq!(env.root.default_generation().expect("readable"), Some(2));
    assert_consistent(&env.root);
}
