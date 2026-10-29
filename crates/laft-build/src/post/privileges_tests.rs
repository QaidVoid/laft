use std::collections::BTreeMap;

use super::*;
use crate::test_support::{mode, recipe, write_tree};

fn outputs() -> Outputs {
    recipe("x").outputs
}

#[test]
fn files_lose_write_bits_and_directories_get_0755() {
    let dir = laft_test::tempdir();
    write_tree(
        dir.path(),
        &[("usr/bin/x", b"x"), ("usr/share/x/data", b"d")],
    );
    fsx::set_mode(&dir.path().join("usr/bin/x"), 0o775).unwrap();
    fsx::set_mode(&dir.path().join("usr/share/x/data"), 0o666).unwrap();
    fsx::set_mode(&dir.path().join("usr/share"), 0o700).unwrap();
    let result = privileges(dir.path(), &outputs()).unwrap();
    assert_eq!(mode(&dir.path().join("usr/bin/x")), 0o555);
    assert_eq!(mode(&dir.path().join("usr/share/x/data")), 0o444);
    assert_eq!(mode(&dir.path().join("usr/share")), 0o755);
    assert_eq!(result.world_writable, ["usr/share/x/data"]);
    assert!(result.requested.is_empty());
}

#[test]
fn an_undeclared_setuid_file_fails() {
    let dir = laft_test::tempdir();
    write_tree(dir.path(), &[("usr/bin/su", b"su")]);
    fsx::set_mode(&dir.path().join("usr/bin/su"), 0o4755).unwrap();
    let error = privileges(dir.path(), &outputs()).expect_err("su is not declared");
    assert_eq!(error.code(), Some("B106"));
    assert!(
        error
            .to_string()
            .contains("usr/bin/su is setuid but not declared in setuid")
    );
}

#[test]
fn declared_privileges_are_requested_and_cleared() {
    let dir = laft_test::tempdir();
    write_tree(
        dir.path(),
        &[("usr/bin/doas", b"d"), ("usr/bin/ping", b"p")],
    );
    fsx::set_mode(&dir.path().join("usr/bin/doas"), 0o4755).unwrap();
    fsx::set_mode(&dir.path().join("usr/bin/ping"), 0o755).unwrap();
    let mut outputs = outputs();
    outputs.setuid = vec!["usr/bin/doas".into()];
    outputs.caps = BTreeMap::from([("usr/bin/ping".into(), "cap_net_raw=ep".into())]);
    let result = privileges(dir.path(), &outputs).unwrap();
    let requested: Vec<(&str, u32, Option<&str>)> = result
        .requested
        .iter()
        .map(|r| (r.path.as_str(), r.mode, r.caps.as_deref()))
        .collect();
    assert_eq!(
        requested,
        [
            ("usr/bin/doas", 0o4755, None),
            ("usr/bin/ping", 0o755, Some("cap_net_raw=ep")),
        ]
    );
    assert_eq!(mode(&dir.path().join("usr/bin/doas")), 0o555);
}

#[test]
fn a_requested_privilege_needs_a_regular_file() {
    let dir = laft_test::tempdir();
    write_tree(dir.path(), &[("usr/bin/x", b"x")]);
    let mut outputs = outputs();
    outputs.setuid = vec!["usr/bin/missing".into()];
    assert_eq!(
        privileges(dir.path(), &outputs).unwrap_err().code(),
        Some("B111")
    );
}

#[test]
fn ownership_names_static_accounts() {
    let dir = laft_test::tempdir();
    write_tree(
        dir.path(),
        &[("etc/dhcpcd/conf", b"c"), ("usr/bin/x", b"x")],
    );
    let mut outputs = outputs();
    outputs.ownership = BTreeMap::from([("etc/dhcpcd".into(), "dhcpcd:dhcpcd".into())]);
    let result = privileges(dir.path(), &outputs).unwrap();
    assert_eq!(
        result.owner("etc/dhcpcd/conf"),
        (Some("dhcpcd".into()), Some("dhcpcd".into()))
    );
    assert_eq!(result.owner("usr/bin/x"), (None, None));

    outputs.ownership = BTreeMap::from([("etc/**".into(), "alice:users".into())]);
    assert_eq!(
        privileges(dir.path(), &outputs).unwrap_err().code(),
        Some("B114")
    );
}
