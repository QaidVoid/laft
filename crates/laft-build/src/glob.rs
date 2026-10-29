//! Globs over payload paths, as recipes write them in `files`, `exclude`,
//! `ignore_deps` and `ownership`.

/// A glob over payload paths.
///
/// Paths are relative and separated by `/`. `*` matches any characters
/// except `/`, `?` matches one character except `/`, `**` as a whole
/// segment matches zero or more segments, and `{a,b}` matches either
/// alternative. Matching is case sensitive and byte based. A glob that
/// matches a directory matches everything below it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Glob {
    /// The glob with every brace alternative expanded, split into segments.
    patterns: Vec<Vec<String>>,
}

impl Glob {
    /// The glob written `text`.
    pub(crate) fn new(text: &str) -> Self {
        let patterns = expand(text)
            .into_iter()
            .map(|p| p.split('/').map(str::to_owned).collect())
            .collect();
        Self { patterns }
    }

    /// Whether the glob matches `path` or one of its parent directories.
    pub(crate) fn matches(&self, path: &str) -> bool {
        let parts: Vec<&str> = path.split('/').collect();
        (1..=parts.len()).any(|n| {
            self.patterns
                .iter()
                .any(|pattern| segments_match(pattern, &parts[..n]))
        })
    }
}

/// Whether any glob of `globs` matches `path`.
pub(crate) fn any_matches(globs: &[Glob], path: &str) -> bool {
    globs.iter().any(|g| g.matches(path))
}

/// Expands the first `{a,b}` group of `text` and recurses, so the result
/// has no braces left. An unclosed brace is taken literally.
fn expand(text: &str) -> Vec<String> {
    let Some(open) = text.find('{') else {
        return vec![text.to_owned()];
    };
    let Some(close) = text[open..].find('}').map(|c| c + open) else {
        return vec![text.to_owned()];
    };
    let (head, tail) = (&text[..open], &text[close + 1..]);
    text[open + 1..close]
        .split(',')
        .flat_map(|alt| expand(&format!("{head}{alt}{tail}")))
        .collect()
}

fn segments_match(pattern: &[String], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((first, rest)) if first == "**" => {
            (0..=path.len()).any(|skip| segments_match(rest, &path[skip..]))
        }
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(part, tail)| segment_matches(first, part) && segments_match(rest, tail)),
    }
}

/// Matches one segment against a pattern whose wildcards are `*` and `?`.
fn segment_matches(pattern: &str, text: &str) -> bool {
    let (pattern, text) = (pattern.as_bytes(), text.as_bytes());
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&b| b == b'*')
}

#[cfg(test)]
#[path = "glob_tests.rs"]
mod tests;
