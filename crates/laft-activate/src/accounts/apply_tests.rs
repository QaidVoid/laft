use std::fs;
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::test_support::{Env, declare_generation, has_entry, mode, users_with};
use crate::{User, Users};

const BOB_PASSWD: &str = "bob:x:1500:1500:Bob:/home/bob:/usr/bin/zsh\n";
const BOB_GROUP: &str = "bob:x:1500:\nwheel:x:10:bob,alice\n";
const BOB_SHADOW: &str = "bob:$6$salt$bobhash:19000:0:99999:7:::\n";
const BOB_GSHADOW: &str = "bob:!::\nwheel:!::bob,alice\n";

/// Writes databases holding `bob`, an account created outside Laft, and
/// `alice` listed in `wheel`.
fn with_bob(env: &Env) {
    env.write("etc/passwd", BOB_PASSWD);
    env.write("etc/group", BOB_GROUP);
    env.write("etc/shadow", BOB_SHADOW);
    env.write("etc/gshadow", BOB_GSHADOW);
}

fn apply(env: &Env, number: u64, users: &Users) -> AccountsReport {
    let lock = env.lock();
    declare_generation(&lock, number, users);
    apply_accounts(&lock, number).expect("accounts apply")
}

fn alice() -> User {
    User {
        password_hash: Some("$6$salt$alicehash".into()),
        ..User::default()
    }
}

fn databases(env: &Env) -> String {
    ["passwd", "group", "shadow", "gshadow"]
        .iter()
        .map(|name| format!("=== {name}\n{}", env.read(&format!("etc/{name}"))))
        .collect()
}

#[test]
fn mutable_accounts_keep_accounts_created_outside_laft() {
    let env = Env::new();
    with_bob(&env);
    let report = apply(&env, 1, &users_with(&[("alice", alice())]));
    assert!(report.removed_users.is_empty());
    assert!(report.warnings.is_empty());
    insta::assert_snapshot!(databases(&env));
}

#[test]
fn immutable_accounts_remove_undeclared_accounts_with_a_warning() {
    let env = Env::new();
    with_bob(&env);
    let mut users = users_with(&[("alice", alice())]);
    users.mutable = false;
    let report = apply(&env, 1, &users);
    assert_eq!(report.removed_users, ["bob"]);
    assert_eq!(report.removed_groups, ["bob"]);
    assert_eq!(
        report.warnings,
        [
            Warning::RemovedAccount {
                kind: "user",
                name: "bob".into()
            },
            Warning::RemovedAccount {
                kind: "group",
                name: "bob".into()
            },
        ]
    );
    for name in ["passwd", "group", "shadow", "gshadow"] {
        assert!(
            !has_entry(&env.read(&format!("etc/{name}")), "bob"),
            "{name}"
        );
    }
    assert_eq!(
        env.read("etc/gshadow")
            .lines()
            .find(|l| l.starts_with("wheel:")),
        Some("wheel:!::")
    );
}

#[test]
fn rollback_removes_accounts_that_only_the_newer_generation_declared() {
    let env = Env::new();
    with_bob(&env);
    let older = users_with(&[("alice", alice())]);
    let carol = User {
        groups: ["wheel".to_owned()].into(),
        ..User::default()
    };
    let newer = users_with(&[("alice", alice()), ("carol", carol)]);
    let lock = env.lock();
    declare_generation(&lock, 1, &older);
    let declared = declare_generation(&lock, 2, &newer);
    let carol_uid = declared.users["carol"].uid;

    apply_accounts(&lock, 1).expect("applies");
    apply_accounts(&lock, 2).expect("applies");
    assert!(has_entry(&env.read("etc/shadow"), "carol"));
    assert!(env.path("home/carol").is_dir());

    let report = apply_accounts(&lock, 1).expect("rollback applies");
    assert_eq!(report.removed_users, ["carol"]);
    assert_eq!(report.removed_groups, ["carol"]);
    assert!(
        report.warnings.is_empty(),
        "removing managed accounts is expected"
    );
    for name in ["passwd", "group", "shadow", "gshadow"] {
        let text = env.read(&format!("etc/{name}"));
        assert!(!text.contains("carol"), "{name} still names carol:\n{text}");
        assert!(has_entry(&text, "bob"), "{name} lost bob");
    }
    assert!(env.path("home/carol").is_dir(), "homes are never removed");
    let allocations = Allocations::load(lock.root()).expect("readable");
    assert_eq!(allocations.users["carol"], carol_uid);
    assert!(!allocations.managed_users.contains("carol"));

    let again = declare_generation(&lock, 3, &newer);
    assert_eq!(again.users["carol"].uid, carol_uid);
}

