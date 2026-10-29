//! Unified diffs of text, for `/etc` files.

use std::fmt::Write as _;

/// The number of unchanged lines shown around a change.
const CONTEXT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Same(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// The line operations that turn `a` into `b`, from a longest common
/// subsequence. Quadratic in the number of lines, which configuration
/// files keep small.
fn ops(a: &[&str], b: &[&str]) -> Vec<Op> {
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.push(Op::Same(i, j));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            out.push(Op::Delete(i));
            i += 1;
        } else {
            out.push(Op::Insert(j));
            j += 1;
        }
    }
    out
}

/// The unified diff from `old` to `new`, labelled `old_name` and
/// `new_name`, or an empty string when they are equal.
pub(crate) fn unified(old_name: &str, new_name: &str, old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let ops = ops(&a, &b);
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| !matches!(op, Op::Same(..)))
        .map(|(i, _)| i)
        .collect();
    let mut out = format!("--- {old_name}\n+++ {new_name}\n");
    let mut k = 0;
    while k < changed.len() {
        let start = changed[k].saturating_sub(CONTEXT);
        let mut end = changed[k] + CONTEXT + 1;
        while k + 1 < changed.len() && changed[k + 1] <= end + CONTEXT {
            k += 1;
            end = changed[k] + CONTEXT + 1;
        }
        let end = end.min(ops.len());
        let hunk = &ops[start..end];
        let first_a = hunk.iter().find_map(|op| match op {
            Op::Same(i, _) | Op::Delete(i) => Some(*i),
            Op::Insert(_) => None,
        });
        let first_b = hunk.iter().find_map(|op| match op {
            Op::Same(_, j) | Op::Insert(j) => Some(*j),
            Op::Delete(_) => None,
        });
        let count_a = hunk
            .iter()
            .filter(|op| !matches!(op, Op::Insert(_)))
            .count();
        let count_b = hunk
            .iter()
            .filter(|op| !matches!(op, Op::Delete(_)))
            .count();
        let at = |first: Option<usize>, count: usize| match (first, count) {
            (_, 0) => first.unwrap_or(0),
            (Some(f), _) => f + 1,
            (None, _) => 0,
        };
        let _ = writeln!(
            out,
            "@@ -{},{count_a} +{},{count_b} @@",
            at(first_a, count_a),
            at(first_b, count_b)
        );
        for op in hunk {
            let _ = match op {
                Op::Same(i, _) => writeln!(out, " {}", a[*i]),
                Op::Delete(i) => writeln!(out, "-{}", a[*i]),
                Op::Insert(j) => writeln!(out, "+{}", b[*j]),
            };
        }
        k += 1;
    }
    out
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
