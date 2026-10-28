use std::fs;
use std::os::unix::fs::symlink;

use super::*;

#[test]
fn symlinks_resolve_inside_the_tree() {
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
