use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::test_support::{Env, build_generation, config_with, link};
use crate::{EtcConfig, File};

fn none() -> BTreeSet<String> {
    BTreeSet::new()
}

fn adopt(path: &str) -> BTreeSet<String> {
    BTreeSet::from([path.to_owned()])
}

fn copied(text: &str) -> EtcConfig {
    let mut config = config_with(&[]);
    config.files.insert(
        "/etc/machine.conf".into(),
        File {
            text: Some(text.into()),
            copy: true,
            mode: 0o640,
            ..File::default()
        },
    );
    config
}

#[test]
fn managed_files_become_symlinks_into_the_static_tree() {
    let env = Env::new();
    let lock = env.lock();
    build_generation(
        &lock,
        1,
        &config_with(&[("/etc/ssh/sshd_config", "Port 22\n")]),
    );
    let report = reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    assert!(report.warnings.is_empty());
    assert!(report.linked.contains(&"ssh/sshd_config".to_owned()));
    assert_eq!(
        link(&env, "etc/ssh/sshd_config").as_deref(),
        Some("/etc/static/ssh/sshd_config")
    );
    assert_eq!(
        link(&env, "etc/hostname").as_deref(),
        Some("/etc/static/hostname")
    );
    assert!(
        !env.path("etc/passwd").exists(),
        "the accounts step writes it"
    );

    let record = ManagedRecord::load(&env.root).expect("readable");
    assert_eq!(record.generation, Some(1));
    assert!(record.managed.contains("ssh/sshd_config"));
    assert!(record.dirs.contains("ssh"));

    let again = reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    assert_eq!(again, EtcReport::default(), "reconciling is idempotent");
}

#[test]
fn an_unmanaged_file_stops_activation_before_anything_changes() {
    let env = Env::new();
    env.write("etc/hosts", "10.0.0.1 nas\n");
    let lock = env.lock();
    build_generation(&lock, 1, &config_with(&[]));
    let expected = vec![Conflict {
        path: "hosts".into(),
        kind: ConflictKind::Unmanaged,
    }];
    assert_eq!(check_etc(&lock, 1, &none()).expect("checks"), expected);
    let err = reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect_err("conflicts");
    assert!(matches!(&err, Error::Conflicts(c) if *c == expected));
    assert!(
        err.to_string()
            .contains("/etc/hosts exists and is not managed by laft"),
        "{err}"
    );
    assert_eq!(env.read("etc/hosts"), "10.0.0.1 nas\n");
    assert!(link(&env, "etc/hostname").is_none(), "nothing changed");
    assert!(!state::state_file(&env.root, "etc-managed.json").exists());
}

#[test]
fn adopting_moves_the_existing_file_aside() {
    let env = Env::new();
    env.write("etc/hosts", "10.0.0.1 nas\n");
    let lock = env.lock();
    build_generation(&lock, 1, &config_with(&[]));
    assert!(
        check_etc(&lock, 1, &adopt("/etc/hosts"))
            .expect("checks")
            .is_empty()
    );
    let report = reconcile_etc(&lock, 1, &adopt("/etc/hosts"), OnConflict::Stop).expect("adopts");
    assert_eq!(report.adopted, ["hosts"]);
    assert_eq!(env.read("etc/hosts.laft-orig"), "10.0.0.1 nas\n");
    assert_eq!(
        link(&env, "etc/hosts").as_deref(),
        Some("/etc/static/hosts")
    );
}

#[test]
fn adopting_never_overwrites_an_earlier_original() {
    let env = Env::new();
    env.write("etc/hosts", "new\n");
    env.write("etc/hosts.laft-orig", "old\n");
    let lock = env.lock();
    build_generation(&lock, 1, &config_with(&[]));
    let err = reconcile_etc(&lock, 1, &adopt("hosts"), OnConflict::Stop).expect_err("refused");
    assert!(matches!(
        &err,
        Error::Conflicts(c) if c[0].kind == ConflictKind::OrigExists
    ));
    assert_eq!(env.read("etc/hosts.laft-orig"), "old\n");
    assert_eq!(env.read("etc/hosts"), "new\n");
}

