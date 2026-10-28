use std::fs::{self, File};
use std::os::unix::fs::symlink;
use std::time::{Duration, SystemTime};

use rustix::fs::FlockOperation;

use crate::test_support::{Built, Env, Pkg, apply, assert_consistent, commit, name};
use crate::{Decision, GcPolicy, Subject, plan_gc};

fn deleted(decisions: &[Decision]) -> Vec<String> {
    decisions
        .iter()
        .filter(|d| !d.keep)
        .map(|d| d.subject.to_string())
        .collect()
}

fn line<'a>(explained: &'a str, subject: &str) -> &'a str {
    explained
        .lines()
        .find(|l| l[9..].starts_with(&format!("{subject} ")))
        .unwrap_or_else(|| panic!("no line for {subject} in\n{explained}"))
}

#[test]
fn gc_keeps_the_newest_and_pointed_generations() {
    let env = Env::new();
    let base = Pkg::new("base", "1")
        .file("usr/lib/libbase.so", b"base")
        .build(&env);
    let mut packages: Vec<Built> = Vec::new();
    for i in 1..=15 {
        let package = Pkg::new(&format!("pkg{i}"), "1")
            .file(&format!("usr/bin/pkg{i}"), format!("{i}").as_bytes())
            .build(&env);
        assert_eq!(commit(&env, &[&base, &package]), i);
        packages.push(package);
    }
    let run = env.root.run_dir();
    fs::create_dir_all(run.join("mounts")).expect("can create the runtime directory");
    symlink("mounts/3", run.join("booted")).expect("can link");
    symlink("mounts/15", run.join("system")).expect("can link");

    let lock = env.lock();
    let policy = GcPolicy {
        keep_generations: 10,
        keep_days: 0,
    };
    let plan = plan_gc(&lock, &policy, SystemTime::now()).expect("plans");
    let mut gone = deleted(plan.decisions());
    gone[4..].sort();
    assert_eq!(
        gone,
        ["gen 5", "gen 4", "gen 2", "gen 1"]
            .into_iter()
            .chain(["pkg1 1-r1", "pkg2 1-r1", "pkg4 1-r1", "pkg5 1-r1"])
            .collect::<Vec<_>>()
    );
    let explained = plan.to_string();
    assert_eq!(
        line(&explained, "gen 15"),
        "kept     gen 15         active system, default, newest 10"
    );
    assert_eq!(line(&explained, "gen 3"), "kept     gen 3          booted");
    assert_eq!(
        line(&explained, "gen 1"),
        "deleted  gen 1          not among the newest 10"
    );
    assert!(line(&explained, "base 1-r1").ends_with(
        "gen 15, gen 14, gen 13, gen 12, gen 11, gen 10, gen 9, gen 8, gen 7, gen 6, gen 3"
    ));
    assert_eq!(
        line(&explained, "pkg1 1-r1"),
        "deleted  pkg1 1-r1      no retained generation or view references it"
    );

    plan.execute(&lock).expect("collects");
    assert_eq!(
        env.root.generations().expect("lists"),
        vec![3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
    );
    let entries = env.root.store().list().expect("lists");
    for (i, package) in packages.iter().enumerate() {
        let kept = !matches!(i + 1, 1 | 2 | 4 | 5);
        assert_eq!(entries.contains(&package.dir), kept, "pkg{}", i + 1);
    }
    assert!(entries.contains(&base.dir));
    assert_consistent(&env.root);
}

#[test]
fn gc_keeps_recent_generations() {
    let env = Env::new();
    let tool = Pkg::new("tool", "1")
        .file("usr/bin/tool", b"tool")
        .build(&env);
    for _ in 0..3 {
        commit(&env, &[&tool]);
    }
    let lock = env.lock();
    let policy = GcPolicy {
        keep_generations: 1,
        keep_days: 30,
    };
    let plan = plan_gc(&lock, &policy, SystemTime::now()).expect("plans");
    assert_eq!(deleted(plan.decisions()), Vec::<String>::new());
    assert!(line(&plan.to_string(), "gen 1").ends_with("younger than 30 days"));

    let later = SystemTime::now() + Duration::from_secs(31 * 86_400);
    let plan = plan_gc(&lock, &policy, later).expect("plans");
    assert_eq!(deleted(plan.decisions()), ["gen 2", "gen 1"]);
    assert_eq!(
        line(&plan.to_string(), "gen 1"),
        "deleted  gen 1          not among the newest 1 and older than 30 days"
    );
}

#[test]
fn gc_keeps_pinned_entries_and_referenced_views() {
    let env = Env::new();
    let pinned = Pkg::new("pinned", "1")
        .file("usr/bin/pinned", b"p")
        .build(&env);
    let old = Pkg::new("old", "1").file("usr/bin/old", b"o").build(&env);
    let viewed = Pkg::new("viewed", "1")
        .file("usr/bin/viewed", b"v")
        .build(&env);
    let used = Pkg::new("used", "1").file("usr/bin/used", b"u").build(&env);
    let tool = Pkg::new("tool", "1").file("usr/bin/tool", b"t").build(&env);
    {
        let lock = env.lock();
        apply(&lock, &[&pinned, &old], None).expect("generation 1");
        apply(&lock, &[&viewed], Some("dev")).expect("generation 2 with dev@1");
        apply(&lock, &[&used], Some("dev")).expect("generation 3 with dev@2");
        apply(&lock, &[&tool], Some("lab")).expect("generation 4 with lab@1");
    }
    symlink(
        format!("../../store/{}", pinned.dir),
        env.root.pins_dir().join("pinned"),
    )
    .expect("can pin");
    symlink("1", env.root.views_dir().join("dev/current")).expect("can link");
    let session =
        File::open(env.root.view_dir(&name("dev"), 2).join("view.json")).expect("view.json exists");
    rustix::fs::flock(&session, FlockOperation::LockShared).expect("can lock");

    let lock = env.lock();
    let policy = GcPolicy {
        keep_generations: 1,
        keep_days: 0,
    };
    let plan = plan_gc(&lock, &policy, SystemTime::now()).expect("plans");
    assert_eq!(
        deleted(plan.decisions()),
        ["gen 3", "gen 2", "gen 1", "old 1-r1"]
    );
    let explained = plan.to_string();
    assert!(line(&explained, "view dev@1").ends_with("current"));
    assert!(line(&explained, "view dev@2").ends_with("in use"));
    assert!(line(&explained, "view lab@1").ends_with("gen 4"));
    assert!(line(&explained, "pinned 1-r1").ends_with("pinned"));
    assert!(line(&explained, "viewed 1-r1").ends_with("view dev@1"));
    assert!(line(&explained, "used 1-r1").ends_with("view dev@2"));

    plan.execute(&lock).expect("collects");
    drop(session);
    assert_eq!(env.root.generations().expect("lists"), vec![4]);
    assert_eq!(
        env.root.view_versions(&name("dev")).expect("lists"),
        vec![1, 2]
    );
    assert!(!env.root.store().list().expect("lists").contains(&old.dir));
    assert_consistent(&env.root);

    fs::remove_file(env.root.views_dir().join("dev/current")).expect("can unlink");
    let plan = plan_gc(&lock, &policy, SystemTime::now()).expect("plans");
    assert_eq!(
        deleted(plan.decisions()),
        ["view dev@1", "view dev@2", "used 1-r1", "viewed 1-r1"]
    );
    plan.execute(&lock).expect("collects");
    assert!(!env.root.views_dir().join("dev").exists());
    assert!(matches!(
        plan.decisions()[0].subject,
        Subject::Generation(4)
    ));
    assert_consistent(&env.root);
}
