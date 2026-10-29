use super::*;
use crate::inspect;

const LIBRARY: &[u8] = include_bytes!("../tests/fixtures/libversioned.so");
const PROGRAM: &[u8] = include_bytes!("../tests/fixtures/program");

/// The library fixture with its `DT_RPATH` value replaced by `value`, which
/// must not be longer than the original.
fn library_with_rpath(value: &str) -> Vec<u8> {
    let old = b"/opt/legacy/lib";
    let mut data = LIBRARY.to_vec();
    let at = data
        .windows(old.len())
        .position(|w| w == old)
        .expect("the fixture's rpath is in its string table");
    let mut replacement = value.as_bytes().to_vec();
    replacement.resize(old.len(), 0);
    data[at..at + old.len()].copy_from_slice(&replacement);
    data
}

#[test]
fn rpath_becomes_runpath_and_nothing_else_changes() {
    let before = inspect(LIBRARY).unwrap().unwrap();
    let mut data = LIBRARY.to_vec();
    assert_eq!(normalize_search_paths(&mut data), Ok(true));
    let after = inspect(&data).unwrap().unwrap();
    assert_eq!(after.runpath, ["/opt/legacy/lib"]);
    assert!(after.rpath.is_empty());
    assert_eq!(after.soname, before.soname);
    assert_eq!(after.exports, before.exports);
    assert_eq!(after.imports, before.imports);
    assert_eq!(data.len(), LIBRARY.len());
}

#[test]
fn rewriting_twice_changes_nothing_the_second_time() {
    let mut data = LIBRARY.to_vec();
    normalize_search_paths(&mut data).unwrap();
    let once = data.clone();
    assert_eq!(normalize_search_paths(&mut data), Ok(false));
    assert_eq!(data, once);
}

#[test]
fn a_search_path_of_only_usr_lib_is_removed() {
    let mut data = library_with_rpath("/usr/lib");
    assert_eq!(normalize_search_paths(&mut data), Ok(true));
    let after = inspect(&data).unwrap().unwrap();
    assert!(after.rpath.is_empty() && after.runpath.is_empty());
    assert_eq!(after.soname, inspect(LIBRARY).unwrap().unwrap().soname);
}

#[test]
fn a_runpath_that_lists_more_than_usr_lib_is_kept() {
    let mut data = PROGRAM.to_vec();
    assert_eq!(normalize_search_paths(&mut data), Ok(false));
    assert_eq!(data, PROGRAM);
}

#[test]
fn files_that_are_not_elf_are_left_alone() {
    let mut data = b"#!/bin/sh\n".to_vec();
    assert_eq!(normalize_search_paths(&mut data), Ok(false));
}

#[test]
fn a_malformed_file_is_rejected_unchanged() {
    let mut data = LIBRARY[..80].to_vec();
    let before = data.clone();
    assert!(normalize_search_paths(&mut data).is_err());
    assert_eq!(data, before);
}