#[test]
fn symlinks_elsewhere_and_files_in_the_way_of_directories_conflict() {
    let env = Env::new();
    std::os::unix::fs::symlink("/run/hostname", env.path("etc/hostname")).expect("links");
    env.write("etc/ssh", "not a directory\n");
    let lock = env.lock();
    build_generation(
        &lock,
        1,
        &config_with(&[("/etc/ssh/sshd_config", "Port 22\n")]),
    );
    let conflicts = check_etc(&lock, 1, &none()).expect("checks");
    assert_eq!(
        conflicts,
        [
            Conflict {
                path: "hostname".into(),
                kind: ConflictKind::ForeignLink("/run/hostname".into()),
            },
            Conflict {
                path: "ssh/sshd_config".into(),
                kind: ConflictKind::NotDirectory("ssh".into()),
            },
        ]
    );
}

#[test]
fn a_user_replaced_managed_file_is_left_alone_with_a_warning_until_reset() {
    let env = Env::new();
    let lock = env.lock();
    build_generation(&lock, 1, &config_with(&[("/etc/motd", "hello\n")]));
    build_generation(&lock, 2, &config_with(&[("/etc/motd", "welcome\n")]));
    reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    fs::remove_file(env.path("etc/motd")).expect("removes");
    env.write("etc/motd", "mine\n");

    for generation in [2, 2] {
        let report =
            reconcile_etc(&lock, generation, &none(), OnConflict::Stop).expect("not a conflict");
        assert_eq!(report.warnings, [Warning::UserReplaced("motd".into())]);
        assert_eq!(env.read("etc/motd"), "mine\n");
    }

    let kept = reset_etc(&lock, &["/etc/motd".to_owned()]).expect("resets");
    assert_eq!(kept, ["motd"]);
    assert_eq!(env.read("etc/motd.laft-old"), "mine\n");
    assert_eq!(link(&env, "etc/motd").as_deref(), Some("/etc/static/motd"));
    let report = reconcile_etc(&lock, 2, &none(), OnConflict::Stop).expect("reconciles");
    assert!(report.warnings.is_empty());
}

#[test]
fn a_user_replaced_file_stays_when_it_is_no_longer_managed() {
    let env = Env::new();
    let lock = env.lock();
    build_generation(&lock, 1, &config_with(&[("/etc/motd", "hello\n")]));
    build_generation(&lock, 2, &config_with(&[]));
    reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    fs::remove_file(env.path("etc/motd")).expect("removes");
    env.write("etc/motd", "mine\n");
    let report = reconcile_etc(&lock, 2, &none(), OnConflict::Stop).expect("reconciles");
    assert!(report.unlinked.is_empty());
    assert_eq!(env.read("etc/motd"), "mine\n");
    let record = ManagedRecord::load(&env.root).expect("readable");
    assert!(!record.managed.contains("motd"));
}

#[test]
fn paths_no_longer_managed_are_unlinked_and_empty_directories_removed() {
    let env = Env::new();
    let lock = env.lock();
    let files = [
        ("/etc/app/one.conf", "1\n"),
        ("/etc/app/sub/two.conf", "2\n"),
        ("/etc/other/three.conf", "3\n"),
    ];
    build_generation(&lock, 1, &config_with(&files));
    build_generation(&lock, 2, &config_with(&[]));
    reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    env.write("etc/other/local.conf", "mine\n");

    let report = reconcile_etc(&lock, 2, &none(), OnConflict::Stop).expect("reconciles");
    assert_eq!(
        report.unlinked,
        ["app/one.conf", "app/sub/two.conf", "other/three.conf"]
    );
    assert!(
        !env.path("etc/app").exists(),
        "empty directories are removed"
    );
    assert_eq!(env.read("etc/other/local.conf"), "mine\n");
    let record = ManagedRecord::load(&env.root).expect("readable");
    assert!(record.dirs.contains("other"));
    assert!(!record.dirs.contains("app") && !record.dirs.contains("app/sub"));

    fs::remove_file(env.path("etc/other/local.conf")).expect("removes");
    reconcile_etc(&lock, 2, &none(), OnConflict::Stop).expect("reconciles");
    assert!(!env.path("etc/other").exists());
}

