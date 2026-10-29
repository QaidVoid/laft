use super::*;
use crate::test_support::{elf_fixture, link, write_tree};

/// The library fixture with its search path replaced by `value`, which is
/// at most as long as the original.
fn library_searching(value: &str) -> Vec<u8> {
    let old = b"/opt/legacy/lib";
    let mut data = elf_fixture("libversioned.so");
    let at = data.windows(old.len()).position(|w| w == old).unwrap();
    let mut new = value.as_bytes().to_vec();
    new.resize(old.len(), 0);
    data[at..at + old.len()].copy_from_slice(&new);
    data
}

fn scan(
    files: &[(&str, &[u8])],
) -> (
    laft_test::TempDir,
    Result<ElfFiles, BuildError>,
    Vec<Warning>,
) {
    let dir = laft_test::tempdir();
    write_tree(dir.path(), files);
    let mut warnings = Vec::new();
    let result = scan_elf(dir.path(), &mut warnings);
    (dir, result, warnings)
}

#[test]
fn rpath_is_rewritten_to_runpath_in_the_payload() {
    let (dir, result, warnings) = scan(&[
        ("usr/lib/libversioned.so.1", &elf_fixture("libversioned.so")),
        ("usr/share/doc/README", b"text"),
    ]);
    let elves = result.unwrap();
    assert_eq!(
        elves.keys().collect::<Vec<_>>(),
        ["usr/lib/libversioned.so.1"]
    );
    let lib = &elves["usr/lib/libversioned.so.1"];
    assert_eq!(lib.runpath, ["/opt/legacy/lib"]);
    assert!(lib.rpath.is_empty());
    let on_disk = read_elf(&dir.path().join("usr/lib/libversioned.so.1"))
        .unwrap()
        .unwrap();
    assert_eq!(on_disk.runpath, ["/opt/legacy/lib"]);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, "W111");
}

#[test]
fn a_search_path_of_only_usr_lib_is_dropped_silently() {
    let (_, result, warnings) = scan(&[("usr/lib/libv.so.1", &library_searching("/usr/lib"))]);
    let elves = result.unwrap();
    assert!(elves["usr/lib/libv.so.1"].runpath.is_empty());
    assert!(warnings.is_empty());
}

#[test]
fn a_search_path_into_the_build_fails() {
    for path in ["/build/obj/lib", "/out/usr/lib"] {
        let (_, result, _) = scan(&[("usr/lib/libv.so.1", &library_searching(path))]);
        assert_eq!(result.unwrap_err().code(), Some("B107"), "{path}");
    }
}

#[test]
fn foreign_machines_fail_except_firmware() {
    let program32 = elf_fixture("program32");
    let (_, result, _) = scan(&[("usr/bin/x", &program32)]);
    assert_eq!(result.unwrap_err().code(), Some("B108"));
    let (_, result, _) = scan(&[("usr/lib/firmware/dev/x", &program32)]);
    assert!(result.is_ok());
}

#[test]
fn malformed_elf_files_are_skipped_with_a_warning() {
    let truncated = elf_fixture("program")[..100].to_vec();
    let (_, result, warnings) = scan(&[("usr/bin/broken", &truncated)]);
    assert!(result.unwrap().is_empty());
    assert_eq!(warnings[0].code, "W115");
}

#[test]
fn a_missing_soname_link_is_created() {
    let (dir, result, _) = scan(&[(
        "usr/lib/libversioned.so.1.2.3",
        &elf_fixture("libversioned.so"),
    )]);
    soname_links(dir.path(), &result.unwrap()).unwrap();
    assert_eq!(
        fs::read_link(dir.path().join("usr/lib/libversioned.so.1")).unwrap(),
        Path::new("libversioned.so.1.2.3")
    );
}

#[test]
fn a_soname_link_to_another_file_fails() {
    let library = elf_fixture("libversioned.so");
    let (dir, result, _) = scan(&[
        ("usr/lib/libversioned.so.1.2", &library),
        ("usr/lib/libversioned.so.1.3", &library),
    ]);
    link(
        dir.path(),
        "usr/lib/libversioned.so.1",
        "libversioned.so.1.2",
    );
    let error = soname_links(dir.path(), &result.unwrap()).unwrap_err();
    assert_eq!(error.code(), Some("B112"));
    assert!(
        error.to_string().contains(
            "usr/lib/libversioned.so.1 points to usr/lib/libversioned.so.1.2 but the SONAME of usr/lib/libversioned.so.1.3"
        ),
        "{error}"
    );
}

#[test]
fn a_private_library_must_be_reachable_through_runpath() {
    let (library, program) = (elf_fixture("libversioned.so"), elf_fixture("program"));
    let (_, result, _) = scan(&[
        ("usr/lib/app/libversioned.so.1", &library),
        ("usr/bin/program", &program),
    ]);
    let error = check_private_libraries(&result.unwrap()).unwrap_err();
    assert_eq!(error.code(), Some("B113"));
    assert!(
        error
            .to_string()
            .contains("usr/bin/program needs libversioned.so.1")
    );

    let (_, result, _) = scan(&[
        ("usr/lib/app/lib/libversioned.so.1", &library),
        ("usr/lib/app/bin/program", &program),
    ]);
    check_private_libraries(&result.unwrap()).expect("$ORIGIN/../lib reaches the library");
}

#[test]
fn needs_nothing_provides_are_reported_when_the_view_is_known() {
    let (_, result, _) = scan(&[("usr/bin/program", &elf_fixture("program"))]);
    let elves = result.unwrap();
    let mut warnings = Vec::new();
    check_needs(&elves, None, &mut warnings);
    assert!(warnings.is_empty());
    check_needs(&elves, Some(&BTreeSet::new()), &mut warnings);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, "W114");
    let view = BTreeSet::from(["libversioned.so.1".parse().unwrap()]);
    let mut warnings = Vec::new();
    check_needs(&elves, Some(&view), &mut warnings);
    assert!(warnings.is_empty());
}

#[test]
fn abi_digests_cover_libraries_directly_in_usr_lib() {
    let library = elf_fixture("libversioned.so");
    let (_, result, _) = scan(&[
        ("usr/lib/libversioned.so.1", &library),
        ("usr/lib/plugins/libversioned.so.1", &library),
        ("usr/bin/program", &elf_fixture("program")),
    ]);
    let digests = abi_digests(&result.unwrap());
    assert_eq!(
        digests.keys().collect::<Vec<_>>(),
        ["usr/lib/libversioned.so.1"]
    );
    let abi = &digests["usr/lib/libversioned.so.1"];
    assert_eq!(abi.soname.as_str(), "libversioned.so.1");
    let expected = laft_elf::inspect(&library).unwrap().unwrap().abi_digest();
    assert_eq!(abi.digest, expected);
}
