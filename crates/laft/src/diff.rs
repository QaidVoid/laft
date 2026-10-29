//! Unified diffs of text, for `/etc` files.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

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
    if a == b {
        return format!("--- {old_name}\n+++ {new_name}\n\\ No newline at end of file\n");
    }
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

/// What a path of a static `/etc` tree holds, for comparing two trees.
#[derive(Debug, PartialEq, Eq)]
enum Content {
    Text(String),
    Link(String),
    Binary(Vec<u8>),
}

/// Every file and symlink below `dir`, by path relative to it.
fn contents(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Content>) -> io::Result<()> {
    for item in fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = item.file_type()?;
        if kind.is_symlink() {
            let target = fs::read_link(item.path())?;
            out.insert(rel, Content::Link(target.to_string_lossy().into_owned()));
        } else if kind.is_dir() {
            contents(&item.path(), &rel, out)?;
        } else {
            let bytes = fs::read(item.path())?;
            let content = match String::from_utf8(bytes) {
                Ok(text) => Content::Text(text),
                Err(e) => Content::Binary(e.into_bytes()),
            };
            out.insert(rel, content);
        }
    }
    Ok(())
}

/// The unified diffs between the static `/etc` trees `old`, or none, and
/// `new`, in path order. A symlink shows as `-> <target>`, and a file that
/// is not UTF-8 is only named.
///
/// # Errors
///
/// Fails when a tree cannot be read.
pub(crate) fn etc_trees(old: Option<&Path>, new: &Path) -> io::Result<String> {
    let mut before = BTreeMap::new();
    if let Some(old) = old {
        contents(old, "", &mut before)?;
    }
    let mut after = BTreeMap::new();
    contents(new, "", &mut after)?;
    let paths: std::collections::BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let mut out = String::new();
    for path in paths {
        let (a, b) = (before.get(path), after.get(path));
        if a == b {
            continue;
        }
        let text = |c: Option<&Content>| match c {
            None => String::new(),
            Some(Content::Text(text)) => text.clone(),
            Some(Content::Link(target)) => format!("-> {target}\n"),
            Some(Content::Binary(_)) => String::new(),
        };
        let name = format!("/etc/{path}");
        let old_name = if a.is_some() {
            name.as_str()
        } else {
            "/dev/null"
        };
        let new_name = if b.is_some() {
            name.as_str()
        } else {
            "/dev/null"
        };
        if matches!(a, Some(Content::Binary(_))) || matches!(b, Some(Content::Binary(_))) {
            let _ = writeln!(out, "Binary files {old_name} and {new_name} differ");
            continue;
        }
        out.push_str(&unified(old_name, new_name, &text(a), &text(b)));
    }
    Ok(out)
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
