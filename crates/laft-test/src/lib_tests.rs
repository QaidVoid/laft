use std::os::unix::fs::symlink;

use super::*;

#[test]
fn a_read_only_tree_is_removed_on_drop() {
    let dir = tempdir();
    let deep = dir.path().join("laft/store/abc/usr/bin");
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("hello"), b"hi").unwrap();
    for path in [
        "laft/store/abc/usr/bin",
        "laft/store/abc/usr",
        "laft/store/abc",
        "laft/store",
    ] {
        fs::set_permissions(dir.path().join(path), fs::Permissions::from_mode(0o555)).unwrap();
    }
    let path = dir.path().to_path_buf();
    drop(dir);
    assert!(!path.exists());
}

#[test]
fn symlinks_are_not_followed() {
    let outside = tempdir();
    fs::set_permissions(outside.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let dir = tempdir();
    symlink(outside.path(), dir.path().join("link")).unwrap();
    make_writable(dir.path()).unwrap();
    let mode = fs::metadata(outside.path()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o555);
}
