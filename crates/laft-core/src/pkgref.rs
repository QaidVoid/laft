use std::fmt;
use std::str::FromStr;

use crate::{Name, ParseError, Variant, VersionPrefix};

/// A reference that selects packages from a repository.
///
/// The text form is `name[@prefix][:variant]`:
///
/// | Reference | Selects |
/// |---|---|
/// | `ripgrep` | the newest `ripgrep`, dynamic variant |
/// | `ripgrep:static` | the newest `ripgrep`, static variant |
/// | `llvm@19` | the newest `llvm` whose version starts with the token `19` |
/// | `llvm@19:static` | both constraints |
///
/// The dynamic variant is the default and is never printed.
///
/// ```
/// use laft_core::{PkgRef, Variant};
///
/// let r: PkgRef = "llvm@19:static".parse()?;
/// assert_eq!(r.name().as_str(), "llvm");
/// assert_eq!(r.variant(), Variant::Static);
/// assert_eq!(r.to_string(), "llvm@19:static");
/// # Ok::<(), laft_core::ParseError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PkgRef {
    name: Name,
    prefix: Option<VersionPrefix>,
    variant: Variant,
}

impl PkgRef {
    /// Builds a reference from its parts.
    pub fn new(name: Name, prefix: Option<VersionPrefix>, variant: Variant) -> Self {
        Self {
            name,
            prefix,
            variant,
        }
    }

    /// The referenced package name.
    pub fn name(&self) -> &Name {
        &self.name
    }

    /// The version prefix, if the reference constrains the version.
    pub fn prefix(&self) -> Option<&VersionPrefix> {
        self.prefix.as_ref()
    }

    /// The selected variant.
    pub fn variant(&self) -> Variant {
        self.variant
    }
}

impl FromStr for PkgRef {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (rest, variant) = match s.split_once(':') {
            Some((rest, variant)) => (rest, variant.parse()?),
            None => (s, Variant::default()),
        };
        let (name, prefix) = match rest.split_once('@') {
            Some((name, prefix)) => (name, Some(prefix.parse()?)),
            None => (rest, None),
        };
        Ok(Self::new(name.parse()?, prefix, variant))
    }
}

impl fmt::Display for PkgRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name.as_str())?;
        if let Some(prefix) = &self.prefix {
            write!(f, "@{prefix}")?;
        }
        if self.variant != Variant::Dynamic {
            write!(f, ":{}", self.variant)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "pkgref_tests.rs"]
mod tests;
