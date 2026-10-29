use std::os::unix::fs::symlink;

use super::*;

#[test]
fn names_one_edit_apart_are_similar() {
    assert!(similar("htop", "hto"));
    assert!(similar("htop", "htoq"));
    assert!(!similar("htop", "htpo"), "a transposition is two edits");
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

#[test]
fn the_esp_comes_from_the_default_generation_s_configuration() {
    let dir = laft_test::tempdir();
    let root = Root::new(dir.path());
    assert_eq!(recorded_esp(&root), None);
    for (number, esp) in [(1, "/efi"), (2, "/boot/efi")] {
        let gen_dir = dir.path().join(format!("laft/gens/{number}"));
        std::fs::create_dir_all(&gen_dir).expect("writable");
        let config = format!("{{\"boot\": {{\"esp\": \"{esp}\"}}}}");
        std::fs::write(gen_dir.join("config.json"), config).expect("writable");
    }
    assert_eq!(
        recorded_esp(&root).as_deref(),
        Some("/boot/efi"),
        "the newest"
    );
    symlink("1", dir.path().join("laft/gens/default")).expect("can link");
    assert_eq!(recorded_esp(&root).as_deref(), Some("/efi"), "the default");
}
