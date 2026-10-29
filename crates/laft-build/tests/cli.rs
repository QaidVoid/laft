//! Drives the `laft-build` binary with the example recipes and an empty
//! build root.

mod support;

use std::path::Path;
use std::process::{Command, Output};

fn laft_build(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_laft-build"))
        .env_remove("LAFT_RECIPES")
        .env_remove("LAFT_BUILD_ROOT")
        .arg("--root")
        .arg(root)
        .arg("--recipes")
        .arg(support::core())
        .args(args)
        .output()
        .expect("the binary runs")
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
    assert!(text.contains("## configure\n#!/usr/bin/bash"));
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