#[test]
fn declared_groups_enforce_the_memberships_of_managed_users() {
    let env = Env::new();
    with_bob(&env);
    apply(&env, 1, &users_with(&[("alice", alice())]));
    let wheel = env
        .read("etc/group")
        .lines()
        .find(|l| l.starts_with("wheel:"))
        .map(str::to_owned);
    assert_eq!(wheel.as_deref(), Some("wheel:x:10:bob"));
}

#[test]
fn an_undeclared_account_with_a_declared_id_is_a_collision() {
    let env = Env::new();
    let lock = env.lock();
    declare_generation(
        &lock,
        1,
        &users_with(&[(
            "alice",
            User {
                uid: Some(1500),
                group: Some("users".into()),
                ..User::default()
            },
        )]),
    );
    // Created outside Laft after the generation was declared.
    with_bob(&env);
    let err = apply_accounts(&lock, 1).expect_err("collides");
    assert_eq!(err.code(), Some("C0411"));
    assert_eq!(
        err.to_string(),
        "declared user \"alice\" and undeclared user \"bob\" both have id 1500"
    );
    assert_eq!(env.read("etc/passwd"), BOB_PASSWD);
    assert_eq!(env.read("etc/shadow"), BOB_SHADOW);
    assert!(!env.path("etc/passwd-").exists());
}

#[test]
fn shadow_entries_follow_the_password_source() {
    let env = Env::new();
    with_bob(&env);
    env.write("etc/laft/secrets/dave", "$y$j9T$davehash\n");
    fs::set_permissions(
        env.path("etc/laft/secrets/dave"),
        fs::Permissions::from_mode(0o600),
    )
    .expect("can chmod");
    let users = users_with(&[
        ("alice", alice()),
        ("bob", User::default()),
        (
            "dave",
            User {
                password_file: Some("/etc/laft/secrets/dave".into()),
                ..User::default()
            },
        ),
        ("erin", User::default()),
    ]);
    apply(&env, 1, &users);
    let shadow = env.read("etc/shadow");
    let line = |name: &str| {
        shadow
            .lines()
            .find(|l| l.starts_with(&format!("{name}:")))
            .map(str::to_owned)
    };
    assert_eq!(
        line("bob").as_deref(),
        Some("bob:$6$salt$bobhash:19000:0:99999:7:::"),
        "an existing hash and its aging fields are kept"
    );
    assert_eq!(
        line("alice").as_deref(),
        Some("alice:$6$salt$alicehash:::::::")
    );
    assert_eq!(line("dave").as_deref(), Some("dave:$y$j9T$davehash:::::::"));
    assert_eq!(
        line("erin").as_deref(),
        Some("erin:!:::::::"),
        "new accounts are locked"
    );

    let replaced = users_with(&[(
        "bob",
        User {
            password_hash: Some("!".into()),
            ..User::default()
        },
    )]);
    apply(&env, 2, &replaced);
    assert!(
        env.read("etc/shadow")
            .contains("bob:!:19000:0:99999:7:::\n")
    );
}

#[test]
fn databases_get_their_modes_and_keep_a_backup() {
    let env = Env::new();
    with_bob(&env);
    apply(&env, 1, &users_with(&[("alice", alice())]));
    for (name, expected) in [
        ("passwd", 0o644),
        ("group", 0o644),
        ("shadow", 0o640),
        ("gshadow", 0o640),
    ] {
        assert_eq!(mode(&env.path(&format!("etc/{name}"))), expected, "{name}");
        assert_eq!(
            mode(&env.path(&format!("etc/{name}-"))),
            expected,
            "{name}-"
        );
        assert!(!env.path(&format!("etc/{name}+")).exists());
    }
    assert_eq!(env.read("etc/passwd-"), BOB_PASSWD);
    assert_eq!(env.read("etc/shadow-"), BOB_SHADOW);
    let copies = CopiesRecord::load(&env.root).expect("readable");
    assert_eq!(
        copies.files["shadow"],
        Blake3Hash::of(env.read("etc/shadow").as_bytes())
    );
    assert!(copies.pending.is_empty());
}

#[test]
fn bad_password_files_stop_before_anything_is_written() {
    let cases: [(&str, Option<&str>, u32, &str); 3] = [
        ("missing", None, 0o600, "C0412"),
        ("open", Some("$6$a$b\n"), 0o640, "C0412"),
        ("garbage", Some("hunter2\n"), 0o600, "C0413"),
    ];
    for (name, content, file_mode, code) in cases {
        let env = Env::new();
        with_bob(&env);
        let path = format!("etc/laft/secrets/{name}");
        if let Some(content) = content {
            env.write(&path, content);
            fs::set_permissions(env.path(&path), fs::Permissions::from_mode(file_mode))
                .expect("can chmod");
        }
        let lock = env.lock();
        let user = User {
            password_file: Some(format!("/{path}")),
            ..User::default()
        };
        declare_generation(&lock, 1, &users_with(&[("alice", user)]));
        let err = apply_accounts(&lock, 1).expect_err("rejected");
        assert_eq!(err.code(), Some(code), "{name}: {err}");
        assert_eq!(env.read("etc/passwd"), BOB_PASSWD, "{name}");
    }
}

