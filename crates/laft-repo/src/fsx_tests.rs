use super::*;

#[test]
fn atomic_write_replaces_the_file_and_leaves_no_temporary_file() {
    let dir = laft_test::tempdir();
    let path = dir.path().join("a/b/index.json.zst");
    write_atomic(&path, b"old", FILE_MODE).unwrap();
    write_atomic(&path, b"new", FILE_MODE).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(list(&dir.path().join("a/b")).unwrap(), ["index.json.zst"]);
}

#[test]
fn copy_keeps_the_content_and_hash() {
    let dir = laft_test::tempdir();
    let from = dir.path().join("from");
    fs::write(&from, b"payload").unwrap();
    let to = dir.path().join("pool/x.lpkg");
    copy_atomic(&from, &to).unwrap();
    assert_eq!(hash_file(&to).unwrap(), (7, Blake3Hash::of(b"payload")));
}

#[test]
fn missing_files_are_reported_as_absent() {
    let dir = laft_test::tempdir();
    let path = dir.path().join("missing");
    assert_eq!(read_optional(&path).unwrap(), None);
    assert_eq!(size(&path).unwrap(), None);
    assert!(!remove(&path).unwrap());
    assert!(list(&path).unwrap().is_empty());
}
