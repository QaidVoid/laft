use rustix::io::Errno;

use super::{Change, Observed, plan, verify};
use crate::access::Caller;

fn alice() -> Caller {
    Caller {
        uid: 1000,
        gid: 1000,
        groups: vec![10, 1000],
    }
}

fn dropped(caller: &Caller) -> Observed {
    Observed {
        uids: [caller.uid; 3],
        gids: [caller.gid; 3],
        groups: caller.groups.clone(),
        setuid_root: Err(Errno::PERM),
        setgid_root: Err(Errno::PERM),
        no_capabilities: true,
    }
}

#[test]
fn groups_are_dropped_before_the_gid_and_the_gid_before_the_uid() {
    assert_eq!(
        plan(&alice()),
        [
            Change::Groups(vec![10, 1000]),
            Change::Gid(1000),
            Change::Uid(1000),
        ]
    );
}

#[test]
fn a_complete_drop_passes() {
    assert!(verify(&alice(), &dropped(&alice())));
}

#[test]
fn the_kernel_may_reorder_the_groups() {
    let mut observed = dropped(&alice());
    observed.groups = vec![1000, 10, 10];
    assert!(verify(&alice(), &observed));
}

#[test]
fn a_root_id_left_in_any_slot_fails() {
    for slot in 0..3 {
        let mut observed = dropped(&alice());
        observed.uids[slot] = 0;
        assert!(!verify(&alice(), &observed), "uid slot {slot}");
        let mut observed = dropped(&alice());
        observed.gids[slot] = 0;
        assert!(!verify(&alice(), &observed), "gid slot {slot}");
    }
}

#[test]
fn a_leftover_supplementary_group_fails() {
    let mut observed = dropped(&alice());
    observed.groups.push(0);
    assert!(!verify(&alice(), &observed));
    observed.groups = vec![];
    assert!(!verify(&alice(), &observed));
}

#[test]
fn regaining_root_fails() {
    let mut observed = dropped(&alice());
    observed.setuid_root = Ok(());
    assert!(!verify(&alice(), &observed));

    let mut observed = dropped(&alice());
    observed.setgid_root = Ok(());
    assert!(!verify(&alice(), &observed));
}

#[test]
fn a_refusal_other_than_eperm_fails() {
    let mut observed = dropped(&alice());
    observed.setuid_root = Err(Errno::INVAL);
    assert!(!verify(&alice(), &observed));
}

#[test]
fn leftover_capabilities_fail() {
    let mut observed = dropped(&alice());
    observed.no_capabilities = false;
    assert!(!verify(&alice(), &observed));
}

#[test]
fn a_caller_in_group_root_may_keep_gid_zero() {
    let caller = Caller {
        uid: 1000,
        gid: 0,
        groups: vec![],
    };
    let mut observed = dropped(&caller);
    observed.setgid_root = Ok(());
    assert!(verify(&caller, &observed));
}

#[test]
fn root_stays_root_with_its_capabilities() {
    let root = Caller {
        uid: 0,
        gid: 0,
        groups: vec![0],
    };
    let mut observed = dropped(&root);
    observed.setuid_root = Ok(());
    observed.setgid_root = Ok(());
    observed.no_capabilities = false;
    assert!(verify(&root, &observed));
    observed.uids = [0, 0, 1000];
    assert!(!verify(&root, &observed));
}

#[test]
fn a_refused_change_names_the_step_and_keeps_the_errno() {
    if rustix::process::geteuid().is_root() {
        println!("skipping: running as root, so the drop would succeed");
        return;
    }
    let caller = Caller {
        uid: rustix::process::getuid().as_raw(),
        gid: rustix::process::getgid().as_raw(),
        groups: vec![rustix::process::getgid().as_raw()],
    };
    let failure = super::drop_to(&caller).unwrap_err();
    assert_eq!(failure.errno, Some(Errno::PERM));
    assert!(
        failure.message.contains("supplementary groups"),
        "{}",
        failure.message
    );
}