#[test]
fn copy_mode_files_follow_the_generation_unless_changed_outside_laft() {
    let env = Env::new();
    let lock = env.lock();
    build_generation(&lock, 1, &copied("one\n"));
    build_generation(&lock, 2, &copied("two\n"));
    build_generation(&lock, 3, &copied("three\n"));

    let report = reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    assert_eq!(report.copied, ["machine.conf"]);
    let path = env.path("etc/machine.conf");
    assert!(fs::symlink_metadata(&path).expect("exists").is_file());
    assert_eq!(crate::test_support::mode(&path), 0o640);

    reconcile_etc(&lock, 2, &none(), OnConflict::Stop).expect("unchanged files are replaced");
    assert_eq!(env.read("etc/machine.conf"), "two\n");

    env.write("etc/machine.conf", "edited\n");
    let err = reconcile_etc(&lock, 3, &none(), OnConflict::Stop).expect_err("changed outside");
    assert!(matches!(
        &err,
        Error::Conflicts(c) if c[0].kind == ConflictKind::Changed
    ));
    assert_eq!(env.read("etc/machine.conf"), "edited\n");

    let kept = reset_etc(&lock, &["machine.conf".to_owned()]).expect("resets");
    assert_eq!(kept, ["machine.conf"]);
    assert_eq!(env.read("etc/machine.conf"), "two\n");
    assert_eq!(env.read("etc/machine.conf.laft-old"), "edited\n");
    reconcile_etc(&lock, 3, &none(), OnConflict::Stop).expect("reset files are Laft's again");
    assert_eq!(env.read("etc/machine.conf"), "three\n");

    build_generation(&lock, 4, &config_with(&[]));
    reconcile_etc(&lock, 4, &none(), OnConflict::Stop).expect("reconciles");
    assert!(
        !path.exists(),
        "an unchanged copy is removed with its declaration"
    );
}

#[test]
fn reconciling_again_after_a_crash_finishes_the_job() {
    let env = Env::new();
    let lock = env.lock();
    build_generation(&lock, 1, &copied("one\n"));
    let mut newer = copied("two\n");
    newer.files.insert(
        "/etc/new/extra.conf".into(),
        File {
            text: Some("x".into()),
            ..File::default()
        },
    );
    build_generation(&lock, 2, &newer);
    reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");

    // A crash while switching to 2: the widened records are saved, a new
    // link and directory exist and the copy has the new content, but the
    // final records were never written.
    let mut record = ManagedRecord::load(&env.root).expect("readable");
    record.managed.insert("new/extra.conf".into());
    record.dirs.insert("new".into());
    record.save(&env.root).expect("saves");
    let mut copies = CopiesRecord::load(&env.root).expect("readable");
    copies
        .pending
        .insert("machine.conf".into(), Blake3Hash::of(b"two\n"));
    copies.save(&env.root).expect("saves");
    env.write("etc/machine.conf", "two\n");
    fs::create_dir_all(env.path("etc/new")).expect("creates");
    std::os::unix::fs::symlink("/etc/static/new/extra.conf", env.path("etc/new/extra.conf"))
        .expect("links");

    // The machine boots the old generation again.
    let report = reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("no conflict");
    assert_eq!(report.unlinked, ["new/extra.conf"]);
    assert!(!env.path("etc/new").exists());
    assert_eq!(env.read("etc/machine.conf"), "one\n");
    let copies = CopiesRecord::load(&env.root).expect("readable");
    assert!(copies.pending.is_empty());
    assert_eq!(copies.files["machine.conf"], Blake3Hash::of(b"one\n"));
}

#[test]
fn reset_refuses_paths_it_cannot_restore() {
    let env = Env::new();
    let lock = env.lock();
    assert!(matches!(
        reset_etc(&lock, &["hosts".to_owned()]),
        Err(Error::Invalid(_))
    ));
    build_generation(&lock, 1, &config_with(&[]));
    reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    for path in ["/etc/shadow", "/etc/machine-id"] {
        let err = reset_etc(&lock, &["/etc/hosts".to_owned(), path.to_owned()]).expect_err(path);
        assert!(matches!(err, Error::NotResettable { .. }), "{path}: {err}");
    }
    assert_eq!(
        link(&env, "etc/hosts").as_deref(),
        Some("/etc/static/hosts")
    );
}

