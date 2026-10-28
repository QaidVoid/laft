use super::*;

#[test]
fn files_without_the_magic_are_not_elf() {
    assert_eq!(inspect(b""), Ok(None));
    assert_eq!(inspect(b"\x7fEL"), Ok(None));
    assert_eq!(inspect(b"#!/bin/sh\nexec true\n"), Ok(None));
}

#[test]
fn truncated_header_is_malformed() {
    assert!(inspect(b"\x7fELF").is_err());
    assert!(inspect(b"\x7fELF\x02\x01\x01").is_err());
}

#[test]
fn unknown_class_is_malformed() {
    let mut header = [0u8; 64];
    header[..4].copy_from_slice(&elf::ELFMAG);
    header[4] = 3;
    header[5] = 1;
    header[6] = 1;
    let error = inspect(&header).expect_err("class 3 does not exist");
    assert_eq!(error.reason(), "unknown ELF class");
}

#[test]
fn search_paths_keep_every_entry_in_order() {
    assert_eq!(
        search_path("$ORIGIN/../lib::/usr/lib"),
        ["$ORIGIN/../lib", "", "/usr/lib"]
    );
}

#[test]
fn needed_entry_must_be_a_soname() {
    assert!(soname("libz.so.1", "DT_NEEDED").is_ok());
    let error = soname("/usr/lib/libz.so.1", "DT_NEEDED").expect_err("a path is not a soname");
    assert!(
        error.reason().starts_with("DT_NEEDED is not a soname: "),
        "{error}"
    );
}
