//! The core loop below `--root`: plan, apply, nothing to do, and the
//! files a generation leaves in the root.

mod support;

use support::pkg;

fn hello() -> support::Pkg {
    pkg("hello", "1.0")
        .file("usr/bin/hello", b"#!/bin/sh\necho hello\n")
        .file("etc/hello.conf", b"greeting = hello\n")
}

#[test]
fn apply_builds_activates_and_makes_the_generation_the_default() {
    let fx = fixture!(hello());
    fx.configure(", hello", "");

    let plan = fx.laft(&["plan", "--json"]).expect(0).json();
    assert_eq!(plan["laft_json"], 1);
    let added: Vec<&str> = plan["views"]["system"]["added"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|p| p["name"].as_str().expect("a name"))
        .collect();
    assert!(
        added.contains(&"hello") && added.contains(&"linux"),
        "{plan}"
    );
    assert!(
        !fx.root().join("laft/gens/1").exists(),
        "plan builds nothing"
    );

    let run = fx.laft(&["apply", "-y"]).expect(0);
    assert!(run.stdout.contains("+ hello"), "{}", run.stdout);
    let root = fx.root();
    assert!(root.join("laft/gens/1/usr/bin/hello").exists());
    assert_eq!(
        std::fs::read_link(root.join("laft/gens/default")).expect("a link"),
        std::path::Path::new("1")
    );
    assert_eq!(
        std::fs::read_link(root.join("etc/hello.conf")).expect("managed"),
        std::path::Path::new("/etc/static/hello.conf")
    );
    assert!(fx.read("etc/passwd").contains("root:x:0:0:"));
    assert!(
        fx.read("boot/limine.conf").contains("Laft 1"),
        "{}",
        fx.read("boot/limine.conf")
    );

    let again = fx.laft(&["apply", "-y"]).expect(0);
    assert!(
        again.stdout.contains("system is up to date"),
        "{}",
        again.stdout
    );
    fx.laft(&["plan", "--exit-code"]).expect(0);
}

#[test]
fn upgrade_rollback_and_gc_move_between_generations() {
    let fx = fixture!(hello());
    fx.configure(", hello", "");
    fx.laft(&["apply", "-y", "-m", "first"]).expect(0);
    fx.publish(vec![
        pkg("hello", "2.0")
            .file("usr/bin/hello", b"#!/bin/sh\necho hello 2\n")
            .file("etc/hello.conf", b"greeting = hello 2\n"),
    ]);

    let kept = fx.laft(&["apply", "-y"]).expect(0);
    assert!(
        kept.stdout.contains("system is up to date"),
        "installed versions are kept"
    );
    fx.laft(&["plan", "--upgrade", "--refresh", "--exit-code"])
        .expect(12);
    let upgraded = fx.laft(&["upgrade", "-y"]).expect(0);
    assert!(upgraded.stdout.contains("hello"), "{}", upgraded.stdout);
    assert!(
        upgraded.stdout.contains("1.0-r1 \u{2192} 2.0-r1"),
        "{}",
        upgraded.stdout
    );
    assert_eq!(fx.etc("hello.conf"), "greeting = hello 2\n");

    let list = fx.laft(&["gens", "list", "--json"]).expect(0).json();
    let gens = list["generations"].as_array().expect("a list");
    assert_eq!(gens.len(), 2);
    assert_eq!(gens[0]["generation"], 2);
    assert_eq!(gens[0]["flags"], serde_json::json!(["*", "d"]));
    assert_eq!(gens[0]["parent"], 1);
    assert_eq!(gens[1]["message"], "first");

    fx.laft(&["rollback", "-y"]).expect(0);
    assert_eq!(fx.etc("hello.conf"), "greeting = hello\n");
    let list = fx.laft(&["gens", "list"]).expect(0);
    let first = list.stdout.lines().nth(1).expect("a row");
    assert!(first.starts_with("2 "), "newest first: {}", list.stdout);
    let one = list.stdout.lines().nth(2).expect("a row");
    assert!(one.starts_with("1    * d"), "{}", list.stdout);

    fx.laft(&["upgrade", "-y"]).expect(0);
    let dry = fx
        .laft(&[
            "gc",
            "--dry-run",
            "--keep-generations",
            "1",
            "--keep-days",
            "0",
        ])
        .expect(0);
    assert!(
        dry.stdout.contains("would remove 2 generations"),
        "{}",
        dry.stdout
    );
    assert!(fx.root().join("laft/gens/2").exists());
    let gc = fx
        .laft(&[
            "gc",
            "--keep-generations",
            "1",
            "--keep-days",
            "0",
            "--explain",
        ])
        .expect(0);
    assert!(
        gc.stdout
            .contains("removed 2 generations, 0 view versions, 1 store entry"),
        "{}",
        gc.stdout
    );
    assert!(gc.stdout.contains("deleted  gen 2"), "{}", gc.stdout);
    assert!(gc.stdout.contains("deleted  hello 1.0-r1"), "{}", gc.stdout);
    assert!(!fx.root().join("laft/gens/2").exists());
    assert!(!fx.root().join("laft/gens/1").exists());
    assert!(fx.root().join("laft/gens/3").exists());
}

