use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;

use crate::{Name, PackageVersion, ParseError, Version, VersionPrefix};

/// A shared object name, as found in `DT_SONAME` and `DT_NEEDED`, for example
/// `libz.so.1`.
///
/// A soname is one or more ASCII letters, digits, `.`, `_`, `-` and `+`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Soname(String);

/// A pkg-config module name, for example `zlib` or `gtk+-3.0`.
///
/// It uses the same alphabet as [`Soname`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PcName(String);

fn validate_library_name(what: &'static str, s: &str) -> Result<(), ParseError> {
    if s.is_empty() {
        return Err(ParseError::new(what, s, "a name cannot be empty"));
    }
    if !s
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
    {
        return Err(ParseError::new(
            what,
            s,
            "only letters, digits, `.`, `_`, `-` and `+` are allowed",
        ));
    }
    Ok(())
}

impl Soname {
    /// The soname as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Soname {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        validate_library_name("soname", s)?;
        Ok(Self(s.to_owned()))
    }
}

impl fmt::Display for Soname {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl PcName {
    /// The module name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for PcName {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        validate_library_name("pkg-config module name", s)?;
        Ok(Self(s.to_owned()))
    }
}

impl fmt::Display for PcName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A comparison operator in a version constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    /// `>=`
    Ge,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `<`
    Lt,
    /// `=`
    Eq,
}

impl Op {
    /// The operator as written.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ge => ">=",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Lt => "<",
            Self::Eq => "=",
        }
    }
}

/// One version constraint, such as `>=1.3` or `=1.3.1-r2`.
///
/// Without a revision, the constraint compares upstream versions only and
/// ignores the revision of the candidate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Constraint {
    /// How the candidate is compared with the bound.
    pub op: Op,
    /// The upstream version of the bound.
    pub version: Version,
    /// The revision of the bound, when the constraint also compares revisions.
    pub rev: Option<NonZeroU32>,
}

impl Constraint {
    /// Whether a package of `version` and revision `rev` satisfies the
    /// constraint.
    ///
    /// Versions compare as described on [`Version`]. When the constraint
    /// carries a revision and `rev` is given, equal versions are then
    /// compared by revision. Otherwise the revision is ignored, so `<2`
    /// rejects `2-r1` and `=1.3` accepts every revision of `1.3`.
    ///
    /// ```
    /// use laft_core::Constraint;
    ///
    /// let c: Constraint = ">=1.3.1-r2".parse()?;
    /// assert!(c.matches(&"1.3.1".parse()?, Some(2)));
    /// assert!(!c.matches(&"1.3.1".parse()?, Some(1)));
    /// assert!(c.matches(&"1.3.1".parse()?, None));
    /// # Ok::<(), laft_core::ParseError>(())
    /// ```
    pub fn matches(&self, version: &Version, rev: Option<u32>) -> bool {
        let mut ordering = version.cmp(&self.version);
        if let (Some(bound), Some(rev)) = (self.rev, rev) {
            ordering = ordering.then(rev.cmp(&bound.get()));
        }
        match self.op {
            Op::Ge => ordering.is_ge(),
            Op::Le => ordering.is_le(),
            Op::Gt => ordering.is_gt(),
            Op::Lt => ordering.is_lt(),
            Op::Eq => ordering.is_eq(),
        }
    }
}

impl FromStr for Constraint {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let ops = [Op::Ge, Op::Le, Op::Gt, Op::Lt, Op::Eq];
        let (op, bound) = ops
            .into_iter()
            .find_map(|op| s.strip_prefix(op.as_str()).map(|rest| (op, rest)))
            .ok_or_else(|| {
                ParseError::new(
                    "version constraint",
                    s,
                    "expected one of `>=`, `<=`, `>`, `<` or `=` before the version",
                )
            })?;
        if bound.contains("-r") {
            let pv: PackageVersion = bound.parse()?;
            Ok(Self {
                op,
                version: pv.version().clone(),
                rev: NonZeroU32::new(pv.rev()),
            })
        } else {
            Ok(Self {
                op,
                version: bound.parse()?,
                rev: None,
            })
        }
    }
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.op.as_str(), self.version)?;
        if let Some(rev) = self.rev {
            write!(f, "-r{rev}")?;
        }
        Ok(())
    }
}

/// Which versions of a named package satisfy a dependency.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VersionReq {
    /// Any version.
    Any,
    /// Versions that match the prefix token by token, written `name@prefix`.
    Prefix(VersionPrefix),
    /// Versions that satisfy every constraint, written `name>=1.3,<2`. The
    /// list is never empty, because an empty list is written as [`Self::Any`].
    Constraints(Vec<Constraint>),
}

impl VersionReq {
    /// Whether a package of `version` and revision `rev` satisfies the
    /// requirement. Revisions are compared only by constraints that carry
    /// one, as described on [`Constraint::matches`].
    pub fn matches(&self, version: &Version, rev: u32) -> bool {
        match self {
            Self::Any => true,
            Self::Prefix(prefix) => prefix.matches(version),
            Self::Constraints(constraints) => {
                constraints.iter().all(|c| c.matches(version, Some(rev)))
            }
        }
    }
}

