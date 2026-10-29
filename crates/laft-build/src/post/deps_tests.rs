use super::*;
use crate::test_support::{elf_fixture, recipe, write_tree};

fn setup(files: &[(&str, &[u8])]) -> (laft_test::TempDir, ElfFiles) {
    let dir = laft_test::tempdir();
    write_tree(dir.path(), files);
    let elves = crate::post::scan_elf(dir.path(), &mut Vec::new()).unwrap();
    (dir, elves)
}

fn set(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|p| (*p).to_owned()).collect()
}

fn texts<T: ToString>(items: &[T]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

#[test]
fn needs_become_dependencies_unless_the_package_provides_them() {
    let (dir, elves) = setup(&[
        ("usr/lib/libversioned.so.1", &elf_fixture("libversioned.so")),
        ("usr/bin/program", &elf_fixture("program")),
        ("usr/lib/pkgconfig/versioned.pc", b"Name: v\nVersion: 1.2\n"),
        (
            "usr/share/pkgconfig/bad.pc",
            b"Name: b\nVersion: not a version!\n",
        ),
    ]);
    let detected = detect(dir.path(), &elves, Variant::Dynamic, &[], &mut Vec::new()).unwrap();
    let all = set(&[
        "usr/bin/program",
        "usr/lib/libversioned.so.1",
        "usr/lib/pkgconfig/versioned.pc",
        "usr/share/pkgconfig/bad.pc",
    ]);
    let (depends, provides) = detected.relations(&all);
    assert!(texts(&depends).is_empty(), "{depends:?}");
    assert_eq!(
        texts(&provides),
        ["pc:bad", "pc:versioned=1.2", "so:libversioned.so.1"]
    );
    let (depends, provides) = detected.relations(&set(&["usr/bin/program"]));
    assert_eq!(texts(&depends), ["so:libversioned.so.1"]);
    assert!(provides.is_empty());
}

#[test]
fn a_library_without_a_soname_provides_its_file_name() {
    let (dir, elves) = setup(&[
        ("usr/lib/libunnamed.so", &elf_fixture("libunnamed.so")),
        (
            "usr/lib/plugins/libplugin.so",
            &elf_fixture("libunnamed.so"),
        ),
    ]);
    let detected = detect(dir.path(), &elves, Variant::Dynamic, &[], &mut Vec::new()).unwrap();
    let (_, provides) = detected.relations(&set(&[
        "usr/lib/libunnamed.so",
        "usr/lib/plugins/libplugin.so",
    ]));
    assert_eq!(texts(&provides), ["so:libunnamed.so"]);
}

#[test]
fn ignored_paths_and_static_builds_detect_nothing() {
    let (dir, elves) = setup(&[
        ("usr/lib/libversioned.so.1", &elf_fixture("libversioned.so")),
        ("usr/bin/program", &elf_fixture("program")),
    ]);
    let ignore = vec!["usr/bin/**".to_owned()];
    let detected = detect(
        dir.path(),
        &elves,
        Variant::Dynamic,
        &ignore,
        &mut Vec::new(),
    )
    .unwrap();
    assert!(detected.needs.is_empty());
    let detected = detect(dir.path(), &elves, Variant::Static, &[], &mut Vec::new()).unwrap();
    assert!(detected.provides.is_empty());
}

#[test]
fn a_private_library_gives_no_dependency() {
    let (dir, elves) = setup(&[
        (
            "usr/lib/app/lib/libversioned.so.1",
            &elf_fixture("libversioned.so"),
        ),
        ("usr/lib/app/bin/program", &elf_fixture("program")),
    ]);
    let detected = detect(dir.path(), &elves, Variant::Dynamic, &[], &mut Vec::new()).unwrap();
    assert!(detected.needs.is_empty());
    assert!(detected.provides.is_empty());
}

#[test]
fn unknown_interpreters_are_reported() {
    let (dir, elves) = setup(&[
        ("usr/bin/a", b"#!/usr/bin/python3 -u\nprint(1)\n"),
        ("usr/bin/b", b"#!/usr/bin/env bash\necho\n"),
        ("usr/bin/c", b"#!/usr/bin/env lua\n"),
        ("usr/bin/d", b"#!/usr/bin/own\n"),
        ("usr/bin/own", b"#!/usr/bin/sh\n"),
        ("usr/bin/lua-not", b"no shebang"),
    ]);
    let mut warnings = Vec::new();
    detect(dir.path(), &elves, Variant::Dynamic, &[], &mut warnings).unwrap();
    let reported: Vec<&str> = warnings
        .iter()
        .map(|w| {
            assert_eq!(w.code, "W112");
            w.message.split_whitespace().next().unwrap()
        })
        .collect();
    assert_eq!(reported, ["usr/bin/a", "usr/bin/c"]);
}

#[test]
fn imports_are_attributed_to_the_libraries_that_export_them() {
    let (dir, elves) = setup(&[
        ("usr/lib/libversioned.so.1", &elf_fixture("libversioned.so")),
        ("usr/bin/program", &elf_fixture("program")),
    ]);
    let imports = imports(dir.path(), &elves, Path::new("/nonexistent"));
    let program = &elves["usr/bin/program"];
    let expected: BTreeSet<String> = program
        .imports
        .iter()
        .map(|i| match &i.version {
            Some(v) => format!("{}@{}", i.name, v.name),
            None => i.name.clone(),
        })
        .filter(|i| exports(&elves)["libversioned.so.1"].contains(i))
        .collect();
    assert!(!expected.is_empty());
    assert_eq!(imports["libversioned.so.1"], expected);
}

#[test]
fn exports_list_versioned_and_default_names() {
    let (_, elves) = setup(&[("usr/lib/libversioned.so.1", &elf_fixture("libversioned.so"))]);
    let exports = exports(&elves);
    let names = &exports["libversioned.so.1"];
    let library = &elves["usr/lib/libversioned.so.1"];
    for export in &library.exports {
        match &export.version {
            Some(v) => {
                assert!(names.contains(&format!("{}@{}", export.name, v.name)));
                if !v.hidden {
                    assert!(names.contains(&export.name));
                }
            }
            None => assert!(names.contains(&export.name)),
        }
    }
}

#[test]
fn static_deps_belong_to_static_builds_only() {
    let dep = |name: &str| StaticDep {
        name: name.parse().unwrap(),
        version: "1".parse().unwrap(),
        rev: 1,
        id: laft_core::StoreId::from_manifest_hash(&laft_core::Blake3Hash::of(name.as_bytes())),
    };
    let view = [dep("zlib"), dep("pcre2"), dep("zlib")];
    let names: Vec<String> = static_deps(Variant::Static, &view)
        .iter()
        .map(|d| d.name.to_string())
        .collect();
    assert_eq!(names, ["pcre2", "zlib"]);
    assert!(static_deps(Variant::Dynamic, &view).is_empty());
}

#[test]
fn payload_paths_activate_triggers() {
    let paths = set(&[
        "usr/share/fonts/ttf/a.ttf",
        "usr/share/applications/x.desktop",
        "usr/bin/x",
    ]);
    let names: Vec<String> = triggers(&paths).iter().map(|n| n.to_string()).collect();
    assert_eq!(names, ["desktop", "fontconfig"]);
}

#[test]
fn alternative_paths_must_be_in_their_package() {
    let mut recipe = recipe("vi");
    recipe.outputs.alternatives.insert(
        "editor".into(),
        crate::Alternative {
            paths: vec!["usr/bin/vi".into()],
            priority: 50,
            subpackage: None,
        },
    );
    let main = Part {
        name: "vi".parse().unwrap(),
        suffix: None,
        paths: set(&["usr/bin/vi"]),
    };
    let groups = alternatives(&recipe, std::slice::from_ref(&main)).unwrap();
    assert_eq!(groups[&main.name][0].paths, ["usr/bin/vi"]);
    assert_eq!(groups[&main.name][0].priority, 50);
    let empty = Part {
        paths: BTreeSet::new(),
        ..main
    };
    assert_eq!(
        alternatives(&recipe, &[empty]).unwrap_err().code(),
        Some("R040")
    );
}
