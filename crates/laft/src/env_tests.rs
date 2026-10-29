use std::os::unix::fs::symlink;

use super::*;

#[test]
fn names_one_edit_apart_are_similar() {
    assert!(similar("htop", "htpo") || similar("htop", "hto"));
    assert!(similar("ripgrep", "ripgre"));
    assert!(similar("vim", "vin"));
    assert!(!similar("vim", "emacs"));
    assert!(!similar("abc", "abcde"));
}

#[test]
fn generation_links_are_read_by_their_last_component() {
    let dir = laft_test::tempdir();
    let link = dir.path().join("system");
    assert_eq!(link_number(&link).expect("absent is fine"), None);
    symlink("mounts/42", &link).expect("can link");
    assert_eq!(link_number(&link).expect("a number"), Some(42));
    let bad = dir.path().join("bad");
    symlink("mounts/x", &bad).expect("can link");
    assert!(link_number(&bad).is_err());
}
