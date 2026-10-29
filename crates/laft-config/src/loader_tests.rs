use std::os::unix::fs::symlink;

use super::*;

fn rel(path: &str) -> RelPath {
    RelPath::new(path).expect("a relative path")
}

fn setup() -> (laft_test::TempDir, DiskLoader, ImportRoot) {
    let dir = laft_test::tempdir();
    let root = dir.path().join("laft");
    fs::create_dir_all(root.join("lib")).expect("mkdir");
    fs::create_dir_all(root.join("secrets")).expect("mkdir");
    fs::write(root.join("lib/net.stav"), "# net\n").expect("write");
    fs::write(root.join("secrets/alice.stav"), "# secret\n").expect("write");
    fs::write(dir.path().join("outside.stav"), "# outside\n").expect("write");
    let loader = DiskLoader::new(&root);
    (dir, loader, ImportRoot::new(root))
}

#[test]
fn files_inside_the_configuration_directory_are_read() {
    let (_dir, loader, root) = setup();
    assert_eq!(
        loader.read(&root, &rel("lib/net.stav")).as_deref(),
        Ok("# net\n")
    );
    assert_eq!(
        loader.read(&root, &rel("lib/none.stav")),
        Err(LoadError::NotFound)
    );
    assert_eq!(loader.read(&root, &rel("lib")), Err(LoadError::NotAFile));
}

#[test]
fn symbolic_links_cannot_leave_the_configuration_directory() {
    let (dir, loader, root) = setup();
    symlink(
        dir.path().join("outside.stav"),
        root.path().join("lib/out.stav"),
    )
    .expect("symlink");
    symlink("net.stav", root.path().join("lib/in.stav")).expect("symlink");
    assert_eq!(
        loader.read(&root, &rel("lib/out.stav")),
        Err(LoadError::OutsideRoot)
    );
    assert_eq!(
        loader.read(&root, &rel("lib/in.stav")).as_deref(),
        Ok("# net\n")
    );
}

#[test]
fn secrets_are_never_read() {
    let (_dir, loader, root) = setup();
    symlink("../secrets/alice.stav", root.path().join("lib/alias.stav")).expect("symlink");
    for path in ["secrets/alice.stav", "lib/alias.stav"] {
        assert!(
            matches!(loader.read(&root, &rel(path)), Err(LoadError::Other(_))),
            "{path}"
        );
    }
}

#[test]
fn other_import_roots_are_refused() {
    let (dir, loader, _root) = setup();
    let other = ImportRoot::new(dir.path());
    assert_eq!(
        loader.read(&other, &rel("outside.stav")),
        Err(LoadError::OutsideRoot)
    );
}

#[test]
fn the_std_catalog_is_served_from_the_binary() {
    let (_dir, loader, _root) = setup();
    assert!(
        loader
            .std_module("services")
            .is_some_and(|s| s.contains("fn sshd("))
    );
    assert!(
        loader
            .std_module("desktop")
            .is_some_and(|s| s.contains("fn wayland("))
    );
    assert!(loader.std_module("base").is_none());
}