#[test]
fn malformed_databases_stop_before_anything_is_written() {
    let env = Env::new();
    let lock = env.lock();
    declare_generation(&lock, 1, &users_with(&[]));
    for passwd in [
        "root:x:zero:0::/root:/bin/sh\n",
        "root:x:+0:0::/root:/bin/sh\n",
    ] {
        env.write("etc/passwd", passwd);
        let err = apply_accounts(&lock, 1).expect_err("malformed");
        assert_eq!(err.code(), Some("C0410"));
        assert!(err.to_string().ends_with("passwd:1: invalid id"), "{err}");
    }
    assert!(!env.path("etc/shadow").exists());
}

#[test]
fn missing_homes_are_created_private_and_existing_ones_are_untouched() {
    let env = Env::new();
    env.write("home/bob/notes", "keep");
    let report = apply(
        &env,
        1,
        &users_with(&[("alice", User::default()), ("bob", User::default())]),
    );
    assert_eq!(report.created_homes, ["/home/alice"]);
    assert_eq!(mode(&env.path("home/alice")), 0o700);
    assert_eq!(env.read("home/bob/notes"), "keep");
    assert!(!env.path("var/empty").exists());
}

#[test]
fn new_entries_are_appended_in_id_order() {
    let env = Env::new();
    with_bob(&env);
    let report = apply(
        &env,
        1,
        &users_with(&[
            (
                "zed",
                User {
                    uid: Some(1001),
                    ..User::default()
                },
            ),
            (
                "amy",
                User {
                    uid: Some(1002),
                    ..User::default()
                },
            ),
        ]),
    );
    assert!(report.removed_users.is_empty());
    let lines: Vec<String> = env
        .read("etc/passwd")
        .lines()
        .map(|l| l.split(':').next().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(lines, ["bob", "root", "sshd", "zed", "amy", "nobody"]);
}

#[test]
fn a_crash_after_the_databases_are_written_still_marks_their_accounts_managed() {
    let env = Env::new();
    with_bob(&env);
    let older = users_with(&[("alice", alice())]);
    let newer = users_with(&[("alice", alice()), ("carol", User::default())]);
    let lock = env.lock();
    declare_generation(&lock, 1, &older);
    declare_generation(&lock, 2, &newer);
    apply_accounts(&lock, 1).expect("applies");

    crate::fault::fail_at(Some("accounts.databases"));
    let interrupted = apply_accounts(&lock, 2);
    crate::fault::fail_at(None);
    assert!(matches!(interrupted, Err(Error::Injected(_))));
    assert!(has_entry(&env.read("etc/passwd"), "carol"));

    let report = apply_accounts(&lock, 1).expect("rollback applies");
    assert_eq!(report.removed_users, ["carol"]);
    assert!(!has_entry(&env.read("etc/passwd"), "carol"));
}

#[test]
fn a_password_file_that_is_not_a_regular_file_is_refused() {
    let env = Env::new();
    with_bob(&env);
    let secrets = env.path("etc/laft/secrets");
    fs::create_dir_all(secrets.join("dir")).expect("can create");
    fs::set_permissions(secrets.join("dir"), fs::Permissions::from_mode(0o700)).expect("can chmod");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        secrets.join("fifo"),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .expect("can create a FIFO");
    for name in ["dir", "fifo"] {
        let lock = env.lock();
        let user = User {
            password_file: Some(format!("/etc/laft/secrets/{name}")),
            ..User::default()
        };
        declare_generation(&lock, 1, &users_with(&[("alice", user)]));
        let err = apply_accounts(&lock, 1).expect_err("rejected");
        assert_eq!(err.code(), Some("C0412"), "{name}: {err}");
        fs::remove_dir_all(crate::state::gen_dir(lock.root(), 1)).expect("can remove");
    }
}

#[test]
fn a_declared_group_keeps_its_administrators_and_password_when_mutable() {
    let env = Env::new();
    env.write("etc/group", "wheel:x:10:alice\n");
    env.write("etc/gshadow", "wheel:$6$salt$grouphash:alice:alice\n");
    apply(&env, 1, &users_with(&[("alice", alice())]));
    let wheel = env
        .read("etc/gshadow")
        .lines()
        .find(|l| l.starts_with("wheel:"))
        .map(str::to_owned);
    assert_eq!(wheel.as_deref(), Some("wheel:$6$salt$grouphash:alice:"));
}

#[test]
fn a_kept_user_with_its_hash_in_passwd_is_not_locked() {
    let env = Env::new();
    env.write(
        "etc/passwd",
        "dave:$6$salt$davehash:1600:1600::/home/dave:/bin/sh\n",
    );
    env.write("etc/group", "dave:x:1600:\n");
    apply(&env, 1, &users_with(&[]));
    let dave = env
        .read("etc/shadow")
        .lines()
        .find(|l| l.starts_with("dave:"))
        .map(str::to_owned);
    assert_eq!(dave.as_deref(), Some("dave:$6$salt$davehash:::::::"));
}

#[test]
fn a_tampered_accounts_record_is_refused() {
    let env = Env::new();
    let lock = env.lock();
    declare_generation(&lock, 1, &users_with(&[("alice", alice())]));
    let path = crate::state::gen_dir(lock.root(), 1).join("accounts.json");
    for (from, to) in [
        ("\"/home/alice\"", "\"/home/../etc\""),
        ("$6$salt$alicehash", "$6$salt:alicehash"),
    ] {
        let text = fs::read_to_string(&path).expect("readable");
        assert!(text.contains(from), "{text}");
        fs::write(&path, text.replace(from, to)).expect("writable");
        let err = apply_accounts(&lock, 1).expect_err("rejected");
        assert!(err.to_string().contains("accounts.json"), "{err}");
        fs::write(&path, text).expect("writable");
    }
    assert!(!env.path("etc/passwd").exists());
}

#[test]
fn renaming_an_account_keeps_its_uid() {
    let env = Env::new();
    let lock = env.lock();
    let first = declare_generation(&lock, 1, &users_with(&[("alice", alice())]));
    apply_accounts(&lock, 1).expect("applies");
    let uid = first.users["alice"].uid;

    let bob = User {
        uid: Some(uid),
        ..alice()
    };
    let renamed = declare_generation(&lock, 2, &users_with(&[("bob", bob)]));
    assert_eq!(renamed.users["bob"].uid, uid);
    apply_accounts(&lock, 2).expect("applies");
    let passwd = env.read("etc/passwd");
    assert!(!has_entry(&passwd, "alice"), "{passwd}");
    assert!(passwd.contains(&format!("bob:x:{uid}:")), "{passwd}");
}

#[test]
fn an_undeclared_group_with_a_declared_gid_is_a_collision() {
    let env = Env::new();
    let lock = env.lock();
    let mut users = users_with(&[]);
    users.groups.insert(
        "media".into(),
        crate::Group {
            gid: Some(1500),
            ..crate::Group::default()
        },
    );
    declare_generation(&lock, 1, &users);
    with_bob(&env);
    let err = apply_accounts(&lock, 1).expect_err("collides");
    assert!(
        matches!(
            &err,
            Error::IdCollision { kind: "group", declared, existing, id: 1500 }
                if declared == "media" && existing == "bob"
        ),
        "{err}"
    );
    assert_eq!(env.read("etc/group"), BOB_GROUP);
}

#[test]
fn short_lines_from_other_tools_read_missing_fields_as_empty() {
    let env = Env::new();
    with_bob(&env);
    env.write("etc/shadow", "bob:$6$salt$bobhash\n");
    let report = apply(&env, 1, &users_with(&[]));
    assert_eq!(
        report.warnings,
        [Warning::ShortLine {
            path: env.path("etc/shadow"),
            line: 1
        }]
    );
    let shadow = env.read("etc/shadow");
    assert!(
        shadow.lines().any(|l| l == "bob:$6$salt$bobhash:::::::"),
        "{shadow}"
    );
}

#[test]
fn lines_without_an_account_name_or_with_odd_ids_are_malformed() {
    for (passwd, reason) in [
        (
            ":x:1000:1000::/home/x:/bin/sh\n",
            "passwd:1: the line names no account",
        ),
        (
            "bob:x:01000:1000::/home/bob:/bin/sh\n",
            "passwd:1: invalid id",
        ),
        (
            "bob:x:+1000:1000::/home/bob:/bin/sh\n",
            "passwd:1: invalid id",
        ),
        ("bob:x:1000\n", "passwd:1: invalid id"),
    ] {
        let env = Env::new();
        let lock = env.lock();
        declare_generation(&lock, 1, &users_with(&[]));
        env.write("etc/passwd", passwd);
        let err = apply_accounts(&lock, 1).expect_err(passwd);
        assert_eq!(err.code(), Some("C0410"), "{passwd}");
        assert!(err.to_string().ends_with(reason), "{err}");
    }
    let env = Env::new();
    let lock = env.lock();
    declare_generation(&lock, 1, &users_with(&[]));
    env.write("etc/passwd", "root:x:0:0::/root:/bin/sh\n");
    apply_accounts(&lock, 1).expect("0 is a valid id");
}
