use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::{Entry, Rendered, load_units, render};
use crate::test_support::{config, example, myapp, valid};
use crate::{Log, Service, ServiceKind};

/// Renders every entry as text: a header with the path and mode or link
/// target, followed by the content of files.
fn dump(rendered: &Rendered, prefix: &str) -> String {
    let mut out = String::new();
    for (path, entry) in rendered.entries() {
        if !path.starts_with(prefix) {
            continue;
        }
        match entry {
            Entry::File { mode, bytes } => {
                let text = std::str::from_utf8(bytes).expect("rendered files are text");
                let _ = write!(out, "=== {path} ({mode:o})\n{text}");
                if !text.ends_with('\n') {
                    out.push('\n');
                }
            }
            Entry::Symlink(target) => {
                let _ = writeln!(out, "=== {path} -> {target}");
            }
        }
    }
    out
}

#[test]
fn example_renders_the_runit_tree() {
    let rendered = render(&valid(&example()), 42).expect("renders");
    insta::assert_snapshot!(dump(&rendered, "services/runit/"));
}

#[test]
fn example_renders_the_entry_points_in_etc() {
    let rendered = render(&valid(&example()), 42).expect("renders");
    insta::assert_snapshot!(dump(&rendered, "etc/"));
}

#[test]
fn example_renders_the_spec_files() {
    let rendered = render(&valid(&example()), 42).expect("renders");
    insta::assert_snapshot!(dump(&rendered, "services/spec/"));
}

#[test]
fn log_policy_shapes_the_service_directory() {
    let quiet = Service {
        log: Log::Discard,
        ..Service::longrun(&["/usr/bin/quiet"])
    };
    let loud = Service {
        log: Log::Console,
        ..Service::longrun(&["/usr/bin/loud"])
    };
    let rendered =
        render(&valid(&config(&[("quiet", quiet), ("loud", loud)])), 1).expect("renders");
    let mut out = dump(&rendered, "services/runit/sv/loud/");
    out.push_str(&dump(&rendered, "services/runit/sv/quiet/"));
    insta::assert_snapshot!(out);
}

#[test]
fn rendering_is_deterministic_on_disk() {
    let set = valid(&example());
    let dir = tempfile::tempdir().expect("can create a temporary directory");
    let (first, second) = (dir.path().join("1"), dir.path().join("2"));
    render(&set, 7)
        .expect("renders")
        .write(&first)
        .expect("writes");
    render(&set, 7)
        .expect("renders")
        .write(&second)
        .expect("writes");
    assert_eq!(tree(&first), tree(&second));
    assert!(
        tree(&first)
            .iter()
            .any(|(p, _)| p == "services/runit/sv/myapp/log/run")
    );
}

/// Every file and symlink below `root` with its mode and content or target,
/// sorted by path.
fn tree(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        for item in fs::read_dir(&dir).expect("readable") {
            let path = item.expect("readable").path();
            let meta = fs::symlink_metadata(&path).expect("exists");
            let relative = path
                .strip_prefix(root)
                .expect("below the root")
                .display()
                .to_string();
            let content = if meta.is_dir() {
                pending.push(path.clone());
                "dir".to_owned()
            } else if meta.file_type().is_symlink() {
                fs::read_link(&path).expect("a link").display().to_string()
            } else {
                String::from_utf8(fs::read(&path).expect("readable")).expect("text")
            };
            out.push((relative, format!("{:o} {content}", meta.mode() & 0o7777)));
        }
    }
    out.sort();
    out
}

#[test]
fn units_read_back_from_disk_match_the_rendered_ones() {
    let rendered = render(&valid(&example()), 3).expect("renders");
    let dir = tempfile::tempdir().expect("can create a temporary directory");
    rendered.write(dir.path()).expect("writes");
    let loaded = load_units(&dir.path().join("services")).expect("loads");
    assert_eq!(&loaded, rendered.units());
}

#[test]
fn unit_hash_follows_structure_and_spec_hash_follows_the_spec() {
    let units = |app: Service| {
        render(&valid(&config(&[("myapp", app)])), 1)
            .expect("renders")
            .units()["myapp"]
            .clone()
    };
    let mut base = myapp();
    base.after.clear();
    let mut env = base.clone();
    env.env.insert("RUST_LOG".into(), "debug".into());
    let mut log = base.clone();
    log.log = Log::Console;
    let (base, env, log) = (units(base), units(env), units(log));
    assert_eq!(base.unit_hash, env.unit_hash);
    assert_ne!(base.spec_hash, env.spec_hash);
    assert_ne!(base.unit_hash, log.unit_hash);
}

#[test]
fn early_kind_change_changes_the_unit() {
    let mut services = config(&[]);
    let before = render(&valid(&services), 1).expect("renders").units()["hostname"].clone();
    let hostname = services.services.get_mut("hostname").expect("standard");
    hostname.kind = ServiceKind::Longrun;
    hostname.exec = std::mem::take(&mut hostname.up);
    hostname.on_change = None;
    let after = render(&valid(&services), 1).expect("renders").units()["hostname"].clone();
    assert_ne!(before.unit_hash, after.unit_hash);
}

#[test]
fn activation_runs_first_even_before_independent_early_longruns() {
    let mut watcher = Service::longrun(&["/usr/bin/watcher"]);
    watcher.early = true;
    let rendered = render(&valid(&config(&[("watcher", watcher)])), 1).expect("renders");
    let Entry::File { bytes, .. } = &rendered.entries()["services/runit/stage/1"] else {
        panic!("stage 1 is a file");
    };
    let calls: Vec<&str> = std::str::from_utf8(bytes)
        .expect("text")
        .lines()
        .filter(|l| l.starts_with("/usr/bin/laft"))
        .collect();
    assert_eq!(
        calls[0],
        "/usr/bin/laft svc up --backend runit laft-activate"
    );
    assert_eq!(
        calls[1],
        "/usr/bin/laft svc exec --backend runit --background watcher"
    );
    let Entry::File { bytes, .. } = &rendered.entries()["services/runit/early-longruns"] else {
        panic!("early-longruns is a file");
    };
    assert_eq!(bytes, b"watcher\nudevd\n");
}

#[test]
fn shutdown_waits_for_the_slowest_supervised_service() {
    let slow = Service {
        stop_timeout: 90,
        ..Service::longrun(&["/usr/bin/slow"])
    };
    let rendered = render(&valid(&config(&[("slow", slow)])), 1).expect("renders");
    let Entry::File { bytes, .. } = &rendered.entries()["services/runit/stage/3"] else {
        panic!("stage 3 is a file");
    };
    let text = std::str::from_utf8(bytes).expect("text");
    assert!(text.contains("sv -w 90 force-stop /run/runit/service/*\n"));
}

#[test]
fn the_finish_script_passes_runits_exit_code_and_signal_to_the_launcher() {
    let rendered = render(&valid(&example()), 1).expect("renders");
    let Some(Entry::File { bytes, .. }) = rendered.entries().get("services/runit/sv/myapp/finish")
    else {
        panic!("myapp has a finish script");
    };
    let script = String::from_utf8(bytes.clone())
        .expect("text")
        .replace("exec /usr/bin/laft", "exec printf '%s\\n'");
    let dir = tempfile::tempdir().expect("can create a temporary directory");
    let path = dir.path().join("finish");
    fs::write(&path, script).expect("write");
    let output = std::process::Command::new("/bin/sh")
        .arg(&path)
        .args(["-1", "15"])
        .output()
        .expect("runs");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "svc\nfinish\n--backend\nrunit\nmyapp\n-1\n15\n"
    );
}
