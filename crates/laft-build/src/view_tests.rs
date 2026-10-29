use super::*;
use crate::test_support::{TestPkg, recipe, refs};

fn none() -> BTreeSet<StoreId> {
    BTreeSet::new()
}

fn names(refs: &[PkgRef]) -> Vec<String> {
    refs.iter()
        .map(ToString::to_string)
        .filter(|r| !BUILD_BASE.contains(&r.as_str()))
        .collect()
}

#[test]
fn a_dynamic_view_requests_build_base_tools_and_dev_packages() {
    let mut recipe = recipe("app");
    recipe.build_deps = refs(&["meson", "pkgconf"]);
    recipe.link_deps = refs(&["zlib", "pcre2@10"]);
    recipe.check_deps = refs(&["python"]);
    let available = ["zlib-dev".parse().unwrap()].into_iter().collect();
    let requested = requests(&recipe, false, &available);
    for base in BUILD_BASE {
        assert!(requested.iter().any(|r| r.to_string() == *base), "{base}");
    }
    assert_eq!(names(&requested), ["meson", "zlib", "zlib-dev", "pcre2@10"]);
    assert_eq!(
        names(&requests(&recipe, true, &available)),
        ["meson", "zlib", "zlib-dev", "pcre2@10", "python"]
    );
}

#[test]
fn a_static_view_requests_static_libraries_and_the_cross_toolchain() {
    let mut recipe = recipe("app:static");
    recipe.build_deps = refs(&["meson"]);
    recipe.link_deps = refs(&["zlib:static"]);
    let available = ["zlib-dev".parse().unwrap()].into_iter().collect();
    assert_eq!(
        names(&requests(&recipe, false, &available)),
        ["meson", "zlib:static", "toolchain-musl"]
    );
}

fn store_with_base(dir: &Path) -> Store {
    let store = Store::at(dir.join("store"));
    for name in BUILD_BASE {
        TestPkg::new(name).add(&store, dir);
    }
    store
}

#[test]
fn the_view_holds_the_closure_of_the_requests() {
    let dir = laft_test::tempdir();
    let store = store_with_base(dir.path());
    TestPkg {
        files: &[("usr/lib/libz.so.1", b"z")],
        provides: &["so:libz.so.1"],
        ..TestPkg::new("zlib")
    }
    .add(&store, dir.path());
    TestPkg {
        files: &[("usr/include/zlib.h", b"h")],
        depends: &["zlib"],
        ..TestPkg::new("zlib-dev")
    }
    .add(&store, dir.path());
    TestPkg::new("unrelated").add(&store, dir.path());
    let mut recipe = recipe("app");
    recipe.link_deps = refs(&["zlib"]);
    let view = assemble(
        &store,
        &none(),
        &recipe,
        false,
        &dir.path().join("view"),
        None,
    )
    .unwrap();
    assert!(view.usr.join("lib/libz.so.1").is_file());
    assert!(view.usr.join("include/zlib.h").is_file());
    assert!(view.usr.join("share/dash/file").is_file());
    assert!(!view.usr.join("share/unrelated").exists());
    let names: Vec<&str> = view.packages.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"zlib-dev") && !names.contains(&"unrelated"));
    assert_eq!(
        view.sonames.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        ["libz.so.1"]
    );
    assert!(view.statics.is_empty());
}

#[test]
fn a_static_view_records_the_static_libraries_it_links() {
    let dir = laft_test::tempdir();
    let store = store_with_base(dir.path());
    TestPkg::new("toolchain-musl").add(&store, dir.path());
    let pcre = TestPkg {
        variant: Variant::Static,
        depends: &["zlib"],
        ..TestPkg::new("pcre2")
    }
    .add(&store, dir.path());
    let zlib = TestPkg {
        variant: Variant::Static,
        ..TestPkg::new("zlib")
    }
    .add(&store, dir.path());
    let mut recipe = recipe("rg:static");
    recipe.link_deps = refs(&["pcre2:static"]);
    let view = assemble(
        &store,
        &none(),
        &recipe,
        false,
        &dir.path().join("view"),
        None,
    )
    .unwrap();
    let statics: Vec<(&str, StoreId)> = view
        .statics
        .iter()
        .map(|s| (s.name.as_str(), s.id))
        .collect();
    assert_eq!(statics, [("pcre2", pcre), ("zlib", zlib)]);
}

