use std::os::unix::fs::symlink;

use super::*;
use crate::test_support::{Env, build_generation, config_with, has_entry, link, users_with};
use crate::{ConflictKind, User, Warning};

fn boot_into(env: &Env, generation: u64) {
    let run = env.path("run/laft");
    fs::create_dir_all(&run).expect("creates run");
    let _ = fs::remove_file(run.join("booted"));
    symlink(format!("mounts/{generation}"), run.join("booted")).expect("links");
}

#[test]
fn boot_makes_etc_and_accounts_match_the_booted_generation() {
    let env = Env::new();
    let lock = env.lock();
    let mut older = config_with(&[("/etc/issue", "Laft\n")]);
    older.users = users_with(&[("alice", User::default())]);
    let mut newer = config_with(&[("/etc/motd", "hello\n")]);
    newer.users = users_with(&[("alice", User::default()), ("carol", User::default())]);
    build_generation(&lock, 1, &older);
    build_generation(&lock, 2, &newer);
    for generation in [1, 2] {
        reconcile_etc(&lock, generation, &BTreeSet::new(), OnConflict::Stop).expect("reconciles");
        apply_accounts(&lock, generation).expect("applies");
    }
    env.write("etc/issue", "my own issue\n");

    boot_into(&env, 1);
    let activation = activate_boot(&lock).expect("activates");
    assert_eq!(activation.generation, 1);
    assert!(activation.recoveries.is_empty());
    assert_eq!(activation.etc.unlinked, ["motd"]);
    assert!(link(&env, "etc/motd").is_none());
    assert!(matches!(
        activation.etc.warnings.as_slice(),
        [Warning::Skipped(c)] if c.path == "issue" && c.kind == ConflictKind::Unmanaged
    ));
    assert_eq!(env.read("etc/issue"), "my own issue\n");
    assert_eq!(activation.accounts.removed_users, ["carol"]);
    assert!(!has_entry(&env.read("etc/passwd"), "carol"));
    assert!(has_entry(&env.read("etc/passwd"), "alice"));
}

#[test]
fn boot_without_a_booted_generation_fails() {
    let env = Env::new();
    let lock = env.lock();
    assert!(matches!(activate_boot(&lock), Err(Error::Io { .. })));
    fs::create_dir_all(env.path("run/laft")).expect("creates run");
    symlink("mounts/current", env.path("run/laft/booted")).expect("links");
    assert!(matches!(activate_boot(&lock), Err(Error::Invalid(_))));
}
