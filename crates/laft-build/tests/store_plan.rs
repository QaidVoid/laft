//! In store mode, the plan leaves a dependency unsatisfied unless an
//! option set in the build-side store holds its constraints.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use laft_build::{BuildRoot, Builder, Mode, Repos, SourceCache};
use laft_core::{Blake3Hash, OptionValue, Variant};
use laft_fetch::{Config, Fetcher};
use laft_pkg::{Compression, Manifest, Meta, scan, write_package};
use support::SOURCE;

/// Writes the package `name` at 1.0 with `options` and seeds it into
/// `root`, as a repository would.
fn seed_with(root: &BuildRoot, name: &str, options: &[(&str, OptionValue)]) {
    let payload = laft_test::tempdir();
    let mut meta = Meta {
        format: laft_pkg::FORMAT,
        name: name.parse().expect("a valid name"),
        version: "1.0".parse().expect("a valid version"),
        rev: 1,
        variant: Variant::default(),
        arch: laft_pkg::Arch::X86_64V3,
        origin: name.parse().expect("a valid name"),
        summary: format!("The {name} package"),
        homepage: None,
        license: vec!["MIT".into()],
        depends: Vec::new(),
        provides: Vec::new(),
        conflicts: Vec::new(),
        replaces: Vec::new(),
        alternatives: Vec::new(),
        triggers: Vec::new(),
        static_deps: Vec::new(),
        abi: BTreeMap::new(),
        abi_epoch: 0,
        options: options
            .iter()
            .map(|(option, value)| ((*option).to_owned(), value.clone()))
            .collect(),
        requested_privileges: Vec::new(),
        installed_size: 0,
    };
    meta.summary = format!("The {name} package");
    let entries = scan(payload.path()).expect("the payload scans");
    let meta_json = meta.to_json().expect("valid metadata");
    let manifest =
        Manifest::new(meta.identity(), Blake3Hash::of(&meta_json), entries).expect("valid");
    let mut bytes = Vec::new();
    write_package(
        &mut bytes,
        payload.path(),
        &meta,
        &manifest,
        Compression::DEVELOPMENT,
    )
    .expect("the package writes");
    let file = root.path().join(format!("{name}.lpkg"));
    fs::create_dir_all(root.path()).expect("writable");
    fs::write(&file, bytes).expect("writable");
    laft_build::seed(root, &file).expect("the package is seeded");
}

#[test]
fn a_plan_counts_only_the_option_set_that_holds() {
    let dir = laft_test::tempdir();
    let repo = support::write_repo(
        dir.path().join("core").as_path(),
        "core",
        "",
        &[(
            "recipes/hello/package.stav",
            &support::recipe(
                "hello",
                &format!("{SOURCE}  build_deps = [greet{{!tool}}]\n  build custom {{}}"),
            ),
        )],
    );
    let repos = Repos::open(&[repo]).expect("the repository opens");

    // The store holds greet with the tool on, which does not satisfy the
    // dependency that refuses the tool.
    let root = BuildRoot::new(dir.path().join("root"));
    seed_with(&root, "greet", &[("tool", OptionValue::TRUE)]);

    let config = Config {
        offline: true,
        ..Config::new(dir.path().join("cache"))
    };
    let cache = SourceCache::new(root.sources(), Fetcher::new(config));
    let builder = Builder::new(&repos, &cache, &root, Mode::Store);
    let none = BTreeSet::new();
    let recipes = support::evaluate(&repos, "hello");
    let (_, _, unsatisfied) = builder
        .plan_open(recipes, &root.db(), &none, &none)
        .expect("the plan scans");
    assert_eq!(unsatisfied.len(), 1, "{unsatisfied:?}");
    let reference = &unsatisfied[0];
    assert_eq!(reference.name().as_str(), "greet");
    assert_eq!(reference.options().to_string(), "{!tool}");
}
