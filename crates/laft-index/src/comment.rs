use std::fmt;
use std::str::FromStr;

use laft_core::Name;
use laft_pkg::Arch;

use crate::Error;

/// The trusted comment of an index signature:
/// `laft-index repo=<repo> channel=<channel> arch=<arch> generation=<n>`.
///
/// The signature covers the comment, so a client can reject a stale or
/// substituted index before decompressing it.
///
/// ```
/// use laft_index::IndexComment;
///
/// let text = "laft-index repo=core channel=current arch=x86_64-v3 generation=1761652800";
/// let comment: IndexComment = text.parse()?;
/// assert_eq!(comment.generation, 1_761_652_800);
/// assert_eq!(comment.to_string(), text);
/// # Ok::<(), laft_index::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexComment {
    /// The repository name.
    pub repo: Name,
    /// The channel.
    pub channel: Name,
    /// The architecture.
    pub arch: Arch,
    /// The index generation.
    pub generation: u64,
}

/// The trusted comment of a `keys.json` signature:
/// `laft-keys repo=<repo> version=<n>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeysComment {
    /// The repository name.
    pub repo: Name,
    /// The `keys.json` version.
    pub version: u64,
}

impl fmt::Display for IndexComment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "laft-index repo={} channel={} arch={} generation={}",
            self.repo, self.channel, self.arch, self.generation
        )
    }
}

impl FromStr for IndexComment {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let [repo, channel, arch, generation] =
            fields(s, "laft-index", ["repo", "channel", "arch", "generation"])?;
        Ok(Self {
            repo: name(s, repo)?,
            channel: name(s, channel)?,
            arch: [Arch::X86_64V3, Arch::Any]
                .into_iter()
                .find(|a| a.as_str() == arch)
                .ok_or_else(|| invalid(s))?,
            generation: number(s, generation)?,
        })
    }
}

impl fmt::Display for KeysComment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "laft-keys repo={} version={}", self.repo, self.version)
    }
}

impl FromStr for KeysComment {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let [repo, version] = fields(s, "laft-keys", ["repo", "version"])?;
        Ok(Self {
            repo: name(s, repo)?,
            version: number(s, version)?,
        })
    }
}

/// Splits `<tag> <key>=<value>...` into the values of exactly `keys`, in
/// order, separated by single spaces.
fn fields<'a, const N: usize>(
    s: &'a str,
    tag: &str,
    keys: [&str; N],
) -> Result<[&'a str; N], Error> {
    let mut words = s.split(' ');
    if words.next() != Some(tag) {
        return Err(invalid(s));
    }
    let mut values = [""; N];
    for (value, key) in values.iter_mut().zip(keys) {
        *value = words
            .next()
            .and_then(|word| word.strip_prefix(key)?.strip_prefix('='))
            .ok_or_else(|| invalid(s))?;
    }
    if words.next().is_some() {
        return Err(invalid(s));
    }
    Ok(values)
}

fn name(s: &str, value: &str) -> Result<Name, Error> {
    value.parse().map_err(|_| invalid(s))
}

/// Parses a decimal number written without sign or leading zeros.
fn number(s: &str, value: &str) -> Result<u64, Error> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| n.to_string() == value)
        .ok_or_else(|| invalid(s))
}

fn invalid(s: &str) -> Error {
    Error::invalid(
        "trusted comment",
        format!("{s:?} does not have the expected form"),
    )
}

#[cfg(test)]
#[path = "comment_tests.rs"]
mod tests;