/// A runtime or build dependency of a package.
///
/// | Text | Satisfied by |
/// |---|---|
/// | `so:libz.so.1` | a package that provides `so:libz.so.1` |
/// | `pc:zlib>=1.3` | a package that provides `pc:zlib=<v>` with `v >= 1.3` |
/// | `virtual:java-runtime` | a package that provides `virtual:java-runtime` |
/// | `zlib` | the package named `zlib`, any version and variant |
/// | `zlib>=1.3,<2` | `zlib` with a version in range, ignoring the revision |
/// | `zlib=1.3.1-r2` | exactly that version and revision |
/// | `llvm@19` | `llvm` whose version matches the prefix `19` |
///
/// ```
/// use laft_core::{Dep, VersionReq};
///
/// let dep: Dep = "zlib>=1.3,<2".parse()?;
/// let Dep::Pkg(name, VersionReq::Constraints(constraints)) = &dep else {
///     panic!("expected a package dependency with constraints");
/// };
/// assert_eq!(name.as_str(), "zlib");
/// assert_eq!(constraints.len(), 2);
/// assert_eq!(dep.to_string(), "zlib>=1.3,<2");
/// # Ok::<(), laft_core::ParseError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Dep {
    /// A shared object, written `so:<soname>`.
    So(Soname),
    /// A pkg-config module with optional constraints on its version, written
    /// `pc:<module>[<constraints>]`. An empty list accepts any version.
    Pc(PcName, Vec<Constraint>),
    /// A virtual capability, written `virtual:<name>`.
    Virtual(Name),
    /// A package by name, written `<name>`, `<name>@<prefix>` or
    /// `<name><constraints>`.
    Pkg(Name, VersionReq),
}

/// Splits `s` at the first constraint operator.
fn split_constraints(s: &str) -> (&str, &str) {
    s.split_at(s.find(['<', '>', '=']).unwrap_or(s.len()))
}

fn parse_constraints(s: &str) -> Result<Vec<Constraint>, ParseError> {
    if s.is_empty() {
        return Ok(Vec::new());
    }
    s.split(',').map(str::parse).collect()
}

fn write_constraints(f: &mut fmt::Formatter<'_>, constraints: &[Constraint]) -> fmt::Result {
    for (i, constraint) in constraints.iter().enumerate() {
        if i > 0 {
            f.write_str(",")?;
        }
        write!(f, "{constraint}")?;
    }
    Ok(())
}

impl FromStr for Dep {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(soname) = s.strip_prefix("so:") {
            return Ok(Self::So(soname.parse()?));
        }
        if let Some(rest) = s.strip_prefix("pc:") {
            let (name, constraints) = split_constraints(rest);
            return Ok(Self::Pc(name.parse()?, parse_constraints(constraints)?));
        }
        if let Some(name) = s.strip_prefix("virtual:") {
            return Ok(Self::Virtual(name.parse()?));
        }
        if let Some((name, prefix)) = s.split_once('@') {
            return Ok(Self::Pkg(
                name.parse()?,
                VersionReq::Prefix(prefix.parse()?),
            ));
        }
        let (name, constraints) = split_constraints(s);
        let req = match parse_constraints(constraints)? {
            constraints if constraints.is_empty() => VersionReq::Any,
            constraints => VersionReq::Constraints(constraints),
        };
        Ok(Self::Pkg(name.parse()?, req))
    }
}

impl fmt::Display for Dep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::So(soname) => write!(f, "so:{soname}"),
            Self::Pc(name, constraints) => {
                write!(f, "pc:{name}")?;
                write_constraints(f, constraints)
            }
            Self::Virtual(name) => write!(f, "virtual:{name}"),
            Self::Pkg(name, VersionReq::Any) => write!(f, "{name}"),
            Self::Pkg(name, VersionReq::Prefix(prefix)) => write!(f, "{name}@{prefix}"),
            Self::Pkg(name, VersionReq::Constraints(constraints)) => {
                write!(f, "{name}")?;
                write_constraints(f, constraints)
            }
        }
    }
}

/// A capability a package offers to satisfy [`Dep`]s of other packages.
///
/// The text forms are `so:<soname>`, `pc:<module>[=<version>]` and
/// `virtual:<name>`. A package name needs no provide, because every package
/// satisfies dependencies on its own name.
///
/// ```
/// use laft_core::Provide;
///
/// let provide: Provide = "pc:zlib=1.3.1".parse()?;
/// assert_eq!(provide.to_string(), "pc:zlib=1.3.1");
/// assert!("zlib".parse::<Provide>().is_err());
/// # Ok::<(), laft_core::ParseError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Provide {
    /// A shared object, written `so:<soname>`.
    So(Soname),
    /// A pkg-config module and the version it reports, written
    /// `pc:<module>[=<version>]`.
    Pc(PcName, Option<Version>),
    /// A virtual capability, written `virtual:<name>`.
    Virtual(Name),
}

impl FromStr for Provide {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(soname) = s.strip_prefix("so:") {
            return Ok(Self::So(soname.parse()?));
        }
        if let Some(rest) = s.strip_prefix("pc:") {
            return Ok(match rest.split_once('=') {
                Some((name, version)) => Self::Pc(name.parse()?, Some(version.parse()?)),
                None => Self::Pc(rest.parse()?, None),
            });
        }
        if let Some(name) = s.strip_prefix("virtual:") {
            return Ok(Self::Virtual(name.parse()?));
        }
        Err(ParseError::new(
            "provide",
            s,
            "expected `so:`, `pc:` or `virtual:` before the name",
        ))
    }
}

impl fmt::Display for Provide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::So(soname) => write!(f, "so:{soname}"),
            Self::Pc(name, None) => write!(f, "pc:{name}"),
            Self::Pc(name, Some(version)) => write!(f, "pc:{name}={version}"),
            Self::Virtual(name) => write!(f, "virtual:{name}"),
        }
    }
}

#[cfg(test)]
#[path = "dep_tests.rs"]
mod tests;
