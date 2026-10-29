use std::os::unix::fs::PermissionsExt;

use super::*;

#[test]
fn a_replaced_file_keeps_its_mode_and_leaves_no_temporary_file() {
    let dir = laft_test::tempdir();
    let path = dir.path().join("system.stav");
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    fs::write(dir.path().join("system"), "other").unwrap();
    replace(&path, b"new").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("system")).unwrap(),
        "other"
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn a_failed_replacement_leaves_the_file_and_no_temporary_file() {
    let dir = laft_test::tempdir();
    assert!(replace(&dir.path().join("missing/x"), b"new").is_err());
    let blocked = dir.path().join("dir");
    fs::create_dir(&blocked).unwrap();
    fs::write(blocked.join("inside"), "").unwrap();
    assert!(replace(&blocked, b"new").is_err());
    assert!(blocked.is_dir());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn a_new_file_never_replaces_an_existing_one() {
    let dir = laft_test::tempdir();
    let path = dir.path().join("etc/laft/hardware.stav");
    create_new(&path, b"first").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "first");
    let error = create_new(&path, b"second").unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(&path).unwrap(), "first");
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}
