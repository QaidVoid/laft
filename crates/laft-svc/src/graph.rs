//! Dependency ordering shared by validation, rendering and reconciliation.

use std::collections::{BTreeMap, BTreeSet};

/// Orders `nodes` so that every node comes after the nodes it depends on.
///
/// `nodes` maps each node to its dependencies. Dependencies that are not
/// nodes themselves are ignored, so a subset of a graph can be ordered on
/// its own. Among the nodes whose dependencies are all placed, the one with
/// the lowest `rank` comes first, and ties are broken by name, so the order
/// is fully determined by the graph.
///
/// Fails with a cycle when the dependencies form one. The cycle starts and
/// ends at its smallest node and lists each node before its dependency.
pub(crate) fn order<'a>(
    nodes: &BTreeMap<&'a str, Vec<&'a str>>,
    rank: impl Fn(&str) -> u8,
) -> Result<Vec<&'a str>, Vec<String>> {
    let mut missing: BTreeMap<&str, usize> = BTreeMap::new();
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (&node, deps) in nodes {
        let deps: BTreeSet<&str> = deps
            .iter()
            .copied()
            .filter(|d| nodes.contains_key(d))
            .collect();
        missing.insert(node, deps.len());
        for dep in deps {
            dependents.entry(dep).or_default().push(node);
        }
    }
    let mut ready: BTreeSet<(u8, &str)> = missing
        .iter()
        .filter(|&(_, &n)| n == 0)
        .map(|(&node, _)| (rank(node), node))
        .collect();
    let mut placed = Vec::with_capacity(nodes.len());
    while let Some((_, node)) = ready.pop_first() {
        placed.push(node);
        for &dependent in dependents.get(node).into_iter().flatten() {
            let count = missing
                .get_mut(dependent)
                .expect("every dependent is a node");
            *count -= 1;
            if *count == 0 {
                ready.insert((rank(dependent), dependent));
            }
        }
    }
    if placed.len() == nodes.len() {
        return Ok(placed);
    }
    let placed: BTreeSet<&str> = placed.into_iter().collect();
    Err(find_cycle(nodes, &placed))
}

/// Finds a cycle among the nodes that could not be placed. Each of them has
/// a dependency that could not be placed either, so following the smallest
/// such dependency must revisit a node.
fn find_cycle(nodes: &BTreeMap<&str, Vec<&str>>, placed: &BTreeSet<&str>) -> Vec<String> {
    let stuck = |node: &str| nodes.contains_key(node) && !placed.contains(node);
    let start = nodes
        .keys()
        .copied()
        .find(|n| stuck(n))
        .expect("a stalled ordering leaves a node unplaced");
    let mut path: Vec<&str> = vec![start];
    loop {
        let last = *path.last().expect("the path is never empty");
        let next = nodes[last]
            .iter()
            .copied()
            .filter(|d| stuck(d))
            .min()
            .expect("an unplaced node has an unplaced dependency");
        if let Some(at) = path.iter().position(|&n| n == next) {
            let cycle = &path[at..];
            let smallest = cycle
                .iter()
                .enumerate()
                .min_by_key(|&(_, n)| n)
                .map_or(0, |(i, _)| i);
            let mut out: Vec<String> = cycle[smallest..]
                .iter()
                .chain(&cycle[..smallest])
                .map(|n| (*n).to_owned())
                .collect();
            out.push(out[0].clone());
            return out;
        }
        path.push(next);
    }
}

#[cfg(test)]
#[path = "graph_tests.rs"]
mod tests;
