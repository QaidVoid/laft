use super::*;
use crate::test_support::{item, recipe, refs};

fn with(text: &str, build: &[&str], link: &[&str]) -> Recipe {
    let mut r = recipe(text);
    r.build_deps = refs(build);
    r.link_deps = refs(link);
    r
}

fn everything(_: &PkgRef) -> bool {
    true
}

fn items(texts: &[&str]) -> BTreeSet<Item> {
    texts.iter().map(|t| item(t)).collect()
}

fn names(order: &[Item]) -> Vec<String> {
    order.iter().map(ToString::to_string).collect()
}

#[test]
fn dependencies_come_before_their_dependents() {
    let mut openssl = with("openssl", &["perl"], &["zlib"]);
    openssl
        .outputs
        .subpackages
        .insert("libs".into(), crate::test_support::subpackage());
    let graph = BuildGraph::new(
        [
            with("curl", &["pkgconf"], &["openssl-libs", "zlib-dev"]),
            openssl,
            with("perl", &[], &[]),
            with("zlib", &[], &[]),
            with("aaa", &[], &["curl"]),
        ],
        &everything,
    )
    .expect("every reference resolves");
    let deps = graph.deps(&item("curl")).expect("curl is in the graph");
    assert_eq!(deps.link, items(&["openssl", "zlib"]));
    assert!(deps.build.is_empty(), "pkgconf is published");
    let all = items(&["aaa", "curl", "openssl", "perl", "zlib"]);
    let order = graph.order(&all, &BTreeSet::new()).expect("no cycle");
    assert_eq!(names(&order), ["perl", "zlib", "openssl", "curl", "aaa"]);
}

#[test]
fn a_static_build_links_only_static_libraries() {
    let problems = BuildGraph::new(
        [
            with("ripgrep:static", &["rust"], &["pcre2:static"]),
            with("ripgrep", &["rust"], &["pcre2"]),
            with("pcre2", &[], &[]),
        ],
        &everything,
    )
    .expect_err("pcre2 has no static variant");
    let messages: Vec<String> = problems.iter().map(ToString::to_string).collect();
    assert_eq!(
        messages,
        ["error[R020]: ripgrep:static links against pcre2, which has no static variant"]
    );
}

#[test]
fn references_outside_the_run_must_be_published() {
    let mut zlib = with("zlib", &["make"], &[]);
    zlib.outputs.deps = refs(&["glibc"]);
    let llvm = with("llvm", &[], &[]);
    let clang = with("clang", &[], &["llvm@18"]);
    let published = |r: &PkgRef| r.name().as_str() == "make" || r.prefix().is_some();
    let problems = BuildGraph::new([zlib, llvm, clang], &published).expect_err("glibc is missing");
    assert_eq!(
        problems,
        [Problem::new(
            "R021",
            "zlib depends on glibc, which no recipe builds and no repository publishes"
        )]
    );
    let llvm19 = with("llvm", &[], &[]);
    let clang = with("clang", &[], &["llvm@1"]);
    let graph = BuildGraph::new([llvm19, clang], &everything).expect("llvm@1 matches 1.0");
    assert_eq!(graph.deps(&item("clang")).map(|d| d.link.len()), Some(1));
}

#[test]
fn only_cycles_of_dirty_items_stop_the_run() {
    let graph = BuildGraph::new(
        [
            with("gcc", &[], &["glibc"]),
            with("glibc", &["gcc"], &[]),
            with("rust", &["rust"], &[]),
            with("hello", &["gcc"], &[]),
        ],
        &everything,
    )
    .expect("every reference resolves");
    let none = BTreeSet::new();
    let error = graph
        .order(&items(&["gcc", "glibc", "hello"]), &none)
        .expect_err("gcc and glibc wait for each other");
    assert_eq!(
        error.to_string(),
        "error[B010]: build cycle among dirty packages: gcc -> glibc -> gcc"
    );
    let order = graph
        .order(&items(&["gcc", "glibc", "hello"]), &items(&["glibc"]))
        .expect("gcc uses the published glibc");
    assert_eq!(names(&order), ["gcc", "glibc", "hello", "rust"]);
    let order = graph
        .order(&items(&["glibc", "hello", "rust"]), &none)
        .expect("gcc is clean and rust builds with the published rust");
    let at = |name: &str| order.iter().position(|i| i.to_string() == name);
    assert!(at("gcc") < at("glibc"), "{order:?}");
    assert!(at("glibc") < at("hello"), "{order:?}");
}

#[test]
fn static_inputs_follow_static_link_dependencies_transitively() {
    let graph = BuildGraph::new(
        [
            with("app:static", &["tool"], &["a:static"]),
            with("a:static", &[], &["b:static"]),
            with("b:static", &[], &[]),
            with("tool", &[], &[]),
        ],
        &everything,
    )
    .expect("every reference resolves");
    assert_eq!(
        graph.static_inputs(&item("app:static")),
        items(&["a:static", "b:static"])
    );
}

#[test]
fn two_recipes_for_one_item_are_reported() {
    let problems = BuildGraph::new(
        [
            recipe("zlib"),
            recipe("zlib"),
            recipe("zlib:static"),
            recipe("curl"),
        ],
        &everything,
    )
    .expect_err("zlib comes twice");
    assert_eq!(
        problems,
        [Problem::new("R022", "the run holds two recipes for zlib")]
    );
}
