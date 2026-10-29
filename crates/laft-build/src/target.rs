//! The machine and variant a recipe is evaluated for, and the `laft/target`
//! host module that exposes it to recipes.

use laft_core::Variant;
use stav::{HostValues, Value};

use crate::host::TARGET_MODULE;

/// The architecture every package is built for.
pub const ARCH: &str = "x86_64-v3";

/// The value of `-march` and `target-cpu` for [`ARCH`].
pub const CPU: &str = "x86-64-v3";

/// The triple of the build machine and of the `dynamic` variant.
pub const GNU_TRIPLE: &str = "x86_64-laft-linux-gnu";

/// The triple of the `static` variant, which is cross compiled against musl.
pub const MUSL_TRIPLE: &str = "x86_64-laft-linux-musl";

/// The musl sysroot that the `static` variant installs into and links
/// against.
pub const SYSROOT: &str = "/usr/x86_64-laft-linux-musl";

/// The target of one evaluation of a recipe: [`ARCH`] and a variant.
///
/// Recipes see it by importing the host module `laft/target`, which binds
/// `variant`, `arch`, `cpu`, `triple` and `libc`:
///
/// ```
/// use laft_build::Target;
/// use laft_core::Variant;
///
/// let target = Target::new(Variant::Static);
/// assert_eq!(target.triple(), "x86_64-laft-linux-musl");
/// assert!(target.module_source().contains("let libc: Libc = musl"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Target {
    variant: Variant,
}

impl Target {
    /// The target that builds `variant` for [`ARCH`].
    pub fn new(variant: Variant) -> Self {
        Self { variant }
    }

    /// The variant being built.
    pub fn variant(self) -> Variant {
        self.variant
    }

    /// The triple of the machine the outputs run on: [`GNU_TRIPLE`] for
    /// `dynamic`, [`MUSL_TRIPLE`] for `static`.
    pub fn triple(self) -> &'static str {
        match self.variant {
            Variant::Dynamic => GNU_TRIPLE,
            Variant::Static => MUSL_TRIPLE,
        }
    }

    /// The C library the outputs link against, `glibc` or `musl`.
    pub fn libc(self) -> &'static str {
        match self.variant {
            Variant::Dynamic => "glibc",
            Variant::Static => "musl",
        }
    }

    /// The values of the `laft/target` module for this target, which one
    /// checked recipe is evaluated with for every variant.
    pub fn values(self) -> HostValues {
        let text = |s: &str| Value::Str(s.to_owned());
        HostValues::new()
            .with(TARGET_MODULE, "variant", text(&self.variant.to_string()))
            .with(TARGET_MODULE, "triple", text(self.triple()))
            .with(TARGET_MODULE, "libc", text(self.libc()))
    }

    /// The source of the `laft/target` module for this target. The types
    /// `Variant` and `Libc` come from the recipe prelude.
    pub fn module_source(self) -> String {
        format!(
            "## The variant being built.\nlet variant: Variant = {}\n\
             ## The architecture being built for.\nlet arch: Str = \"{ARCH}\"\n\
             ## The CPU the compiler optimizes for.\nlet cpu: Str = \"{CPU}\"\n\
             ## The triple of the machine the outputs run on.\nlet triple: Str = \"{}\"\n\
             ## The C library the outputs link against.\nlet libc: Libc = {}\n",
            self.variant,
            self.triple(),
            self.libc(),
        )
    }
}