#[test]
fn a_view_the_store_cannot_satisfy_fails() {
    let dir = laft_test::tempdir();
    let store = Store::at(dir.path().join("store"));
    TestPkg::new("dash").add(&store, dir.path());
    let error = assemble(
        &store,
        &none(),
        &recipe("app"),
        false,
        &dir.path().join("view"),
        None,
    )
    .unwrap_err();
    assert_eq!(error.code(), Some("B050"));
}

#[test]
fn a_smoke_view_holds_the_package_and_its_runtime_closure() {
    let dir = laft_test::tempdir();
    let store = Store::at(dir.path().join("store"));
    for name in ["dash", "coreutils", "zlib", "gcc"] {
        TestPkg::new(name).add(&store, dir.path());
    }
    TestPkg {
        depends: &["zlib"],
        ..TestPkg::new("app")
    }
    .add(&store, dir.path());
    let entries = store_entries(&store).unwrap();
    let app = entries
        .iter()
        .find(|e| e.meta.name.as_str() == "app")
        .unwrap();
    let usr = assemble_smoke(&store, &none(), app, &dir.path().join("smoke")).unwrap();
    for present in ["app", "zlib", "dash", "coreutils"] {
        assert!(usr.join("share").join(present).exists(), "{present}");
    }
    assert!(!usr.join("share/gcc").exists());
}

#[test]
fn key_inputs_take_link_dependencies_from_the_store() {
    let dir = laft_test::tempdir();
    let store = Store::at(dir.path().join("store"));
    TestPkg {
        provides: &["so:libz.so.1"],
        ..TestPkg::new("zlib")
    }
    .add(&store, dir.path());
    let mut recipe = recipe("app");
    recipe.link_deps = refs(&["zlib", "missing"]);
    let inputs = key_inputs(&recipe, &store, &none()).unwrap();
    assert_eq!(inputs.links.len(), 1);
    assert_eq!(inputs.links[0].abi_epoch, 2);
    assert_eq!(
        inputs.links[0].sonames.iter().collect::<Vec<_>>(),
        ["libz.so.1"]
    );
    assert!(inputs.statics.is_empty());
}

#[test]
fn the_view_takes_an_alternative_from_its_highest_priority_member() {
    let dir = laft_test::tempdir();
    let store = store_with_base(dir.path());
    for (name, priority) in [("gcc", 100), ("clang", 10)] {
        TestPkg {
            files: &[("usr/bin/cc", name.as_bytes())],
            alternative: Some(("cc", &["usr/bin/cc"], priority)),
            ..TestPkg::new(name)
        }
        .add(&store, dir.path());
    }
    let mut recipe = recipe("app");
    recipe.build_deps = refs(&["clang", "gcc"]);
    let view = assemble(
        &store,
        &none(),
        &recipe,
        false,
        &dir.path().join("view"),
        None,
    )
    .unwrap();
    assert_eq!(std::fs::read(view.usr.join("bin/cc")).unwrap(), b"gcc");
}

#[test]
fn a_pinned_view_keeps_the_packages_of_an_earlier_build() {
    let dir = laft_test::tempdir();
    let store = store_with_base(dir.path());
    let old = TestPkg {
        files: &[("usr/lib/libz.so.1", b"old")],
        ..TestPkg::new("zlib")
    }
    .add(&store, dir.path());
    let mut pinned: BTreeSet<StoreId> = store_entries(&store)
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();
    std::fs::remove_dir_all(dir.path().join("payload-zlib-dynamic")).unwrap();
    TestPkg {
        files: &[("usr/lib/libz.so.1", b"new")],
        ..TestPkg::new("zlib")
    }
    .add(&store, dir.path());
    let mut recipe = recipe("app");
    recipe.link_deps = refs(&["zlib"]);
    let view = assemble(
        &store,
        &none(),
        &recipe,
        false,
        &dir.path().join("view"),
        Some(&pinned),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(view.usr.join("lib/libz.so.1")).unwrap(),
        b"old"
    );
    assert!(view.packages.iter().any(|p| p.id == old));

    pinned.insert(StoreId::from_manifest_hash(&laft_core::Blake3Hash::of(
        b"gone",
    )));
    let error = assemble(
        &store,
        &none(),
        &recipe,
        false,
        &dir.path().join("again"),
        Some(&pinned),
    )
    .unwrap_err();
    assert_eq!(error.code(), Some("B050"));
}

