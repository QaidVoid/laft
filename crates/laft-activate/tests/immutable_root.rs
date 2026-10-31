//! Immutable accounts retain a declared root without keeping other accounts.

use std::collections::BTreeMap;
use std::fs;

use laft_activate::{
    Declared, DeclaredUser, Error, EtcConfig, Password, Security, User, Users, apply_accounts,
    check_config, declare,
};
use laft_store::Root;
use laft_test::TempDir;

const DATABASES: [(&str, &str); 4] = [
    (
        "passwd",
        "root:x:0:0::/root:/bin/sh\nguest:x:1500:1500::/home/guest:/bin/sh\n",
    ),
    ("shadow", "root:!:::::::\nguest:!:::::::\n"),
    ("group", "root:x:0:\nguest:x:1500:\n"),
    ("gshadow", "root:!::\nguest:!::\n"),
];

fn fixture() -> TempDir {
    let dir = laft_test::tempdir();
    fs::create_dir(dir.path().join("etc")).expect("can create etc");
    for (name, text) in DATABASES {
        fs::write(dir.path().join("etc").join(name), text).expect("can write database");
    }
    dir
}

fn assert_untouched(dir: &TempDir) {
    for (name, text) in DATABASES {
        assert_eq!(
            fs::read_to_string(dir.path().join("etc").join(name)).expect("readable"),
            text
        );
        for suffix in ["-", "+"] {
            assert!(
                !dir.path()
                    .join("etc")
                    .join(format!("{name}{suffix}"))
                    .exists()
            );
        }
    }
    for record in ["accounts.json", "etc-copies.json"] {
        assert!(!dir.path().join("laft/state").join(record).exists());
    }
}

#[test]
fn checking_immutable_accounts_without_root_refuses_without_writes() {
    for accounts in [
        BTreeMap::new(),
        BTreeMap::from([("guest".into(), User::default())]),
    ] {
        let dir = fixture();
        let config = EtcConfig {
            users: Users {
                mutable: false,
                accounts,
                ..Users::default()
            },
            ..EtcConfig::default()
        };
        let errors = check_config(&config, &Security::default());
        assert_untouched(&dir);
        assert_eq!(
            errors.iter().map(|e| e.code()).collect::<Vec<_>>(),
            [Some("C0416")]
        );
        assert!(errors[0].to_string().contains("root"));
        assert!(errors[0].to_string().contains("mutable"));
    }
}

#[test]
fn declaring_immutable_accounts_without_root_refuses_without_writes() {
    for accounts in [
        BTreeMap::new(),
        BTreeMap::from([("guest".into(), User::default())]),
    ] {
        let dir = fixture();
        let root = Root::new(dir.path());
        let lock = root.lock("test").expect("the lock is free");
        let result = declare(
            &lock,
            &Users {
                mutable: false,
                accounts,
                ..Users::default()
            },
        );
        assert_untouched(&dir);
        let Err(Error::Config(errors)) = result else {
            panic!("missing root must be rejected");
        };
        assert_eq!(
            errors.iter().map(|e| e.code()).collect::<Vec<_>>(),
            [Some("C0416")]
        );
    }
}

fn stored_user(uid: u32) -> DeclaredUser {
    DeclaredUser {
        uid,
        gid: 0,
        description: String::new(),
        home: "/root".into(),
        create_home: false,
        shell: "/bin/sh".into(),
        password: Password::Unset,
    }
}

#[test]
fn applying_immutable_records_without_root_uid_zero_refuses_before_writes() {
    for users in [
        BTreeMap::new(),
        BTreeMap::from([("alias".into(), stored_user(0))]),
        BTreeMap::from([("root".into(), stored_user(1000))]),
    ] {
        let dir = fixture();
        let root = Root::new(dir.path());
        let lock = root.lock("test").expect("the lock is free");
        let generation = dir.path().join("laft/gens/1");
        fs::create_dir_all(&generation).expect("can create generation");
        Declared {
            version: 1,
            mutable: false,
            users,
            groups: BTreeMap::new(),
        }
        .write(&generation)
        .expect("can store record");
        let result = apply_accounts(&lock, 1);
        assert_untouched(&dir);
        assert!(matches!(result, Err(Error::Corrupt { path, reason })
            if path == generation.join("accounts.json") && reason.contains("root") && reason.contains("uid 0")));
        assert!(!dir.path().join("etc/.pwd.lock").exists());
    }
}

#[test]
fn immutable_root_defaults_resolve_zero_ids_and_remove_other_accounts() {
    let dir = fixture();
    let root = Root::new(dir.path());
    let lock = root.lock("test").expect("the lock is free");
    let config = EtcConfig {
        users: Users {
            mutable: false,
            accounts: BTreeMap::from([("root".into(), User::default())]),
            ..Users::default()
        },
        ..EtcConfig::default()
    };
    assert!(check_config(&config, &Security::default()).is_empty());
    let declared = declare(&lock, &config.users).expect("declared root is valid");
    assert_eq!(
        (declared.users["root"].uid, declared.users["root"].gid),
        (0, 0)
    );
    assert_eq!(declared.groups["root"].gid, 0);
    let generation = dir.path().join("laft/gens/1");
    fs::create_dir_all(&generation).expect("can create generation");
    declared.write(&generation).expect("can store record");
    let report = apply_accounts(&lock, 1).expect("root-only immutable accounts apply");
    assert_eq!(report.removed_users, ["guest"]);
    assert_eq!(report.removed_groups, ["guest"]);
    for (name, _) in DATABASES {
        let text = fs::read_to_string(dir.path().join("etc").join(name)).expect("readable");
        assert_eq!(text.lines().count(), 1, "{name}: {text}");
        assert!(text.starts_with("root:"), "{name}: {text}");
    }
}

#[test]
fn mutable_accounts_do_not_require_a_root_declaration() {
    let dir = fixture();
    let root = Root::new(dir.path());
    let lock = root.lock("test").expect("the lock is free");
    assert!(check_config(&EtcConfig::default(), &Security::default()).is_empty());
    assert!(
        declare(&lock, &Users::default())
            .expect("mutable accounts remain valid")
            .users
            .is_empty()
    );
}
