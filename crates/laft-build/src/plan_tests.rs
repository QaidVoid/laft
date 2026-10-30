use laft_core::PkgRef;

use super::*;
use crate::recipe::{Fetch, FileRef, Source};
use crate::test_support::{item, recipe, refs};

fn hash(byte: u8) -> Sha256Hash {
    Sha256Hash::from_bytes([byte; 32])
}

fn id(byte: u8) -> StoreId {
    StoreId::from_manifest_hash(&Blake3Hash::from_bytes([byte; 32]))
}

fn name(text: &str) -> Name {
    text.parse().expect("a valid name")
}

fn link(library: &str, abi_epoch: u32, sonames: &[&str]) -> LinkInput {
    LinkInput {
        name: name(library),
        abi_epoch,
        sonames: sonames.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// A recipe with one URL source and one patch.
fn foo(text: &str) -> Recipe {
    let mut r = recipe(text);
    r.sources = vec![Source {
        label: "main".into(),
        fetch: Fetch::Url {
            urls: vec!["https://example.org/foo-1.0.tar.gz".into()],
            sha256: hash(1),
            file: "foo-1.0.tar.gz".into(),
            extract: true,
        },
        dest: String::new(),
        strip_components: None,
    }];
    r.patches = vec![FileRef {
        path: "/build/recipes/core/foo/patches/0001.patch".into(),
        blake3: Blake3Hash::of(b"patch"),
    }];
    r
}

fn record(inputs: &KeyInputs, item: &Item) -> BuildRecord {
    BuildRecord {
        item: item.clone(),
        key: inputs.build_key(),
        inputs: inputs.clone(),
        outputs: BTreeMap::new(),
        imports: BTreeMap::new(),
    }
}

/// The triggers of `changed` against a build of `before`, after checking
/// that the build key changed exactly when a trigger fired.
fn fired(before: &KeyInputs, changed: &KeyInputs) -> Vec<&'static str> {
    let previous = record(before, &item("foo"));
    let triggers = changed.triggers(Some(&previous), false);
    assert_eq!(
        triggers.is_empty(),
        before.build_key() == changed.build_key(),
        "{triggers:?}"
    );
    triggers.into_iter().map(Trigger::code).collect()
}

#[test]
fn every_trigger_fires_exactly_when_its_input_changes() {
    let base = foo("foo:static");
    let links = vec![link("zlib", 0, &["libz.so.1"])];
    let statics = vec![StaticInput {
        name: name("zlib"),
        id: id(1),
    }];
    let before = KeyInputs::new(&base, links.clone(), statics.clone());
    let same = KeyInputs::new(&base.clone(), links.clone(), statics.clone());
    assert_eq!(fired(&before, &same), Vec::<&str>::new());

    let mut edited = base.clone();
    edited.meta.summary = "Another summary".into();
    assert_eq!(
        fired(
            &before,
            &KeyInputs::new(&edited, links.clone(), statics.clone())
        ),
        ["T1"]
    );

    let mut operations = base.clone();
    operations.operations.cpe = vec!["cpe:2.3:a:foo:foo".into()];
    assert_eq!(
        fired(
            &before,
            &KeyInputs::new(&operations, links.clone(), statics.clone())
        ),
        Vec::<&str>::new(),
        "operational fields never rebuild"
    );

    let mut source = base.clone();
    let Fetch::Url { sha256, .. } = &mut source.sources[0].fetch;
    *sha256 = hash(2);
    assert_eq!(
        fired(
            &before,
            &KeyInputs::new(&source, links.clone(), statics.clone())
        ),
        ["T2"]
    );
    let mut patch = base.clone();
    patch.patches[0].blake3 = Blake3Hash::of(b"another patch");
    assert_eq!(
        fired(
            &before,
            &KeyInputs::new(&patch, links.clone(), statics.clone())
        ),
        ["T2"]
    );

    let gone = vec![link("zlib", 0, &["libz.so.2"])];
    assert_eq!(
        fired(&before, &KeyInputs::new(&base, gone, statics.clone())),
        ["T3"]
    );

    let rebuilt = vec![StaticInput {
        name: name("zlib"),
        id: id(2),
    }];
    assert_eq!(
        fired(&before, &KeyInputs::new(&base, links.clone(), rebuilt)),
        ["T4"]
    );

    let epoch = vec![link("zlib", 1, &["libz.so.1"])];
    assert_eq!(
        fired(&before, &KeyInputs::new(&base, epoch, statics.clone())),
        ["T5"]
    );

    let mut toolchain = base.clone();
    toolchain.toolchain_epoch = 1;
    assert_eq!(
        fired(
            &before,
            &KeyInputs::new(&toolchain, links.clone(), statics.clone())
        ),
        ["T6"]
    );

    let codes = |t: BTreeSet<Trigger>| t.into_iter().map(Trigger::code).collect::<Vec<_>>();
    assert_eq!(codes(before.triggers(None, false)), ["T7"]);
    assert_eq!(codes(before.triggers(None, true)), ["T7", "T8"]);
    let previous = record(&before, &item("foo"));
    assert_eq!(codes(before.triggers(Some(&previous), true)), ["T8"]);
}

#[test]
fn dynamic_builds_ignore_static_packages_in_their_view() {
    let base = foo("foo");
    let statics = |byte| {
        vec![StaticInput {
            name: name("zlib"),
            id: id(byte),
        }]
    };
    let before = KeyInputs::new(&base, Vec::new(), statics(1));
    assert_eq!(
        fired(&before, &KeyInputs::new(&base, Vec::new(), statics(2))),
        Vec::<&str>::new()
    );
}

#[test]
fn a_raised_abi_epoch_rebuilds_only_what_links_against_the_library() {
    let mut library = recipe("libfoo");
    library.abi_epoch = 1;
    let mut linker = recipe("app");
    linker.link_deps = refs(&["libfoo"]);
    let mut runner = recipe("tool");
    runner.outputs.deps = refs(&["libfoo"]);
    let graph = BuildGraph::new([library, linker, runner], &|_: &PkgRef| true)
        .expect("every reference resolves");
    let links_of = |item: &Item, epoch: &dyn Fn(&Item) -> u32| -> Vec<LinkInput> {
        let deps = graph.deps(item).expect("in the graph");
        deps.link
            .iter()
            .map(|l| link(l.name.as_str(), epoch(l), &["libfoo.so.1"]))
            .collect()
    };
    for (text, expected) in [("app", vec!["T5"]), ("tool", vec![])] {
        let it = item(text);
        let r = graph.recipe(&it).expect("in the graph");
        let before = KeyInputs::new(r, links_of(&it, &|_| 0), Vec::new());
        let now = KeyInputs::new(
            r,
            links_of(&it, &|l| graph.recipe(l).map_or(0, |r| r.abi_epoch)),
            Vec::new(),
        );
        assert_eq!(fired(&before, &now), expected, "{text}");
    }
}

/// libfoo, linked by app and by other, and the static pair pcre2 and
/// ripgrep.
fn run() -> (BuildGraph, BTreeMap<Item, BuildRecord>) {
    let mut app = recipe("app");
    app.link_deps = refs(&["libfoo"]);
    let mut other = recipe("other");
    other.link_deps = refs(&["libfoo"]);
    let mut rg = recipe("ripgrep");
    rg.link_deps = refs(&["pcre2"]);
    let mut rg_static = recipe("ripgrep:static");
    rg_static.link_deps = refs(&["pcre2:static"]);
    let graph = BuildGraph::new(
        [
            recipe("libfoo"),
            app,
            other,
            recipe("pcre2"),
            recipe("pcre2:static"),
            rg,
            rg_static,
        ],
        &|_: &PkgRef| true,
    )
    .expect("every reference resolves");
    let mut previous = BTreeMap::new();
    for it in graph.items() {
        let r = graph.recipe(it).expect("in the graph");
        let links = graph
            .deps(it)
            .into_iter()
            .flat_map(|d| &d.link)
            .map(|l| {
                link(
                    l.name.as_str(),
                    0,
                    &[format!("lib{}.so.1", l.name).as_str()],
                )
            })
            .collect();
        let inputs = KeyInputs::new(r, links, Vec::new());
        let mut rec = record(&inputs, it);
        rec.outputs.insert(it.name.clone(), id(1));
        previous.insert(it.clone(), rec);
    }
    let imports = |symbols: &[&str]| {
        BTreeMap::from([(
            "liblibfoo.so.1".to_owned(),
            symbols.iter().map(|s| (*s).to_owned()).collect(),
        )])
    };
    if let Some(app) = previous.get_mut(&item("app")) {
        app.imports = imports(&["foo_open@FOO_1", "foo_close@FOO_1"]);
    }
    if let Some(other) = previous.get_mut(&item("other")) {
        other.imports = imports(&["foo_open@FOO_1"]);
    }
    (graph, previous)
}

fn built(outputs: &[(&str, u8)], exports: &[(&str, &[&str])]) -> Built {
    Built {
        outputs: outputs.iter().map(|(n, b)| (name(n), id(*b))).collect(),
        exports: exports
            .iter()
            .map(|(soname, symbols)| {
                (
                    (*soname).to_owned(),
                    symbols.iter().map(|s| (*s).to_owned()).collect(),
                )
            })
            .collect(),
    }
}

fn dirty(texts: &[&str]) -> BTreeMap<Item, BTreeSet<Trigger>> {
    texts
        .iter()
        .map(|t| (item(t), BTreeSet::from([Trigger::Plan])))
        .collect()
}

#[test]
fn unchanged_outputs_stop_propagation() {
    let (graph, previous) = run();
    let mut schedule =
        Schedule::new(&graph, dirty(&["libfoo"]), previous, &BTreeSet::new()).expect("no cycle");
    assert_eq!(schedule.next_build(), Some(item("libfoo")));
    let same = built(&[("libfoo", 1)], &[("liblibfoo.so.1", &[])]);
    assert_eq!(schedule.finish(&item("libfoo"), &same), Cutoff::Unchanged);
    assert_eq!(schedule.next_build(), None);
}

#[test]
fn a_removed_symbol_rebuilds_only_its_importers() {
    let (graph, previous) = run();
    let mut schedule =
        Schedule::new(&graph, dirty(&["libfoo"]), previous, &BTreeSet::new()).expect("no cycle");
    assert_eq!(schedule.next_build(), Some(item("libfoo")));
    let new = built(&[("libfoo", 2)], &[("liblibfoo.so.1", &["foo_open@FOO_1"])]);
    let Cutoff::Changed(dirtied) = schedule.finish(&item("libfoo"), &new) else {
        panic!("the outputs changed");
    };
    assert_eq!(
        dirtied,
        [Dirtied {
            item: item("app"),
            trigger: Trigger::Import,
            reason: "liblibfoo.so.1 no longer exports foo_close@FOO_1".into(),
            late: false,
        }]
    );
    assert_eq!(schedule.next_build(), Some(item("app")));
    assert_eq!(schedule.next_build(), None);
}

#[test]
fn a_new_soname_rebuilds_everything_that_imported_the_old_one() {
    let (graph, previous) = run();
    let mut schedule =
        Schedule::new(&graph, dirty(&["libfoo"]), previous, &BTreeSet::new()).expect("no cycle");
    schedule.next_build();
    let new = built(&[("libfoo", 2)], &[("liblibfoo.so.2", &["foo_open@FOO_2"])]);
    let Cutoff::Changed(dirtied) = schedule.finish(&item("libfoo"), &new) else {
        panic!("the outputs changed");
    };
    let reasons: Vec<(String, &str)> = dirtied
        .iter()
        .map(|d| (d.item.to_string(), d.reason.as_str()))
        .collect();
    let reason = "liblibfoo.so.1 is no longer provided (now liblibfoo.so.2)";
    assert_eq!(
        reasons,
        [("app".to_owned(), reason), ("other".to_owned(), reason)]
    );
    let planned: Vec<String> = schedule
        .planned()
        .into_iter()
        .map(|(i, t)| {
            format!(
                "{i} {}",
                t.iter().map(|t| t.code()).collect::<Vec<_>>().join(",")
            )
        })
        .collect();
    assert_eq!(planned, ["libfoo T1", "app T3", "other T3"]);
}

#[test]
fn a_changed_static_library_rebuilds_only_static_dependents() {
    let (graph, previous) = run();
    let mut schedule = Schedule::new(&graph, dirty(&["pcre2:static"]), previous, &BTreeSet::new())
        .expect("no cycle");
    assert_eq!(schedule.next_build(), Some(item("pcre2:static")));
    let new = built(&[("pcre2", 2)], &[]);
    let Cutoff::Changed(dirtied) = schedule.finish(&item("pcre2:static"), &new) else {
        panic!("the outputs changed");
    };
    assert_eq!(
        dirtied,
        [Dirtied {
            item: item("ripgrep:static"),
            trigger: Trigger::Static,
            reason: "pcre2:static has a new store id".into(),
            late: false,
        }]
    );
    assert_eq!(schedule.next_build(), Some(item("ripgrep:static")));
    assert_eq!(schedule.next_build(), None);
}

#[test]
fn dirty_items_in_a_cycle_cannot_be_scheduled() {
    let mut gcc = recipe("gcc");
    gcc.link_deps = refs(&["glibc"]);
    let mut glibc = recipe("glibc");
    glibc.build_deps = refs(&["gcc"]);
    let graph = BuildGraph::new([gcc, glibc], &|_: &PkgRef| true).expect("resolves");
    let error = Schedule::new(
        &graph,
        dirty(&["gcc", "glibc"]),
        BTreeMap::new(),
        &BTreeSet::new(),
    )
    .expect_err("a cycle");
    assert_eq!(error.code, "B010");
}

#[test]
fn a_stored_record_rederives_its_build_key() {
    let inputs = KeyInputs::new(
        &foo("foo:static"),
        vec![link("zlib", 0, &["libz.so.1"])],
        vec![StaticInput {
            name: name("zlib"),
            id: id(1),
        }],
    );
    let stored = serde_json::to_string(&record(&inputs, &item("foo"))).expect("serializable");
    let read: BuildRecord = serde_json::from_str(&stored).expect("readable");
    let canonical = laft_core::to_canonical_json(&read.inputs).expect("canonical");
    assert_eq!(Blake3Hash::of(&canonical), read.key);
}
