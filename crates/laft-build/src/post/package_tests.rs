use std::collections::BTreeMap;

use super::*;
use crate::test_support::{elf_fixture, recipe, write_tree};

struct Fixture {
    dir: laft_test::TempDir,
    entries: Vec<Entry>,
    elves: ElfFiles,
    detected: Detected,
    privileges: Privileges,
    abi: BTreeMap<String, Abi>,
}

fn fixture(recipe: &Recipe) -> Fixture {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    write_tree(
        &out,
        &[
            ("usr/bin/program", &elf_fixture("program")),
            ("usr/lib/libversioned.so.1", &elf_fixture("libversioned.so")),
            ("usr/include/v.h", b"h"),
            ("etc/v/v.conf", b"conf"),
        ],
    );
    let privileges = crate::post::privileges(&out, &recipe.outputs).unwrap();
    let elves = crate::post::scan_elf(&out, &mut Vec::new()).unwrap();
    let detected = crate::post::detect(&out, &elves, recipe.variant, &[], &mut Vec::new()).unwrap();
    let abi = crate::post::abi_digests(&elves);
    let entries = laft_pkg::scan(&out).unwrap();
    Fixture {
        dir,
        entries,
        elves,
        detected,
        privileges,
        abi,
    }
}

impl Fixture {
    fn payload(&self) -> Payload<'_> {
        Payload {
            entries: &self.entries,
            elves: &self.elves,
            detected: &self.detected,
            privileges: &self.privileges,
            abi: &self.abi,
            static_deps: &[],
        }
    }
}

fn part(name: &str, suffix: Option<&str>, paths: &[&str]) -> Part {
    Part {
        name: name.parse().unwrap(),
        suffix: suffix.map(str::to_owned),
        paths: paths.iter().map(|p| (*p).to_owned()).collect(),
    }
}

fn texts<T: ToString>(items: &[T]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

#[test]
fn the_main_package_combines_detected_and_declared_relations() {
    let mut recipe = recipe("v");
    recipe.outputs.deps = crate::test_support::refs(&["bash"]);
    recipe.outputs.provides = vec!["virtual:greeter".into()];
    recipe.outputs.ownership = BTreeMap::from([("etc/v".into(), "log:log".into())]);
    let fixture = fixture(&recipe);
    let main = part(
        "v",
        None,
        &[
            "usr/bin/program",
            "usr/lib/libversioned.so.1",
            "etc/v/v.conf",
        ],
    );
    let package = package(fixture.payload(), &recipe, &main, Vec::new()).unwrap();
    assert_eq!(texts(&package.meta.depends), ["bash"]);
    assert_eq!(
        texts(&package.meta.provides),
        ["so:libversioned.so.1", "virtual:greeter"]
    );
    assert_eq!(
        package.meta.abi.keys().collect::<Vec<_>>(),
        ["usr/lib/libversioned.so.1"]
    );
    let paths: Vec<&str> = package.manifest.entries().iter().map(Entry::path).collect();
    assert_eq!(
        paths,
        [
            "etc",
            "etc/v",
            "etc/v/v.conf",
            "usr",
            "usr/bin",
            "usr/bin/program",
            "usr/lib",
            "usr/lib/libversioned.so.1",
        ]
    );
    let conf = &package.manifest.entries()[2];
    assert!(matches!(conf, Entry::File { owner: Some(o), .. } if o == "log"));
    assert!(matches!(
        &package.manifest.entries()[0],
        Entry::Dir {
            mode: 0o755,
            owner: None,
            ..
        }
    ));
    let files: u64 = [
        "usr/bin/program",
        "usr/lib/libversioned.so.1",
        "etc/v/v.conf",
    ]
    .iter()
    .map(|p| {
        fs::metadata(fixture.dir.path().join("out").join(p))
            .unwrap()
            .len()
    })
    .sum();
    assert_eq!(package.meta.installed_size, files);
}

#[test]
fn automatic_subpackages_depend_on_the_exact_main_package() {
    let recipe = recipe("v");
    let fixture = fixture(&recipe);
    let dev = part("v-dev", Some("dev"), &["usr/include/v.h"]);
    let package = package(fixture.payload(), &recipe, &dev, Vec::new()).unwrap();
    assert_eq!(texts(&package.meta.depends), ["v=1.0-r1"]);
    assert_eq!(package.meta.summary, "A package (dev)");
    assert_eq!(package.meta.origin.as_str(), "v");
}

#[test]
fn the_store_id_follows_the_metadata_and_the_payload() {
    let recipe = recipe("v");
    let fixture = fixture(&recipe);
    let main = part("v", None, &["usr/bin/program"]);
    let first = package(fixture.payload(), &recipe, &main, Vec::new()).unwrap();
    let second = package(fixture.payload(), &recipe, &main, Vec::new()).unwrap();
    assert_eq!(first.manifest.store_id(), second.manifest.store_id());
    let mut edited = recipe.clone();
    edited.meta.summary = "Another summary".into();
    let third = package(fixture.payload(), &edited, &main, Vec::new()).unwrap();
    assert_ne!(first.manifest.store_id(), third.manifest.store_id());
}

#[test]
fn packages_are_written_under_their_file_names() {
    let recipe = recipe("v");
    let fixture = fixture(&recipe);
    let main = part("v", None, &["usr/bin/program", "etc/v/v.conf"]);
    let package = package(fixture.payload(), &recipe, &main, Vec::new()).unwrap();
    let dir = fixture.dir.path().join("pkgs");
    let written = write_packages(
        &fixture.dir.path().join("out"),
        std::slice::from_ref(&package),
        &dir,
        Compression::DEVELOPMENT,
    )
    .unwrap();
    assert_eq!(written, [dir.join("v-1.0-r1.x86_64-v3.lpkg")]);
    let reader = laft_pkg::Reader::new(File::open(&written[0]).unwrap()).unwrap();
    assert_eq!(reader.manifest().store_id(), package.manifest.store_id());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
}
