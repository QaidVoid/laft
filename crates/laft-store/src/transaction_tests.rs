use crate::journal::Journal;
use crate::test_support::{Env, Pkg, apply, assert_consistent, commit, name};
use crate::{Error, Outcome, State, fault};

fn position(point: &str) -> usize {
    fault::POINTS
        .iter()
        .position(|p| *p == point)
        .expect("a listed point")
}

#[test]
fn interrupted_transactions_recover_to_a_consistent_tree() {
    for &point in fault::POINTS {
        let env = Env::new();
        let base = Pkg::new("base", "1")
            .file("usr/lib/libbase.so.1", b"base")
            .build(&env);
        assert_eq!(commit(&env, &[&base]), 1);
        let tool = Pkg::new("tool", "2")
            .file("usr/bin/tool", b"tool")
            .symlink("usr/bin/t", "tool")
            .build(&env);

        let lock = env.lock();
        fault::fail_at(Some(point));
        let result = apply(&lock, &[&base, &tool], Some("dev"));
        fault::fail_at(None);
        assert!(
            matches!(result, Err(Error::Injected(p)) if p == point),
            "{point}: {result:?}"
        );

        let recovered = lock.recover().expect("recovery succeeds");
        assert_eq!(
            recovered.len(),
            usize::from(point != "begin.counters"),
            "{point}"
        );
        assert_consistent(&env.root);

        let at = position(point);
        let built = at >= position("assembled");
        let stored = at >= position("store.stored");
        let has_tool = env.root.store().list().expect("lists").contains(&tool.dir);
        assert_eq!(has_tool, stored, "{point}: tool entry");
        let generations = if built { vec![1, 2] } else { vec![1] };
        assert_eq!(
            env.root.generations().expect("lists"),
            generations,
            "{point}"
        );
        let views = if built { vec![1] } else { vec![] };
        assert_eq!(
            env.root.view_versions(&name("dev")).expect("lists"),
            views,
            "{point}"
        );
        let default = if at >= position("default.linked") {
            2
        } else {
            1
        };
        assert_eq!(
            env.root.default_generation().expect("readable"),
            Some(default),
            "{point}"
        );

        let next =
            apply(&lock, &[&base, &tool], Some("dev")).expect("the next transaction succeeds");
        assert_eq!(next, 3, "{point}: numbers are never reused");
        assert_eq!(env.root.default_generation().expect("readable"), Some(3));
        assert_eq!(
            env.root.view_versions(&name("dev")).expect("lists").last(),
            Some(&2)
        );
        assert_consistent(&env.root);
    }
}

#[test]
fn pending_transaction_blocks_the_next_one() {
    let env = Env::new();
    let tool = Pkg::new("tool", "2")
        .file("usr/bin/tool", b"tool")
        .build(&env);
    let lock = env.lock();
    fault::fail_at(Some("assemble.started"));
    let _ = apply(&lock, &[&tool], None);
    fault::fail_at(None);
    let error = apply(&lock, &[&tool], None).expect_err("recovery has not run");
    assert!(
        matches!(
            error,
            Error::Pending {
                state: State::Assembling,
                ..
            }
        ),
        "{error}"
    );
    let recovered = lock.recover().expect("recovers");
    assert_eq!(recovered[0].outcome, Outcome::RolledBack);
    assert_eq!(apply(&lock, &[&tool], None).expect("succeeds"), 2);
}

/// Rewrites the only journal as an activation would have left it.
fn rewrite_journal(env: &Env, state: State, boot_id: Option<&str>) {
    let mut journal = Journal::all(&env.root)
        .expect("readable")
        .pop()
        .expect("a journal exists");
    journal.state = state;
    if let Some(boot_id) = boot_id {
        journal.boot_id = boot_id.into();
    }
    journal.save(&env.root).expect("writable");
}

#[test]
fn activation_states_recover_as_recorded() {
    let env = Env::new();
    let tool = Pkg::new("tool", "2")
        .file("usr/bin/tool", b"tool")
        .build(&env);
    commit(&env, &[&tool]);
    let lock = env.lock();

    fault::fail_at(Some("assembled"));
    let _ = apply(&lock, &[&tool], None);
    fault::fail_at(None);
    rewrite_journal(&env, State::Activating, None);
    let recovered = lock.recover().expect("recovers");
    assert_eq!(recovered[0].outcome, Outcome::ActivationInterrupted);
    assert!(matches!(
        apply(&lock, &[&tool], None),
        Err(Error::Pending {
            state: State::Activating,
            ..
        })
    ));

    rewrite_journal(&env, State::Activating, Some("an earlier boot"));
    let recovered = lock.recover().expect("recovers");
    assert_eq!(recovered[0].outcome, Outcome::RebootedDuringActivation);
    assert_eq!(env.root.default_generation().expect("readable"), Some(1));
    assert_consistent(&env.root);

    fault::fail_at(Some("assembled"));
    let _ = apply(&lock, &[&tool], None);
    fault::fail_at(None);
    rewrite_journal(&env, State::Activated, None);
    let recovered = lock.recover().expect("recovers");
    assert_eq!(recovered[0].outcome, Outcome::MadeDefault);
    assert_eq!(recovered[0].generation, Some(3));
    assert_eq!(env.root.default_generation().expect("readable"), Some(3));
    assert!(
        recovered[0]
            .to_string()
            .contains("generation 3 is now the default")
    );
    assert_consistent(&env.root);
}