#[test]
fn json_output_is_a_document_or_a_stream_of_events() {
    let fx = fixture!(hello());
    fx.configure(", hello", "");
    let refused = fx.laft(&["apply", "--json"]).expect(2);
    let events = refused.events();
    assert_eq!(events.first().expect("a start event")["event"], "start");
    assert_eq!(events.last().expect("an error")["event"], "error");
    assert_eq!(events.last().expect("an error")["exit"], 2);
    assert!(!fx.root().join("laft/gens/1").exists());

    let run = fx.laft(&["apply", "--json", "-y"]).expect(0);
    let events = run.events();
    assert_eq!(events[0]["event"], "start");
    assert_eq!(events[0]["laft_json"], 1);
    assert!(events.iter().any(|e| e["event"] == "plan"));
    let result = events.last().expect("a result");
    assert_eq!(result["event"], "result");
    assert_eq!(result["generation"], 1);
    assert_eq!(result["default"], true);
    assert!(
        run.stderr.is_empty(),
        "JSON mode prints no progress: {}",
        run.stderr
    );

    for args in [
        &["gens", "list", "--json"][..],
        &["plan", "--json"],
        &["gens", "show", "1", "--json"],
    ] {
        let doc = fx.laft(args).expect(0).json();
        assert_eq!(doc["laft_json"], 1, "{args:?}");
    }
}

#[test]
fn exit_statuses_name_what_went_wrong() {
    let fx = fixture!(hello());
    fx.configure(", hello", "  hostname = 3");
    let config = fx.laft(&["plan"]).expect(11);
    assert!(config.stderr.contains("hostname"), "{}", config.stderr);

    fx.configure(", hello@9", "");
    let resolve = fx.laft(&["plan"]).expect(4);
    assert!(resolve.stderr.contains("error[R001]"), "{}", resolve.stderr);

    fx.configure(", missing-thing", "");
    let unknown = fx.laft(&["plan"]).expect(11);
    assert!(unknown.stderr.contains("C0901"), "{}", unknown.stderr);

    fx.configure(", hello", "");
    std::fs::write(fx.root().join("etc/hello.conf"), "mine\n").expect("writable");
    let conflict = fx.laft(&["apply", "-y"]).expect(6);
    assert!(
        conflict.stderr.contains("/etc/hello.conf"),
        "{}",
        conflict.stderr
    );
    assert_eq!(fx.etc("hello.conf"), "mine\n", "nothing was overwritten");
    assert!(
        fx.root().join("laft/gens/1/gen.json").exists(),
        "the generation was built"
    );

    fx.laft(&["apply", "-y", "--adopt", "/etc/hello.conf"])
        .expect(0);
    assert_eq!(fx.etc("hello.conf.laft-orig"), "mine\n");
    assert_eq!(fx.etc("hello.conf"), "greeting = hello\n");

    fx.configure(", hello", "  timezone = \"Europe/Oslo\"");
    fx.laft(&["apply"]).expect(2);
    fx.laft(&["plan", "--exit-code"]).expect(12);
}
