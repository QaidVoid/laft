use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use crate::ParseError;

/// An upstream version string, such as `14.1.1`, `2.0~rc1` or `2024.01`.
///
/// A version is 1 to 128 bytes of ASCII letters, digits, `.`, `_` and `~`, and
/// it is kept exactly as written.
///
/// # Ordering
///
/// The version is split into tokens: maximal runs of digits, maximal runs of
/// letters, and `~`. The separators `.` and `_` produce no token. Tokens are
/// compared pairwise:
///
/// - `~` sorts before everything, including the end of the version, so
///   `2.0~rc1 < 2.0`.
/// - Digit runs compare as numbers, ignoring leading zeros, so `1.2 < 1.10`.
/// - A digit run sorts after a letter run, so `1.0a < 1.0.1`.
/// - Letter runs compare bytewise.
/// - A version with more tokens sorts after one that is otherwise equal, so
///   `1.0 < 1.0.0` and `1.0 < 1.0a`.
///
/// When all tokens are equal, the raw bytes decide, so the order is total and
/// two versions are equal only if they are the same string.
///
/// ```
/// use laft_core::Version;
///
/// let v = |s: &str| s.parse::<Version>().unwrap();
/// assert!(v("2.0~rc1") < v("2.0"));
/// assert!(v("6.17.3") < v("6.17.10"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version(String);

/// The longest version string, in bytes.
const MAX_LEN: usize = 128;

impl Version {
    /// The version as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate(what: &'static str, s: &str) -> Result<(), ParseError> {
    let err = |reason| ParseError::new(what, s, reason);
    if s.is_empty() {
        return Err(err("a version cannot be empty"));
    }
    if s.len() > MAX_LEN {
        return Err(err("a version is at most 128 bytes long"));
    }
    if !s
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~'))
    {
        return Err(err("only letters, digits, `.`, `_` and `~` are allowed"));
    }
    Ok(())
}

impl FromStr for Version {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        validate("version", s)?;
        Ok(Self(s.to_owned()))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let mut a = Tokens::new(&self.0);
        let mut b = Tokens::new(&other.0);
        loop {
            let ordering = match (a.next(), b.next()) {
                (None, None) => break,
                (Some(Token::Tilde), Some(Token::Tilde)) => Ordering::Equal,
                (Some(Token::Tilde), _) => return Ordering::Less,
                (_, Some(Token::Tilde)) => return Ordering::Greater,
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(Token::Num(x)), Some(Token::Num(y))) => cmp_digits(x, y),
                (Some(Token::Num(_)), Some(Token::Alpha(_))) => return Ordering::Greater,
                (Some(Token::Alpha(_)), Some(Token::Num(_))) => return Ordering::Less,
                (Some(Token::Alpha(x)), Some(Token::Alpha(y))) => x.cmp(y),
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        self.0.as_bytes().cmp(other.0.as_bytes())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A version prefix, as written after `@` in a package reference.
///
/// A prefix matches a version when its tokens equal the version's first tokens,
/// using the tokens described on [`Version`]. Matching is token-wise, never
/// character-wise: `19.1` matches `19.1.7` but not `19.10.0`, and `2` does not
/// match `20.1.0`.
///
/// ```
/// use laft_core::{Version, VersionPrefix};
///
/// let prefix: VersionPrefix = "19.1".parse()?;
/// assert!(prefix.matches(&"19.1.7".parse::<Version>()?));
/// assert!(!prefix.matches(&"19.10.0".parse::<Version>()?));
/// # Ok::<(), laft_core::ParseError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VersionPrefix(String);

impl VersionPrefix {
    /// The prefix as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `version` starts with this prefix, token by token.
    pub fn matches(&self, version: &Version) -> bool {
        let mut v = Tokens::new(&version.0);
        Tokens::new(&self.0).all(|p| match (p, v.next()) {
            (Token::Tilde, Some(Token::Tilde)) => true,
            (Token::Num(x), Some(Token::Num(y))) => cmp_digits(x, y) == Ordering::Equal,
            (Token::Alpha(x), Some(Token::Alpha(y))) => x == y,
            _ => false,
        })
    }

    /// Whether every version matched by `self` is also matched by `other`,
    /// which holds when `other`'s tokens are a prefix of `self`'s.
    pub fn is_within(&self, other: &VersionPrefix) -> bool {
        other.matches(&Version(self.0.clone()))
    }
}

impl FromStr for VersionPrefix {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        validate("version prefix", s)?;
        if Tokens::new(s).next().is_none() {
            return Err(ParseError::new(
                "version prefix",
                s,
                "a prefix needs at least one letter, digit or `~`",
            ));
        }
        Ok(Self(s.to_owned()))
    }
}

impl fmt::Display for VersionPrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A package version: the upstream [`Version`] plus Laft's revision number.
///
/// The revision starts at 1 and counts rebuilds of the same upstream version
/// with a changed recipe. The text form is `<version>-r<rev>`, for example
/// `14.1.1-r1`. Package versions order by version first, then by revision.
///
/// ```
/// use laft_core::PackageVersion;
///
/// let pv: PackageVersion = "14.1.1-r2".parse()?;
/// assert_eq!(pv.rev(), 2);
/// assert!(pv > "14.1.1-r1".parse()?);
/// # Ok::<(), laft_core::ParseError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageVersion {
    version: Version,
    rev: u32,
}

impl PackageVersion {
    /// Combines an upstream version and a revision.
    ///
    /// # Errors
    ///
    /// Fails when `rev` is 0, because revisions start at 1.
    pub fn new(version: Version, rev: u32) -> Result<Self, ParseError> {
        if rev == 0 {
            return Err(ParseError::new(
                "package version",
                &format!("{version}-r0"),
                "revisions start at 1",
            ));
        }
        Ok(Self { version, rev })
    }

    /// The upstream version.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// The revision, starting at 1.
    pub fn rev(&self) -> u32 {
        self.rev
    }
}

impl FromStr for PackageVersion {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = |reason| ParseError::new("package version", s, reason);
        let (version, rev) = s
            .rsplit_once("-r")
            .ok_or_else(|| err("expected `<version>-r<revision>`"))?;
        if rev.is_empty() || !rev.bytes().all(|b| b.is_ascii_digit()) || rev.starts_with('0') {
            return Err(err("the revision is a number starting at 1"));
        }
        let rev = rev
            .parse()
            .map_err(|_| err("the revision does not fit in 32 bits"))?;
        Self::new(version.parse()?, rev)
    }
}

impl fmt::Display for PackageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-r{}", self.version, self.rev)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Tilde,
    Num(&'a str),
    Alpha(&'a str),
}

struct Tokens<'a> {
    rest: &'a str,
}

impl<'a> Tokens<'a> {
    fn new(s: &'a str) -> Self {
        Self { rest: s }
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        self.rest = self.rest.trim_start_matches(['.', '_']);
        let first = *self.rest.as_bytes().first()?;
        let len = if first == b'~' {
            1
        } else if first.is_ascii_digit() {
            self.rest.bytes().take_while(u8::is_ascii_digit).count()
        } else {
            // At least one byte, so a byte outside the version alphabet cannot
            // stall the iterator.
            self.rest
                .bytes()
                .take_while(u8::is_ascii_alphabetic)
                .count()
                .max(1)
        };
        let (token, rest) = self.rest.split_at(len);
        self.rest = rest;
        Some(if first == b'~' {
            Token::Tilde
        } else if first.is_ascii_digit() {
            Token::Num(token)
        } else {
            Token::Alpha(token)
        })
    }
}

fn cmp_digits(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

#[cfg(test)]
#[path = "version_tests.rs"]
mod tests;
