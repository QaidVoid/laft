use super::*;
use crate::test_support::{mode, recipe, write_tree};

#[test]
fn top_level_license_files_are_installed_by_default() {
    let dir = laft_test::tempdir();
    let (src, out) = (dir.path().join("src"), dir.path().join("out"));
    write_tree(
        &src,
        &[
            ("COPYING", b"gpl"),
            ("LICENSE.md", b"mit"),
            ("README", b"r"),
            ("doc/LICENSE", b"x"),
        ],
    );
    install_licenses(&recipe("zlib"), &src, &out).unwrap();
    let dir = out.join("usr/share/licenses/zlib");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["COPYING", "LICENSE.md"]);
    assert_eq!(mode(&dir.join("COPYING")), 0o644);
}

#[test]
fn listed_license_files_must_exist() {
    let dir = laft_test::tempdir();
    let (src, out) = (dir.path().join("src"), dir.path().join("out"));
    write_tree(&src, &[("doc/LICENSE", b"x")]);
    let mut recipe = recipe("zlib");
    recipe.meta.license_files = Some(vec!["doc/LICENSE".into()]);
    install_licenses(&recipe, &src, &out).unwrap();
    assert!(out.join("usr/share/licenses/zlib/LICENSE").is_file());
    recipe.meta.license_files = Some(vec!["missing".into()]);
    assert_eq!(
        install_licenses(&recipe, &src, &out).unwrap_err().code(),
        Some("B023")
    );
}

#[test]
fn cross_files_point_at_the_musl_toolchain() {
    let dir = laft_test::tempdir();
    write_cross_files(dir.path()).unwrap();
    let meson = fs::read_to_string(dir.path().join("meson.ini")).unwrap();
    assert!(meson.contains("c = 'x86_64-laft-linux-musl-gcc'"));
    assert!(meson.contains("sys_root = '/usr/x86_64-laft-linux-musl'"));
    let cmake = fs::read_to_string(dir.path().join("toolchain.cmake")).unwrap();
    assert!(cmake.contains("set(CMAKE_SYSROOT /usr/x86_64-laft-linux-musl)"));
}

#[test]
fn recipe_trees_become_read_only() {
    let dir = laft_test::tempdir();
    write_tree(dir.path(), &[("core/zlib/fix.patch", b"p")]);
    read_only(dir.path()).unwrap();
    assert_eq!(mode(&dir.path().join("core/zlib/fix.patch")), 0o444);
    assert_eq!(mode(&dir.path().join("core/zlib")), 0o555);
}
