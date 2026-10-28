use std::collections::BTreeMap;

use super::order;

fn graph<'a>(edges: &[(&'a str, &[&'a str])]) -> BTreeMap<&'a str, Vec<&'a str>> {
    edges.iter().map(|(n, d)| (*n, d.to_vec())).collect()
}

#[test]
fn dependencies_come_first_and_ties_go_by_rank_then_name() {
    let g = graph(&[
        ("root", &[]),
        ("b", &["root"]),
        ("a", &["root"]),
        ("daemon", &["root"]),
        ("end", &["a", "b"]),
    ]);
    let ranked = order(&g, |n| u8::from(n != "daemon")).expect("no cycle");
    assert_eq!(ranked, ["root", "daemon", "a", "b", "end"]);
    let by_name = order(&g, |_| 0).expect("no cycle");
    assert_eq!(by_name, ["root", "a", "b", "daemon", "end"]);
}

#[test]
fn dependencies_outside_the_graph_are_ignored() {
    let g = graph(&[("a", &["elsewhere"]), ("b", &["a", "elsewhere"])]);
    assert_eq!(order(&g, |_| 0).expect("no cycle"), ["a", "b"]);
}

#[test]
fn cycle_is_reported_in_order_from_its_smallest_node() {
    let g = graph(&[
        ("worker", &["myapp"]),
        ("myapp", &["zeta"]),
        ("zeta", &["worker"]),
        ("free", &[]),
        ("blocked", &["worker"]),
    ]);
    assert_eq!(
        order(&g, |_| 0).unwrap_err(),
        ["myapp", "zeta", "worker", "myapp"]
    );
}

#[test]
fn self_dependency_is_a_cycle() {
    let g = graph(&[("a", &["a"])]);
    assert_eq!(order(&g, |_| 0).unwrap_err(), ["a", "a"]);
}
