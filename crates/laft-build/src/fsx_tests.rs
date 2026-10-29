use std::os::unix::fs::symlink;

use super::*;

#[test]
fn symlinks_resolve_inside_the_payload() {
    let dir = laft_test::tempdir();
    let root = dir.path();
    fs::create_dir_all(root.join("usr/lib")).unwrap();
    fs::write(root.join("usr/lib/libfoo.so.1.2"), b"lib").unwrap();
    symlink("libfoo.so.1.2", root.join("usr/lib/libfoo.so.1")).unwrap();
    symlink("/usr/lib/libfoo.so.1", root.join("usr/lib/libfoo.so")).unwrap();
    symlink("../../../escape", root.join("usr/lib/out")).unwrap();
    symlink("loop", root.join("usr/lib/loop")).unwrap();
    assert_eq!(
        resolve(root, "usr/lib/libfoo.so").as_deref(),
        Some("usr/lib/libfoo.so.1.2")
    );
    assert_eq!(
        resolve(root, "usr/lib/libfoo.so.1.2").as_deref(),
        Some("usr/lib/libfoo.so.1.2")
    );
    assert_eq!(resolve(root, "usr/lib/out"), None);
    assert_eq!(resolve(root, "usr/lib/loop"), None);
    assert_eq!(resolve(root, "usr/lib/missing"), None);
}

#[test]
fn absolute_paths_normalize_lexically() {
    assert_eq!(normalize("/usr/lib/app/../lib/./x"), "/usr/lib/lib/x");
    assert_eq!(normalize("/../../usr"), "/usr");
    assert_eq!(normalize("/usr/lib/"), "/usr/lib");
}

#[test]
fn a_tree_with_read_only_directories_is_removed() {
    let dir = laft_test::tempdir();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("a/b")).unwrap();
    fs::write(tree.join("a/b/f"), b"x").unwrap();
    set_mode(&tree.join("a/b"), 0o555).unwrap();
    set_mode(&tree.join("a"), 0o555).unwrap();
    remove_tree(&tree).unwrap();
    assert!(!tree.exists());
    remove_tree(&tree).unwrap();
}

#[test]
fn a_read_only_file_is_replaced_with_its_mode() {
    let dir = laft_test::tempdir();
    let file = dir.path().join("f");
    fs::write(&file, b"old").unwrap();
    set_mode(&file, 0o555).unwrap();
    replace_file(&file, b"new").unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"new");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o7777,
        0o555
    );
}

#[test]
fn files_open_beneath_their_root_only() {
    let dir = laft_test::tempdir();
    let root = dir.path().join("root");
    fs::create_dir_all(root.join("doc")).unwrap();
    fs::write(root.join("doc/LICENSE"), b"x").unwrap();
    fs::write(dir.path().join("secret"), b"s").unwrap();
    symlink("doc/LICENSE", root.join("inside")).unwrap();
    symlink(dir.path().join("secret"), root.join("absolute")).unwrap();
    symlink("../secret", root.join("relative")).unwrap();
    assert!(open_beneath(&root, "doc/LICENSE").is_ok());
    assert!(open_beneath(&root, "inside").is_ok());
    let secret = dir.path().join("secret");
    for rel in [
        secret.to_str().unwrap(),
        "../secret",
        "doc/../../secret",
        "./doc/LICENSE",
        "",
        "absolute",
        "relative",
        "doc",
    ] {
        assert!(open_beneath(&root, rel).is_err(), "{rel}");
    }
}

#[test]
fn a_bare_file_name_lives_in_the_working_directory() {
    assert_eq!(dir_of(Path::new("record")), Path::new("."));
    assert_eq!(dir_of(Path::new("stages/record")), Path::new("stages"));
    assert_eq!(dir_of(Path::new("/build/record")), Path::new("/build"));
}