/// Reconciles a changed copy-mode `machine.conf` and resets it once, so
/// that `machine.conf.laft-old` holds `edited\n`.
fn reset_once(env: &Env, lock: &Lock) {
    build_generation(lock, 1, &copied("one\n"));
    reconcile_etc(lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    env.write("etc/machine.conf", "edited\n");
    reset_etc(lock, &["machine.conf".to_owned()]).expect("resets");
}

#[test]
fn reset_never_overwrites_an_earlier_backup() {
    let env = Env::new();
    let lock = env.lock();
    reset_once(&env, &lock);

    env.write("etc/machine.conf", "edited again\n");
    let err = reset_etc(&lock, &["machine.conf".to_owned()]).expect_err("backup exists");
    assert!(matches!(err, Error::NotResettable { .. }), "{err}");
    assert_eq!(env.read("etc/machine.conf"), "edited again\n");
    assert_eq!(env.read("etc/machine.conf.laft-old"), "edited\n");
}

#[test]
fn resetting_laft_content_keeps_no_backup() {
    let env = Env::new();
    let lock = env.lock();
    reset_once(&env, &lock);

    let kept = reset_etc(&lock, &["machine.conf".to_owned()]).expect("resets");
    assert!(kept.is_empty(), "{kept:?}");
    assert_eq!(env.read("etc/machine.conf"), "one\n");
    assert_eq!(env.read("etc/machine.conf.laft-old"), "edited\n");
}

#[test]
fn a_failed_reset_leaves_the_live_file_in_place() {
    let env = Env::new();
    let lock = env.lock();
    build_generation(&lock, 1, &copied("one\n"));
    reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    env.write("etc/machine.conf", "edited\n");
    let static_etc = crate::state::gen_dir(lock.root(), 1).join("etc");
    fs::set_permissions(&static_etc, fs::Permissions::from_mode(0o755)).expect("can chmod");
    fs::remove_file(static_etc.join("machine.conf")).expect("can remove");

    reset_etc(&lock, &["machine.conf".to_owned()]).expect_err("the static copy is gone");
    assert_eq!(env.read("etc/machine.conf"), "edited\n");
    assert!(!env.path("etc/machine.conf.laft-old").exists());
}

#[test]
fn reset_refuses_paths_outside_etc() {
    let env = Env::new();
    let lock = env.lock();
    build_generation(&lock, 1, &config_with(&[]));
    reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    for path in [
        "/etchosts",
        "../passwd",
        "a/../hosts",
        "./hosts",
        "/etc/../hosts",
        "",
    ] {
        let err = reset_etc(&lock, &[path.to_owned()]).expect_err(path);
        assert!(
            matches!(err, Error::NotResettable { reason, .. } if reason.contains("not below /etc")),
            "{path}: {err}"
        );
    }
}

#[test]
fn stale_pending_hashes_are_dropped_with_their_declaration() {
    let env = Env::new();
    let lock = env.lock();
    build_generation(&lock, 1, &copied("one\n"));
    build_generation(&lock, 2, &config_with(&[]));
    reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    let mut copies = CopiesRecord::load(&env.root).expect("readable");
    copies
        .pending
        .insert("machine.conf".into(), Blake3Hash::of(b"two\n"));
    copies.save(&env.root).expect("saves");

    reconcile_etc(&lock, 2, &none(), OnConflict::Stop).expect("reconciles");
    let copies = CopiesRecord::load(&env.root).expect("readable");
    assert!(copies.pending.is_empty(), "{:?}", copies.pending);
}

#[test]
fn a_changed_copy_that_is_no_longer_declared_is_kept_with_a_warning() {
    let env = Env::new();
    let lock = env.lock();
    build_generation(&lock, 1, &copied("one\n"));
    build_generation(&lock, 2, &config_with(&[]));
    reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    env.write("etc/machine.conf", "edited\n");

    let report = reconcile_etc(&lock, 2, &none(), OnConflict::Stop).expect("reconciles");
    assert_eq!(
        report.warnings,
        [Warning::ChangedCopyKept("machine.conf".into())]
    );
    assert_eq!(env.read("etc/machine.conf"), "edited\n");
}

#[test]
fn a_copy_replaced_by_a_link_or_directory_is_kept_with_a_warning() {
    for replace in ["link", "dir"] {
        let env = Env::new();
        let lock = env.lock();
        build_generation(&lock, 1, &copied("one\n"));
        build_generation(&lock, 2, &config_with(&[]));
        reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
        let live = env.path("etc/machine.conf");
        fs::remove_file(&live).unwrap();
        match replace {
            "link" => std::os::unix::fs::symlink("elsewhere", &live).unwrap(),
            _ => fs::create_dir(&live).unwrap(),
        }

        let report = reconcile_etc(&lock, 2, &none(), OnConflict::Stop).expect("reconciles");
        assert_eq!(
            report.warnings,
            [Warning::ChangedCopyKept("machine.conf".into())],
            "{replace}"
        );
        assert!(fs::symlink_metadata(&live).is_ok(), "{replace}");
    }
}

#[test]
fn a_managed_directory_replaced_by_a_file_does_not_stop_reconciling() {
    let env = Env::new();
    let lock = env.lock();
    build_generation(&lock, 1, &config_with(&[("/etc/app/one.conf", "1\n")]));
    build_generation(&lock, 2, &config_with(&[]));
    reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    fs::remove_dir_all(env.path("etc/app")).expect("can remove");
    env.write("etc/app", "a file now\n");

    reconcile_etc(&lock, 2, &none(), OnConflict::Stop).expect("reconciles");
    assert_eq!(env.read("etc/app"), "a file now\n");
    let record = ManagedRecord::load(&env.root).expect("readable");
    assert!(!record.dirs.contains("app"), "{:?}", record.dirs);
}

#[test]
fn an_emptied_directory_laft_created_is_replaced_without_a_conflict() {
    let env = Env::new();
    let lock = env.lock();
    let files = [
        ("/etc/app/one.conf", "1\n"),
        ("/etc/app/sub/two.conf", "2\n"),
    ];
    build_generation(&lock, 1, &config_with(&files));
    build_generation(&lock, 2, &config_with(&[("/etc/app", "now a file\n")]));
    let mut copy = copied("copied\n");
    let machine = copy.files.remove("/etc/machine.conf").expect("declared");
    copy.files.insert("/etc/app".into(), machine);
    build_generation(&lock, 3, &copy);

    reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    reconcile_etc(&lock, 2, &none(), OnConflict::Stop).expect("no conflict");
    assert_eq!(link(&env, "etc/app").as_deref(), Some("/etc/static/app"));

    reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    reconcile_etc(&lock, 3, &none(), OnConflict::Stop).expect("no conflict");
    assert_eq!(env.read("etc/app"), "copied\n");
    let record = ManagedRecord::load(&env.root).expect("readable");
    assert!(!record.dirs.contains("app"), "{:?}", record.dirs);
}

#[test]
fn a_directory_that_keeps_files_or_was_not_created_by_laft_conflicts() {
    let env = Env::new();
    let lock = env.lock();
    build_generation(&lock, 1, &config_with(&[("/etc/app/one.conf", "1\n")]));
    build_generation(
        &lock,
        2,
        &config_with(&[("/etc/app", "x\n"), ("/etc/own", "y\n")]),
    );
    reconcile_etc(&lock, 1, &none(), OnConflict::Stop).expect("reconciles");
    env.write("etc/app/local.conf", "mine\n");
    fs::create_dir(env.path("etc/own")).expect("creates");

    let err = reconcile_etc(&lock, 2, &none(), OnConflict::Stop).expect_err("conflicts");
    let Error::Conflicts(conflicts) = err else {
        panic!("{err}");
    };
    let paths: Vec<&str> = conflicts.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["app", "own"]);
    assert_eq!(env.read("etc/app/local.conf"), "mine\n");
}
