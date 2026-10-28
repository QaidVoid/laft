use std::fmt;
use std::str::FromStr;

use crate::ParseError;

/// A build configuration of a package.
///
/// One recipe can produce several variants of the same package. Variants
/// install the same paths, so a view contains at most one variant of a
/// package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Variant {
    /// Built against the system C library and linked dynamically. This is the
    /// default and is never written out in package references.
    #[default]
    Dynamic,
    /// Statically linked executables that need no shared library at runtime.
    /// Libraries of this variant install into a separate sysroot so that they
    /// never collide with the dynamic variant.
    Static,
}

impl Variant {
    /// The variant's name as written in package references.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dynamic => "dynamic",
            Self::Static => "static",
        }
    }
}

impl FromStr for Variant {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "dynamic" => Ok(Self::Dynamic),
            "static" => Ok(Self::Static),
            _ => Err(ParseError::new(
                "variant",
                s,
                "expected `dynamic` or `static`",
            )),
        }
    }
}

impl fmt::Display for Variant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
