use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

use super::{Invocation, MAX_BYTES, MAX_ENTRIES, USAGE, parse, within_limits};

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

#[test]
fn view_and_command_are_split() {
    let parsed = parse(&args(&["laft-enter", "dev", "make", "-j4"])).unwrap();
    assert_eq!(
        parsed,
        Invocation {
            view: "dev".to_owned(),
            command: args(&["make", "-j4"]),
        }
    );
}

#[test]
fn a_view_alone_runs_the_default_shell() {
    let parsed = parse(&args(&["laft-enter", "dev"])).unwrap();
    assert!(parsed.command.is_empty());
}

#[test]
fn options_are_not_interpreted() {
    let parsed = parse(&args(&["laft-enter", "dev", "--help"])).unwrap();
    assert_eq!(parsed.command, args(&["--help"]));
    assert!(parse(&args(&["laft-enter", "--help"])).is_err());
}

#[test]
fn a_missing_view_prints_the_usage() {
    assert_eq!(parse(&args(&["laft-enter"])), Err(USAGE.to_owned()));
    assert_eq!(parse(&[]), Err(USAGE.to_owned()));
}

#[test]
fn system_is_reserved() {
    let error = parse(&args(&["laft-enter", "system"])).unwrap_err();
    assert!(error.contains("reserved"), "{error}");
}

#[test]
fn names_that_could_leave_the_views_directory_are_rejected() {
    for view in [
        "", ".", "..", "../dev", "dev/..", "/dev", "a/b", "Dev", "dev ", "-dev", "dev.x",
    ] {
        let error = parse(&args(&["laft-enter", view])).unwrap_err();
        assert!(error.starts_with("invalid view name"), "{view:?}: {error}");
    }
    let long = "a".repeat(65);
    assert!(parse(&args(&["laft-enter", &long])).is_err());
}

#[test]
fn names_that_are_not_utf8_are_rejected() {
    let view = OsString::from_vec(vec![b'd', 0xff]);
    let error = parse(&[OsString::from("laft-enter"), view]).unwrap_err();
    assert!(error.starts_with("invalid view name"), "{error}");
}

#[test]
fn package_style_names_are_accepted() {
    for view in ["dev", "py3.12", "rust_nightly", "2026"] {
        assert_eq!(parse(&args(&["laft-enter", view])).unwrap().view, view);
    }
}

#[test]
fn limits_count_entries_and_bytes() {
    let env = |count: usize, value: usize| -> Vec<(OsString, OsString)> {
        (0..count)
            .map(|i| (OsString::from(format!("V{i}")), "x".repeat(value).into()))
            .collect()
    };
    let one = args(&["laft-enter"]);
    assert!(within_limits(&one, &env(MAX_ENTRIES - 1, 1)));
    assert!(!within_limits(&one, &env(MAX_ENTRIES, 1)));

    let big = vec![OsString::from("x".repeat(MAX_BYTES - 1))];
    assert!(within_limits(&big, &[]));
    let too_big = vec![OsString::from("x".repeat(MAX_BYTES))];
    assert!(!within_limits(&too_big, &[]));
    assert!(!within_limits(&one, &env(1, MAX_BYTES)));
}
