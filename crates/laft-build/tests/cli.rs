//! Drives the `laft-build` binary with the example recipes and an empty
//! build root.

mod support;

use std::path::Path;
use std::process::{Command, Output};

fn laft_build(root: &Path, args: &[&str]) -> Output {
    let recipes = support::core();
    let mut all = vec!["--recipes", recipes.to_str().expect("a UTF-8 path")];
    all.extend(args);
    without_recipes(root, &all)
}

/// Runs `laft-build` with the build root `root` and `args` only.
fn without_recipes(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_laft-build"))
        .env_remove("LAFT_RECIPES")
        .env_remove("LAFT_BUILD_ROOT")
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .expect("the binary runs")
}

/// Writes a stage file below `dir` that builds `packages` from the example
/// recipes and runs a script inside the root, and returns it.
fn stage_file(dir: &Path, packages: &str) -> String {
    let file = dir.join("stage.stav");
    std::fs::write(
        &file,
        format!(
            "stage s {{\n  recipes = \"{}\"\n  steps = [\n    {{ name = \"tools\", packages = [{packages}] }},\n    \
             {{ name = \"check\", place = root, script = \"true\" }},\n  ]\n}}\n",
            support::core().display()
        ),
    )
    .expect("the test directory is writable");
    file.to_str().expect("a UTF-8 path").to_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn eval_prints_the_static_plan_as_json() {
    let dir = laft_test::tempdir();
    let output = laft_build(dir.path(), &["--json", "eval", "zlib:static"]);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let recipe = &value[0];
    assert_eq!(recipe["variant"], "static");
    assert_eq!(recipe["env"]["DESTDIR"], "/out/usr/x86_64-laft-linux-musl");
    assert_eq!(recipe["link_deps"], serde_json::json!([]));
}

#[test]
fn eval_shows_the_rendered_phases() {
    let dir = laft_test::tempdir();
    let output = laft_build(dir.path(), &["eval", "zlib:static"]);
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(
        text.starts_with("zlib:static 1.3.1-r1\nplan hash: b3:"),
        "{text}"
    );
    assert!(text.contains("SOURCE_DATE_EPOCH=315532800"));
    assert!(text.contains("## configure\n#!/bin/sh\nset -eu\n"));
}

#[test]
fn a_new_package_is_planned_with_its_trigger() {
    let dir = laft_test::tempdir();
    let output = laft_build(dir.path(), &["plan", "zlib"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "zlib  T7 new\nzlib:static  T7 new\n");
    let output = laft_build(dir.path(), &["--json", "plan", "zlib:static", "--rebuild"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"build": [{"item": "zlib:static", "triggers": ["T7 new", "T8 forced"]}]})
    );
}

#[test]
fn a_dependency_missing_from_the_store_fails_planning() {
    let dir = laft_test::tempdir();
    let output = laft_build(dir.path(), &["plan", "pixman"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("R021"));
}

#[test]
fn bootstrap_dry_run_lists_what_it_would_build() {
    let dir = laft_test::tempdir();
    let output = laft_build(
        dir.path(),
        &[
            "bootstrap",
            "--root",
            "/",
            "--dry-run",
            "zlib:static",
            "base",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        "zlib:static: would build\nbase: would build\n"
    );
}

#[test]
fn an_installing_dry_run_lists_what_it_would_install() {
    let dir = laft_test::tempdir();
    let dest = dir.path().join("system");
    let dest = dest.to_str().expect("a UTF-8 path");
    let output = laft_build(
        dir.path(),
        &[
            "bootstrap",
            "--root",
            "/",
            "--install",
            dest,
            "--dry-run",
            "zlib:static",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "zlib:static: would install\n");
    let output = laft_build(
        dir.path(),
        &[
            "bootstrap",
            "--root",
            "/",
            "--path",
            "/a:b",
            "--dry-run",
            "zlib",
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn an_unpacking_dry_run_lists_what_it_would_build_and_unpack() {
    let dir = laft_test::tempdir();
    let dest = dir.path().join("system");
    let dest = dest.to_str().expect("a UTF-8 path");
    let output = laft_build(
        dir.path(),
        &[
            "bootstrap",
            "--root",
            "/",
            "--unpack",
            dest,
            "--dry-run",
            "zlib:dynamic",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "zlib: would build and unpack\n");
    let output = laft_build(
        dir.path(),
        &[
            "bootstrap",
            "--root",
            "/",
            "--unpack",
            dest,
            "--install",
            dest,
            "--dry-run",
            "zlib",
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn relative_bootstrap_directories_mean_the_working_directory() {
    if !Path::new("/usr/bin/sh").exists() {
        println!("skipping: the host has no /usr/bin/sh");
        return;
    }
    let dir = laft_test::tempdir();
    let install = "  empty = true\n  build custom {}\n  phases {\n    \
                   install = \"echo hi > \\\"$DESTDIR/greeting\\\"; echo \\\"$PATH\\\" > \\\"$DESTDIR/path\\\"\"\n  }";
    support::write_repo(
        &dir.path().join("repo"),
        "t",
        "",
        &[(
            "recipes/greet/package.stav",
            &support::recipe("greet", install),
        )],
    );
    std::fs::create_dir(dir.path().join("host")).unwrap();
    std::os::unix::fs::symlink("/usr", dir.path().join("host/usr")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_laft-build"))
        .env_remove("LAFT_RECIPES")
        .env_remove("LAFT_BUILD_ROOT")
        .current_dir(dir.path())
        .args(["--root", "build", "--recipes", "repo", "bootstrap"])
        .args(["--root", "host", "--install", "sys", "--path", "tools/bin"])
        .arg("greet")
        .output()
        .expect("the binary runs");
    assert!(output.status.success(), "{output:?}");
    let sys = dir.path().join("sys");
    assert_eq!(
        std::fs::read_to_string(sys.join("greeting")).unwrap(),
        "hi\n"
    );
    assert_eq!(
        std::fs::read_to_string(sys.join("path")).unwrap(),
        format!(
            "{}:{}\n",
            dir.path().join("tools/bin").display(),
            dir.path().join("host/usr/bin").display()
        )
    );
}

#[test]
fn a_relative_build_root_from_the_environment_means_the_working_directory() {
    if !Path::new("/usr/bin/sh").exists() {
        println!("skipping: the host has no /usr/bin/sh");
        return;
    }
    let dir = laft_test::tempdir();
    let install = "  empty = true\n  build custom {}\n  phases {\n    \
                   install = \"echo \\\"$HOME\\\" > \\\"$DESTDIR/home\\\"\"\n  }";
    support::write_repo(
        &dir.path().join("repo"),
        "t",
        "",
        &[(
            "recipes/greet/package.stav",
            &support::recipe("greet", install),
        )],
    );
    std::fs::create_dir(dir.path().join("host")).unwrap();
    std::os::unix::fs::symlink("/usr", dir.path().join("host/usr")).unwrap();
    for (var, value, root) in [
        ("LAFT_BUILD_ROOT", "env-root", "env-root"),
        ("XDG_DATA_HOME", "data", "data/laft-build"),
        ("HOME", "home", "home/.local/share/laft-build"),
    ] {
        let sys = format!("sys-{var}");
        let output = Command::new(env!("CARGO_BIN_EXE_laft-build"))
            .env_remove("LAFT_RECIPES")
            .env_remove("LAFT_BUILD_ROOT")
            .env_remove("XDG_DATA_HOME")
            .env(var, value)
            .current_dir(dir.path())
            .args(["--recipes", "repo", "bootstrap", "--root", "host"])
            .args(["--install", &sys, "greet"])
            .output()
            .expect("the binary runs");
        assert!(output.status.success(), "{var}: {output:?}");
        let home = std::fs::read_to_string(dir.path().join(&sys).join("home")).unwrap();
        assert!(
            home.starts_with(&format!("{}/work/", dir.path().join(root).display())),
            "{var}: {home}"
        );
    }
}

#[test]
fn a_stage_dry_run_lists_its_steps_without_touching_the_root() {
    let dir = laft_test::tempdir();
    let file = stage_file(dir.path(), "zlib");
    let root = dir.path().join("system");
    let root = root.to_str().unwrap();
    let output = without_recipes(dir.path(), &["stage", &file, "--root", root, "--dry-run"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        "tools: zlib: pending (0s)\ncheck: script: pending (0s)\n"
    );
    assert!(!Path::new(root).exists());
}

#[test]
fn usage_errors_exit_with_2() {
    let dir = laft_test::tempdir();
    let output = Command::new(env!("CARGO_BIN_EXE_laft-build"))
        .env_remove("LAFT_RECIPES")
        .arg("--root")
        .arg(dir.path())
        .args(["plan"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no recipe repository"));
    let output = laft_build(dir.path(), &["eval", "nonexistent"]);
    assert_eq!(output.status.code(), Some(2));
    let output = laft_build(dir.path(), &["eval", "base:static"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn stage_usage_errors_exit_with_2() {
    let dir = laft_test::tempdir();
    let root = dir.path().join("system");
    let root = root.to_str().unwrap();
    let file = stage_file(dir.path(), "zlib");
    let output = laft_build(dir.path(), &["stage", &file, "--root", root, "--dry-run"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("--recipes"));
    let output = without_recipes(
        dir.path(),
        &[
            "stage",
            &file,
            "--root",
            root,
            "--min-free-gib",
            "17179869184",
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let file = stage_file(dir.path(), "nonexistent");
    let output = without_recipes(dir.path(), &["stage", &file, "--root", root, "--dry-run"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn lint_checks_every_recipe() {
    let dir = laft_test::tempdir();
    let output = laft_build(dir.path(), &["--json", "lint"]);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let results = value["lint"].as_array().unwrap();
    assert!(results.iter().all(|r| r["ok"] == true));
    assert!(results.iter().any(|r| r["package"] == "zlib"));
}

#[test]
fn gc_removes_working_trees() {
    let dir = laft_test::tempdir();
    std::fs::create_dir_all(dir.path().join("work/abc/build")).unwrap();
    let output = laft_build(dir.path(), &["gc"]);
    assert!(output.status.success(), "{output:?}");
    assert!(!dir.path().join("work").exists());
}

#[test]
fn job_counts_and_timeouts_must_be_usable() {
    let dir = laft_test::tempdir();
    for flags in [
        ["--jobs", "0"],
        ["--timeout-hours", "0"],
        ["--timeout-hours", "18446744073709551615"],
    ] {
        let mut args = vec!["shell", "zlib:static"];
        args.extend(flags);
        let output = laft_build(dir.path(), &args);
        assert_eq!(output.status.code(), Some(2), "{flags:?}: {output:?}");
    }
    let output = laft_build(
        dir.path(),
        &["gc", "--sources-older-than", "18446744073709551615"],
    );
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn a_package_without_a_build_for_this_architecture_says_so() {
    let dir = laft_test::tempdir();
    let body = format!(
        "{}  arch = [\"aarch64\"]\n  build custom {{}}",
        support::SOURCE
    );
    let repo = support::write_repo(
        &dir.path().join("repo"),
        "core",
        "",
        &[("recipes/foo/package.stav", &support::recipe("foo", &body))],
    );
    let output = Command::new(env!("CARGO_BIN_EXE_laft-build"))
        .env_remove("LAFT_RECIPES")
        .arg("--root")
        .arg(dir.path().join("root"))
        .arg("--recipes")
        .arg(&repo)
        .args(["shell", "foo"])
        .output()
        .expect("the binary runs");
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("foo builds nothing on this architecture"),
        "{stderr}"
    );
}

#[test]
fn gc_removes_the_outputs_of_reproducibility_checks() {
    let dir = laft_test::tempdir();
    for sub in ["repro/work/k", "repro/out", "repro/logs/zlib"] {
        std::fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    let output = laft_build(dir.path(), &["gc"]);
    assert!(output.status.success(), "{output:?}");
    assert!(!dir.path().join("repro/out").exists());
    assert!(!dir.path().join("repro/logs").exists());
}
