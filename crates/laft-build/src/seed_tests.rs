use super::*;
use crate::test_support::TestPkg;

#[test]
fn a_seeded_package_enters_the_store_and_is_recorded_once() {
    let dir = laft_test::tempdir();
    let root = BuildRoot::new(dir.path().join("root"));
    let (file, id) = TestPkg::new("zlib").write(dir.path());
    let entry = seed(&root, &file).unwrap();
    assert!(entry.starts_with(&format!("{id}-zlib-")), "{entry}");
    assert_eq!(root.store().list().unwrap(), std::slice::from_ref(&entry));
    assert_eq!(seeds(&root.seeds()).unwrap(), BTreeSet::from([id]));
    assert_eq!(seed(&root, &file).unwrap(), entry);
    assert_eq!(seeds(&root.seeds()).unwrap(), BTreeSet::from([id]));
}

#[test]
fn seeding_a_package_the_store_has_leaves_it_unmarked() {
    let dir = laft_test::tempdir();
    let root = BuildRoot::new(dir.path().join("root"));
    let (file, id) = TestPkg::new("zlib").write(dir.path());
    root.store().insert(&file, &id, &root.tmp()).unwrap();
    seed(&root, &file).unwrap();
    assert!(seeds(&root.seeds()).unwrap().is_empty());
}

#[test]
fn a_seed_made_again_by_a_build_is_forgotten() {
    let dir = laft_test::tempdir();
    let root = BuildRoot::new(dir.path().join("root"));
    let (zlib, zlib_id) = TestPkg::new("zlib").write(dir.path());
    let (xz, xz_id) = TestPkg::new("xz").write(dir.path());
    seed(&root, &zlib).unwrap();
    seed(&root, &xz).unwrap();
    forget(&root.seeds(), &zlib_id).unwrap();
    forget(&root.seeds(), &zlib_id).unwrap();
    assert_eq!(seeds(&root.seeds()).unwrap(), BTreeSet::from([xz_id]));
}

#[test]
fn a_file_that_is_not_a_package_is_refused() {
    let dir = laft_test::tempdir();
    let root = BuildRoot::new(dir.path().join("root"));
    let file = dir.path().join("junk.lpkg");
    fs::write(&file, b"not a package").unwrap();
    assert!(matches!(seed(&root, &file), Err(BuildError::Package(_))));
    assert!(root.store().list().unwrap().is_empty());
    assert!(seeds(&root.seeds()).unwrap().is_empty());
}

#[test]
fn stray_names_in_the_seed_records_are_ignored() {
    let dir = laft_test::tempdir();
    let records = dir.path().join("seeds");
    fs::create_dir_all(&records).unwrap();
    fs::write(records.join(".abc.123.tmp"), b"").unwrap();
    assert!(seeds(&records).unwrap().is_empty());
    assert!(seeds(&dir.path().join("missing")).unwrap().is_empty());
}
