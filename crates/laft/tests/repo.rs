//! Explicit repository selection without generation assembly or namespaces.

mod support;

use std::fs;

use laft_test::TempDir;
use support::{pkg, repo_tool, run, s};

fn repositories(core_enabled: bool) -> TempDir {
    let dir = laft_test::tempdir();
    let key = dir.path().join("online");
    repo_tool(&["keygen", "--unencrypted", "--out", s(&key)]);
    let out = dir.path().join("out");
    pkg("probe", "1")
        .file("usr/share/probe", b"probe")
        .write(&out);
    let repo = dir.path().join("repo");
    repo_tool(&[
        "index",
        "--repo",
        s(&repo),
        "--name",
        "core",
        "--channel",
        "current",
        "--key",
        s(&key.with_extension("key")),
        s(&out),
    ]);
    let published = dir.path().join("www");
    repo_tool(&["publish", s(&repo), s(&published)]);
    let public = fs::read_to_string(key.with_extension("pub")).expect("public key");
    let public = public.lines().last().expect("key line");
    let root = dir.path().join("root");
    fs::create_dir_all(root.join("etc/laft")).expect("configuration directory");
    fs::write(
        root.join("etc/laft/system.stav"),
        format!(
            "system {{\n\
         repo core {{ url = \"file://{}\", key = \"{public}\", enable = {core_enabled} }}\n\
         repo dormant {{ url = \"file://{}\", key = \"{public}\", enable = false }}\n\
         fs \"/\" {{ device = \"UUID=11111111-1111-1111-1111-111111111111\", fstype = ext4 }}\n\
         fs \"/boot\" {{ device = \"UUID=1A2B-3C4D\", fstype = vfat }}\n\
         }}\n",
            published.display(),
            dir.path().join("missing").display(),
        ),
    )
    .expect("configuration");
    dir
}

#[test]
fn explicit_disabled_repository_is_an_error_without_state_changes() {
    let dir = repositories(true);
    let root = dir.path().join("root");
    let output = run(&["--root", s(&root), "--offline", "repo", "sync", "dormant"]).expect(2);
    assert!(
        output.stderr.contains("repository dormant is disabled"),
        "{}",
        output.stderr
    );
    assert!(!root.join("laft").exists());
}

#[test]
fn invalid_repository_selections_do_not_partially_sync() {
    for names in [
        ["core", "dormant"],
        ["dormant", "core"],
        ["core", "missing"],
    ] {
        let dir = repositories(true);
        let root = dir.path().join("root");
        let mut args = vec!["--root", s(&root), "--offline", "repo", "sync"];
        args.extend(names);
        run(&args).expect(2);
        assert!(
            !root.join("laft").exists(),
            "invalid selection created state: {names:?}"
        );
    }
}

#[test]
fn sync_defaults_to_enabled_repositories_and_accepts_explicit_enabled_names() {
    for names in [vec![], vec!["core"]] {
        let dir = repositories(true);
        let root = dir.path().join("root");
        let mut args = vec!["--root", s(&root), "--offline", "--json", "repo", "sync"];
        args.extend(names);
        let events = run(&args).expect(0).events();
        let result = events
            .iter()
            .find(|e| e["event"] == "result")
            .expect("result");
        assert_eq!(result["repos"].as_array().expect("repositories").len(), 1);
        assert_eq!(result["repos"][0]["repo"], "core");
    }
}

#[test]
fn unqualified_sync_with_no_enabled_repositories_is_a_noop() {
    let dir = repositories(false);
    let root = dir.path().join("root");
    let events = run(&["--root", s(&root), "--offline", "--json", "repo", "sync"])
        .expect(0)
        .events();
    let result = events
        .iter()
        .find(|e| e["event"] == "result")
        .expect("result");
    assert!(result["repos"].as_array().expect("repositories").is_empty());
}

#[test]
fn disabled_repository_json_sync_reports_error_instead_of_success() {
    let dir = repositories(true);
    let root = dir.path().join("root");
    let events = run(&[
        "--root",
        s(&root),
        "--offline",
        "--json",
        "repo",
        "sync",
        "dormant",
    ])
    .expect(2)
    .events();
    assert_eq!(events.first().expect("start")["event"], "start");
    let error = events.last().expect("error");
    assert_eq!(error["event"], "error");
    assert_eq!(error["exit"], 2);
    assert!(
        error["message"]
            .as_str()
            .expect("message")
            .contains("repository dormant is disabled")
    );
    assert!(!events.iter().any(|e| e["event"] == "result"));
    assert!(!root.join("laft").exists());
}

#[test]
fn the_key_command_prints_the_public_key_of_the_local_repository() {
    let dir = laft_test::tempdir();
    let root = dir.path().join("root");
    fs::create_dir_all(root.join("etc/laft")).expect("configuration directory");
    fs::write(root.join("etc/laft/system.stav"), "system {\n}\n").expect("configuration");

    let first = run(&["--root", s(&root), "--offline", "repo", "key", "local"]).expect(0);
    let second = run(&["--root", s(&root), "--offline", "repo", "key", "local"]).expect(0);
    assert_eq!(
        first.stdout.trim(),
        second.stdout.trim(),
        "the key is stable"
    );
    assert!(first.stdout.trim().starts_with("RW"), "{}", first.stdout);
    let public = fs::read_to_string(root.join("laft/state/local.key.pub"))
        .expect("the public key file exists");
    let stored = public
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .expect("the public key file has a key line");
    assert_eq!(
        first.stdout.trim(),
        stored,
        "the printed key is the stored one"
    );

    let json = run(&[
        "--root",
        s(&root),
        "--offline",
        "--json",
        "repo",
        "key",
        "local",
    ])
    .expect(0);
    assert!(json.stdout.contains("\"key\""), "{}", json.stdout);

    run(&["--root", s(&root), "--offline", "repo", "key", "core"]).expect(2);
}
