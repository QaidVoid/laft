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
        Some("B044")
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

#[test]
fn license_files_outside_the_source_tree_are_refused() {
    let dir = laft_test::tempdir();
    let (src, out) = (dir.path().join("src"), dir.path().join("out"));
    write_tree(&src, &[("LICENSE", b"x")]);
    write_tree(dir.path(), &[("secret", b"host")]);
    std::os::unix::fs::symlink(dir.path().join("secret"), src.join("COPYING")).unwrap();
    let mut recipe = recipe("zlib");
    for file in [
        dir.path().join("secret").to_string_lossy().into_owned(),
        "../secret".to_owned(),
        "COPYING".to_owned(),
    ] {
        recipe.meta.license_files = Some(vec![file.clone()]);
        assert_eq!(
            install_licenses(&recipe, &src, &out).unwrap_err().code(),
            Some("B044"),
            "{file}"
        );
    }
    assert!(!out.join("usr/share/licenses/zlib/secret").exists());
    assert!(!out.join("usr/share/licenses/zlib/COPYING").exists());
}

#[test]
fn license_files_replace_links_the_install_phase_left() {
    let dir = laft_test::tempdir();
    let (src, out) = (dir.path().join("src"), dir.path().join("out"));
    write_tree(&src, &[("LICENSE", b"mit")]);
    write_tree(dir.path(), &[("host", b"keep")]);
    let licenses = out.join("usr/share/licenses/zlib");
    fs::create_dir_all(&licenses).unwrap();
    std::os::unix::fs::symlink(dir.path().join("host"), licenses.join("LICENSE")).unwrap();
    install_licenses(&recipe("zlib"), &src, &out).unwrap();
    assert_eq!(fs::read(dir.path().join("host")).unwrap(), b"keep");
    assert_eq!(fs::read(licenses.join("LICENSE")).unwrap(), b"mit");

    let out = dir.path().join("linked");
    fs::create_dir_all(out.join("usr/share")).unwrap();
    std::os::unix::fs::symlink(dir.path(), out.join("usr/share/licenses")).unwrap();
    assert_eq!(
        install_licenses(&recipe("zlib"), &src, &out)
            .unwrap_err()
            .code(),
        Some("B044")
    );
    assert!(!dir.path().join("zlib").exists());
}

#[test]
fn an_install_record_holds_a_versioned_build_key() {
    let dir = laft_test::tempdir();
    let item = Item::new("zlib".parse().unwrap(), Variant::Dynamic);
    let key = Blake3Hash::of(b"inputs");
    let record = dir.path().join(INSTALLED).join("zlib+dynamic");
    write_key(&record, &key).unwrap();
    let text = fs::read_to_string(&record).unwrap();
    assert_eq!(text, format!("{{\"format\":1,\"key\":\"{key}\"}}\n"));
    assert_eq!(installed(dir.path(), &item).unwrap(), Some(key));
}

#[test]
fn an_install_record_of_the_first_format_is_a_bare_key_line() {
    let dir = laft_test::tempdir();
    let item = Item::new("zlib".parse().unwrap(), Variant::Dynamic);
    let record = dir.path().join(INSTALLED).join("zlib+dynamic");
    let key = Blake3Hash::of(b"inputs");
    fsx::write_atomic(&record, format!("{key}\n").as_bytes(), 0o644).unwrap();
    assert_eq!(installed(dir.path(), &item).unwrap(), Some(key));
}

#[test]
fn an_install_record_of_another_format_is_refused() {
    let dir = laft_test::tempdir();
    let item = Item::new("zlib".parse().unwrap(), Variant::Dynamic);
    let record = dir.path().join(INSTALLED).join("zlib+dynamic");
    let key = Blake3Hash::of(b"inputs");
    for (text, says) in [
        (format!("{{\"format\":2,\"key\":\"{key}\"}}\n"), "format 2"),
        ("b3:nonsense\n".to_owned(), "not a build key record"),
        ("{\"format\":1,\"key\":\"none\"}\n".to_owned(), "none"),
    ] {
        fsx::write_atomic(&record, text.as_bytes(), 0o644).unwrap();
        let error = installed(dir.path(), &item).unwrap_err();
        assert_eq!(error.code(), Some("B044"), "{text}");
        assert!(error.to_string().contains(says), "{text}: {error}");
    }
}
