use super::*;
use crate::test_support::{Env, base_users, users_with};
use crate::{Group, User};

fn resolve_fresh(users: &Users) -> Result<Declared, Vec<ConfigError>> {
    resolve(users, &mut Allocations::default(), &Databases::default())
}

#[test]
fn users_without_ids_get_the_lowest_free_ids_in_name_order() {
    let users = users_with(&[("bob", User::default()), ("alice", User::default())]);
    let declared = resolve_fresh(&users).expect("valid");
    assert_eq!(declared.users["alice"].uid, 1000);
    assert_eq!(declared.users["bob"].uid, 1001);
    assert_eq!(declared.users["sshd"].uid, 31);
    assert_eq!(declared.users["nobody"].uid, 65534);
    assert_eq!(declared.users["nobody"].gid, 65534);
}

#[test]
fn ids_stay_stable_across_generations() {
    let env = Env::new();
    let lock = env.lock();
    let first = declare(
        &lock,
        &users_with(&[("alice", User::default()), ("carol", User::default())]),
    )
    .expect("valid");
    assert_eq!(first.users["carol"].uid, 1001);

    // A user sorting before both appears, and carol is removed for a while.
    let second = declare(
        &lock,
        &users_with(&[("aaron", User::default()), ("alice", User::default())]),
    )
    .expect("valid");
    assert_eq!(second.users["alice"].uid, 1000);
    assert_eq!(
        second.users["aaron"].uid, 1002,
        "1001 stays reserved for carol"
    );

    let third = declare(&lock, &users_with(&[("carol", User::default())])).expect("valid");
    assert_eq!(third.users["carol"].uid, 1001);
    assert_eq!(third.groups["carol"].gid, 1001);

    let recorded = Allocations::load(lock.root()).expect("readable");
    assert_eq!(recorded.users["alice"], 1000);
    assert_eq!(recorded.users["aaron"], 1002);
}

#[test]
fn private_groups_take_the_uid_when_it_is_free() {
    let mut users = users_with(&[("alice", User::default()), ("bob", User::default())]);
    users.groups.insert(
        "media".into(),
        Group {
            gid: Some(1001),
            ..Group::default()
        },
    );
    let declared = resolve_fresh(&users).expect("valid");
    assert_eq!(declared.groups["alice"].gid, 1000);
    assert_eq!(declared.users["alice"].gid, 1000);
    assert_eq!(declared.users["bob"].uid, 1001);
    assert_ne!(declared.groups["bob"].gid, 1001, "media holds 1001");
    assert_eq!(declared.users["bob"].gid, declared.groups["bob"].gid);
}

#[test]
fn ids_in_the_current_databases_are_not_allocated() {
    let env = Env::new();
    env.write("etc/passwd", "bob:x:1000:1000::/home/bob:/usr/bin/bash\n");
    env.write("etc/group", "bob:x:1000:\n");
    let declared = declare(&env.lock(), &users_with(&[("alice", User::default())])).expect("valid");
    assert_eq!(declared.users["alice"].uid, 1001);
    assert_eq!(declared.groups["alice"].gid, 1001);
}

#[test]
fn a_declared_account_keeps_the_id_it_already_has() {
    let env = Env::new();
    env.write("etc/passwd", "bob:x:1005:1005::/home/bob:/usr/bin/bash\n");
    env.write("etc/group", "bob:x:1005:\n");
    let declared = declare(&env.lock(), &users_with(&[("bob", User::default())])).expect("valid");
    assert_eq!(declared.users["bob"].uid, 1005);
    assert_eq!(declared.groups["bob"].gid, 1005);
}

#[test]
fn system_accounts_allocate_from_the_system_range_with_system_defaults() {
    let users = users_with(&[(
        "postgres",
        User {
            system: true,
            ..User::default()
        },
    )]);
    let declared = resolve_fresh(&users).expect("valid");
    let postgres = &declared.users["postgres"];
    assert_eq!(postgres.uid, 101);
    assert_eq!(declared.groups["postgres"].gid, 101);
    assert_eq!(postgres.home, "/var/empty");
    assert_eq!(postgres.shell, "/usr/bin/nologin");
    assert!(!postgres.create_home);
    let alice = resolve_fresh(&users_with(&[("alice", User::default())])).expect("valid");
    assert_eq!(alice.users["alice"].home, "/home/alice");
    assert_eq!(alice.users["alice"].shell, "/usr/bin/bash");
    assert!(alice.users["alice"].create_home);
}

#[test]
fn a_regular_user_cannot_take_a_system_group_as_primary_group() {
    let users = users_with(&[("audio", User::default())]);
    let errors = resolve_fresh(&users).expect_err("rejected");
    assert_eq!(
        errors,
        vec![ConfigError::SystemPrimaryGroup("audio".into())]
    );
    assert_eq!(errors[0].code(), Some("C0415"));
    assert_eq!(
        errors[0].to_string(),
        "user \"audio\" would take the system group \"audio\" as its primary group. Set group explicitly."
    );

    let explicit = users_with(&[(
        "audio",
        User {
            group: Some("users".into()),
            ..User::default()
        },
    )]);
    let declared = resolve_fresh(&explicit).expect("an explicit group is fine");
    assert_eq!(declared.users["audio"].gid, 100);
}

#[test]
fn memberships_combine_user_groups_and_group_members() {
    let mut users = users_with(&[
        (
            "alice",
            User {
                groups: ["wheel".to_owned(), "bob".to_owned()].into(),
                ..User::default()
            },
        ),
        ("bob", User::default()),
    ]);
    users
        .groups
        .get_mut("audio")
        .expect("declared")
        .members
        .insert("bob".into());
    let declared = resolve_fresh(&users).expect("valid");
    assert_eq!(
        declared.groups["wheel"].members,
        ["alice".to_owned()].into()
    );
    assert_eq!(declared.groups["audio"].members, ["bob".to_owned()].into());
    assert_eq!(
        declared.groups["bob"].members,
        ["alice".to_owned()].into(),
        "an implicit group can be referenced"
    );
}

#[test]
fn reserved_and_registry_ids_are_rejected() {
    let mut users = users_with(&[
        (
            "alice",
            User {
                uid: Some(50),
                ..User::default()
            },
        ),
        (
            "bob",
            User {
                uid: Some(2000),
                ..User::default()
            },
        ),
        (
            "carol",
            User {
                uid: Some(2000),
                ..User::default()
            },
        ),
    ]);
    users.accounts.get_mut("sshd").expect("declared").uid = Some(77);
    let errors = resolve_fresh(&users).expect_err("rejected");
    let codes: Vec<_> = errors.iter().filter_map(ConfigError::code).collect();
    assert_eq!(codes, ["C0407", "C0414", "C0408"], "{errors:?}");
}

#[test]
fn invalid_names_fields_and_references_are_all_reported() {
    let mut users = base_users();
    users.accounts.insert(
        "Bad".into(),
        User {
            password_hash: Some("$6$x$y".into()),
            password_file: Some("/etc/laft/secrets/bad".into()),
            description: "a:b".into(),
            groups: ["missing".to_owned()].into(),
            ..User::default()
        },
    );
    let errors = resolve_fresh(&users).expect_err("rejected");
    let codes: Vec<_> = errors.iter().map(ConfigError::code).collect();
    assert_eq!(
        codes,
        [Some("C0405"), Some("C0405"), Some("C0406"), None, None],
        "{errors:?}"
    );
}
