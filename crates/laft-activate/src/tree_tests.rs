use std::fs;
use std::os::unix::fs::symlink;

use super::*;

#[test]
fn symlinks_resolve_inside_the_tree() {
    let dir = laft_test::tempdir();
    let tree = dir.path();
    fs::create_dir_all(tree.join("usr/lib/firmware/vendor")).unwrap();
    fs::write(tree.join("usr/lib/firmware/vendor/a.bin"), b"a").unwrap();
    symlink(
        "/usr/lib/firmware/vendor/a.bin",
        tree.join("usr/lib/firmware/abs.bin"),
    )
    .unwrap();
    symlink("vendor/a.bin", tree.join("usr/lib/firmware/rel.bin")).unwrap();
    symlink(
        "../../../../../../../usr/lib/firmware/vendor/a.bin",
        tree.join("usr/lib/firmware/up.bin"),
    )
    .unwrap();
    symlink("/etc/passwd", tree.join("usr/lib/firmware/host.bin")).unwrap();
    symlink("loop.bin", tree.join("usr/lib/firmware/loop.bin")).unwrap();

    let expected = Some(tree.join("usr/lib/firmware/vendor/a.bin"));
    for link in ["abs.bin", "rel.bin", "up.bin", "vendor/a.bin"] {
        let found = resolve_file(tree, &format!("usr/lib/firmware/{link}")).unwrap();
        assert_eq!(found, expected, "{link}");
    }
    for missing in ["host.bin", "loop.bin", "none.bin", "vendor"] {
        let found = resolve_file(tree, &format!("usr/lib/firmware/{missing}")).unwrap();
        assert_eq!(found, None, "{missing}");
    }
}

#[test]
fn walk_lists_parents_first_without_following_links() {
    let dir = laft_test::tempdir();
    let tree = dir.path();
    fs::create_dir_all(tree.join("usr/b/c")).unwrap();
    fs::write(tree.join("usr/b/c/f"), b"").unwrap();
    fs::write(tree.join("usr/a"), b"").unwrap();
    symlink("/", tree.join("usr/link")).unwrap();
    assert_eq!(
        walk(tree, "usr").unwrap(),
        ["usr/a", "usr/b", "usr/b/c", "usr/b/c/f", "usr/link"]
    );
    assert!(walk(tree, "usr/missing").unwrap().is_empty());
}

#[test]
fn walk_and_subdirs_never_enter_a_symlinked_directory() {
    let dir = laft_test::tempdir();
    let outside = dir.path().join("outside");
    fs::create_dir_all(outside.join("info/sub")).unwrap();
    fs::write(outside.join("info/host"), b"").unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("usr")).unwrap();
    symlink(&outside, tree.join("usr/share")).unwrap();
    symlink(outside.join("info"), tree.join("usr/info")).unwrap();

    assert!(walk(&tree, "usr/share/info").unwrap().is_empty());
    assert!(walk(&tree, "usr/info").unwrap().is_empty());
    assert!(subdirs(&tree, "usr/share/info").unwrap().is_empty());
    assert!(subdirs(&tree, "usr/info").unwrap().is_empty());
}

#[test]
fn walk_refuses_paths_that_are_not_relative_and_normalized() {
    let dir = laft_test::tempdir();
    fs::create_dir_all(dir.path().join("usr")).unwrap();
    for rel in ["", "/usr", "usr/../usr", "./usr", "usr//x"] {
        assert!(walk(dir.path(), rel).is_err(), "{rel:?}");
        assert!(subdirs(dir.path(), rel).is_err(), "{rel:?}");
    }
}
