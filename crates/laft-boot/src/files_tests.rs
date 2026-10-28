use std::fs;
use std::os::unix::fs::symlink;

use super::{TOP_LEVEL_LINKS, check_layout, clear, replace_symlink, write_atomic};

#[test]
fn atomic_write_replaces_the_content_and_leaves_no_temporary_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("boot.json");
    fs::write(&path, "old").unwrap();
    write_atomic(&path, b"new").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
}

#[test]
fn symlink_is_replaced_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let link = tmp.path().join("default");
    symlink("42", &link).unwrap();
    replace_symlink(&link, "41", &tmp.path().join("default.tmp")).unwrap();
    assert_eq!(fs::read_link(&link).unwrap().to_str(), Some("41"));
    assert!(!tmp.path().join("default.tmp").exists());
}

#[test]
fn complete_layout_passes_the_check() {
    let tmp = tempfile::tempdir().unwrap();
    for (name, target) in TOP_LEVEL_LINKS {
        symlink(target, tmp.path().join(name)).unwrap();
    }
    assert_eq!(check_layout(tmp.path()), Ok(()));
}

#[test]
fn missing_or_wrong_links_fail_the_check() {
    let tmp = tempfile::tempdir().unwrap();
    for (name, target) in &TOP_LEVEL_LINKS[1..4] {
        symlink(target, tmp.path().join(name)).unwrap();
    }
    fs::create_dir(tmp.path().join("usr")).unwrap();
    let err = check_layout(tmp.path()).unwrap_err();
    assert!(
        err.contains("/usr -> /run/laft/system/usr, /lib64 -> usr/lib"),
        "{err}"
    );
}

#[test]
fn clearing_removes_everything_but_the_kept_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("lib/modules/6.17.5-laft1")).unwrap();
    fs::write(root.join("lib/modules/6.17.5-laft1/nvme.ko"), "ko").unwrap();
    fs::write(root.join("init"), "elf").unwrap();
    symlink("busybox", root.join("sh")).unwrap();
    fs::create_dir_all(root.join("newroot/laft")).unwrap();
    fs::write(root.join("newroot/laft/keep"), "keep").unwrap();

    clear(root, &root.join("newroot"));

    let left: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["newroot"]);
    assert!(root.join("newroot/laft/keep").exists());
}
