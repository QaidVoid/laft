//! The `build` block: the local build settings of a configuration.

mod support;

use laft_config::{DepOptions, Missing};
use support::{DISKS, config_dir};

/// Loads a configuration whose system body is `body`.
fn load(body: &str) -> laft_config::SystemConfig {
    let dir = config_dir(&[("system.stav", &format!("system {{\n{DISKS}\n{body}\n}}\n"))]);
    support::load(dir.path())
        .expect("the configuration is valid")
        .config
}

#[test]
fn the_defaults_are_fail_context_and_automatic_budgets() {
    let config = load("");
    assert_eq!(config.build.missing, Missing::Fail);
    assert_eq!(config.build.dep_options, DepOptions::Context);
    assert_eq!((config.build.jobs, config.build.workers), (0, 0));
}

#[test]
fn the_settings_convert() {
    let config = load("build { missing = build, dep_options = defaults, jobs = 8, workers = 2 }");
    assert_eq!(config.build.missing, Missing::Build);
    assert_eq!(config.build.dep_options, DepOptions::Defaults);
    assert_eq!((config.build.jobs, config.build.workers), (8, 2));
}

#[test]
fn a_budget_above_the_limit_is_a_type_error() {
    let dir = config_dir(&[(
        "system.stav",
        &format!("system {{\n{DISKS}\n  build {{ jobs = 1025 }}\n}}\n"),
    )]);
    let report = support::load(dir.path()).expect_err("1025 jobs are too many");
    assert_eq!(support::codes(&report), ["E0803"]);
}

#[test]
fn the_block_is_an_attribute_of_its_own() {
    let config = load("build.jobs = 4");
    assert_eq!(config.build.jobs, 4);
}
