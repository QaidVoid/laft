use std::fmt;
use std::str::FromStr;

use crate::opt_constraint::{OptionConstraints, split_tail_group};
use crate::{Name, ParseError, Variant, VersionPrefix};

/// A reference that selects packages from a repository.
///
/// The text form is `name[@prefix][:variant]{opt=tag,...}`:
///
/// | Reference | Selects |
/// |---|---|
/// | `ripgrep` | the newest `ripgrep`, dynamic variant |
/// | `ripgrep:static` | the newest `ripgrep`, static variant |
/// | `llvm@19` | the newest `llvm` whose version starts with the token `19` |
/// | `llvm@19:static` | both constraints |
/// | `mesa{llvm,!x11}` | the newest `mesa` with `llvm` on and `x11` off |
/// | `mesa{vulkan=amd}` | the newest `mesa` with `vulkan=amd` |
///
/// The dynamic variant is the default and is never printed, and empty
/// option constraints are never printed.
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
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PkgRef {
    name: Name,
    prefix: Option<VersionPrefix>,
    variant: Variant,
    options: OptionConstraints,
}

impl fmt::Debug for PkgRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PkgRef({self})")
    }
}

impl PkgRef {
    /// Builds a reference from its parts, without option constraints.
    pub fn new(name: Name, prefix: Option<VersionPrefix>, variant: Variant) -> Self {
        Self {
            name,
            prefix,
            variant,
            options: OptionConstraints::default(),
        }
    }

    /// Builds a reference from its parts and its option constraints.
    pub fn new_with_options(
        name: Name,
        prefix: Option<VersionPrefix>,
        variant: Variant,
        options: OptionConstraints,
    ) -> Self {
        Self {
            name,
            prefix,
            variant,
            options,
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

    /// The option constraints of the reference, which may be empty.
    pub fn options(&self) -> &OptionConstraints {
        &self.options
    }
}

impl FromStr for PkgRef {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (rest, options) = match split_tail_group(s) {
            (rest, Some(group)) => (rest, group.parse()?),
            (rest, None) => (rest, OptionConstraints::default()),
        };
        let (rest, variant) = match rest.split_once(':') {
            Some((rest, variant)) => (rest, variant.parse()?),
            None => (rest, Variant::default()),
        };
        let (name, prefix) = match rest.split_once('@') {
            Some((name, prefix)) => (name, Some(prefix.parse()?)),
            None => (rest, None),
        };
        Ok(Self::new_with_options(
            name.parse()?,
            prefix,
            variant,
            options,
        ))
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
        write!(f, "{}", self.options)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "pkgref_tests.rs"]
mod tests;