#[test]
fn a_seed_serves_views_until_a_build_makes_a_package_of_its_origin() {
    let dir = laft_test::tempdir();
    let store = store_with_base(dir.path());
    let seed = TestPkg {
        files: &[("usr/lib/libz.so.1", b"seed")],
        ..TestPkg::new("zlib")
    }
    .add(&store, dir.path());
    let seed_dev = TestPkg {
        files: &[("usr/include/zlib.h", b"seed")],
        origin: Some("zlib"),
        ..TestPkg::new("zlib-dev")
    }
    .add(&store, dir.path());
    let seeds = BTreeSet::from([seed, seed_dev]);
    let mut recipe = recipe("app");
    recipe.link_deps = refs(&["zlib"]);
    let view = assemble(&store, &seeds, &recipe, false, &dir.path().join("a"), None).unwrap();
    assert_eq!(
        std::fs::read(view.usr.join("lib/libz.so.1")).unwrap(),
        b"seed"
    );
    assert!(view.usr.join("include/zlib.h").is_file());

    std::fs::remove_dir_all(dir.path().join("payload-zlib-dynamic")).unwrap();
    let built = TestPkg {
        files: &[("usr/lib/libz.so.1", b"built")],
        ..TestPkg::new("zlib")
    }
    .add(&store, dir.path());
    let view = assemble(&store, &seeds, &recipe, false, &dir.path().join("b"), None).unwrap();
    assert_eq!(
        std::fs::read(view.usr.join("lib/libz.so.1")).unwrap(),
        b"built"
    );
    assert!(
        !view.usr.join("include/zlib.h").exists(),
        "the seeded zlib-dev leaves with the seeded zlib"
    );
    let ids: Vec<StoreId> = view.packages.iter().map(|p| p.id).collect();
    assert!(ids.contains(&built) && !ids.contains(&seed) && !ids.contains(&seed_dev));

    let pinned: BTreeSet<StoreId> = store_entries(&store)
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .filter(|id| *id != built)
        .collect();
    let view = assemble(
        &store,
        &seeds,
        &recipe,
        false,
        &dir.path().join("c"),
        Some(&pinned),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(view.usr.join("lib/libz.so.1")).unwrap(),
        b"seed",
        "a pinned view keeps the seeds it used"
    );
}

#[test]
fn key_inputs_follow_the_usable_packages() {
    let dir = laft_test::tempdir();
    let store = Store::at(dir.path().join("store"));
    let seed = TestPkg {
        provides: &["so:libz.so.1"],
        ..TestPkg::new("zlib")
    }
    .add(&store, dir.path());
    let seeds = BTreeSet::from([seed]);
    let mut recipe = recipe("app");
    recipe.link_deps = refs(&["zlib"]);
    let inputs = key_inputs(&recipe, &store, &seeds).unwrap();
    assert_eq!(
        inputs.links[0].sonames.iter().collect::<Vec<_>>(),
        ["libz.so.1"]
    );

    std::fs::remove_dir_all(dir.path().join("payload-zlib-dynamic")).unwrap();
    TestPkg {
        files: &[("usr/lib/libz.so.2", b"z")],
        provides: &["so:libz.so.2"],
        ..TestPkg::new("zlib")
    }
    .add(&store, dir.path());
    let inputs = key_inputs(&recipe, &store, &seeds).unwrap();
    assert_eq!(
        inputs.links[0].sonames.iter().collect::<Vec<_>>(),
        ["libz.so.2"]
    );
}
