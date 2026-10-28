use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use laft_core::{Dep, Name, Timestamp, Variant, Version, VersionReq};
use laft_pkg::Meta;

/// A package as people read it in messages: `ripgrep 14.1.1-r1`, or
/// `ripgrep:static 14.1.1-r1` for the static variant.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Package {
    /// The package name.
    pub name: Name,
    /// The upstream version.
    pub version: Version,
    /// The revision.
    pub rev: u32,
    /// The variant.
    pub variant: Variant,
}

impl Package {
    /// The label of the package described by `meta`.
    pub fn of(meta: &Meta) -> Self {
        Self {
            name: meta.name.clone(),
            version: meta.version.clone(),
            rev: meta.rev,
            variant: meta.variant,
        }
    }

    /// The name with the variant suffix, for example `zlib:static`.
    pub(crate) fn reference(&self) -> String {
        match self.variant {
            Variant::Dynamic => self.name.to_string(),
            variant => format!("{}:{variant}", self.name),
        }
    }
}

impl fmt::Display for Package {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}-r{}", self.reference(), self.version, self.rev)
    }
}

/// Why a package is in a resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// A request of the view asked for it.
    Requested {
        /// Where the request was made, for example
        /// `/etc/laft/system.stav:8:22`.
        origin: String,
        /// The request as written, for example `llvm@19`.
        text: String,
    },
    /// Another selected package depends on it.
    Dependency {
        /// The name of the depending package.
        by: Name,
        /// The dependency this package satisfies, for example
        /// `so:libz.so.1`.
        dep: Dep,
    },
}

/// A version constraint the resolver learned when a dependency contradicted
/// an earlier choice, and restarted with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Learned {
    /// The constrained package name.
    pub name: Name,
    /// The versions the name is limited to.
    pub req: VersionReq,
    /// The package whose dependency imposed the constraint.
    pub by: Package,
}

impl Learned {
    /// The constraint as a dependency, for example `foo<2`.
    pub fn dep(&self) -> Dep {
        Dep::Pkg(self.name.clone(), self.req.clone())
    }
}

impl fmt::Display for Learned {
    /// Writes `learned foo<2 from bar 1.0-r1`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "learned {} from {}", self.dep(), self.by)
    }
}

/// Something worth telling the user about a successful resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// W001: a request named a package that no repository has any more, and
    /// exactly one package replaces it, so the request now selects that
    /// package.
    Renamed {
        /// The requested name.
        from: Name,
        /// The replacing package.
        to: Name,
        /// Where the request was made.
        origin: String,
    },
    /// W002: an installed package is in no repository, and the resolution
    /// keeps it.
    Orphaned {
        /// The kept package.
        package: Package,
    },
    /// W003: the installed version was yanked, so the resolution moves to
    /// another one.
    Yanked {
        /// The yanked installed package.
        package: Package,
        /// Why it was yanked.
        reason: String,
        /// The package selected instead.
        replacement: Package,
    },
}

impl Warning {
    /// The warning code, `W001` to `W003`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Renamed { .. } => "W001",
            Self::Orphaned { .. } => "W002",
            Self::Yanked { .. } => "W003",
        }
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "warning[{}]: ", self.code())?;
        match self {
            Self::Renamed { from, to, origin } => write!(
                f,
                "{from} was renamed to {to} ({to} replaces {from}); update {origin}"
            ),
            Self::Orphaned { package } => write!(
                f,
                "{package} is no longer in any repository; keeping the installed version"
            ),
            Self::Yanked {
                package,
                reason,
                replacement,
            } => write!(
                f,
                "{package} was yanked ({reason}); moving to {replacement}"
            ),
        }
    }
}

/// One package on the way from a package to the request that pulled it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The depending package.
    pub package: Package,
    /// What it needs from the package before it in the chain.
    pub dep: Dep,
}

/// The shortest path from a package up to a request: the packages that
/// depend on it in turn, ending at a requested one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    /// The depending packages, nearest first. Empty when the package itself
    /// was requested.
    pub steps: Vec<Step>,
    /// Where the request at the end of the chain was made.
    pub origin: String,
    /// The request as written.
    pub request: String,
}

impl Chain {
    /// Finds the shortest chain from `start` by a breadth-first search over
    /// dependency reasons. `reasons` looks up a selected package. Ties are
    /// broken by the order of the reasons, so the result is deterministic.
    pub(crate) fn find<'r>(
        start: &Name,
        reasons: impl Fn(&Name) -> Option<(Package, &'r [Reason])>,
    ) -> Option<Self> {
        let mut parent: BTreeMap<Name, (Name, Dep)> = BTreeMap::new();
        let mut seen = BTreeSet::from([start.clone()]);
        let mut queue = VecDeque::from([start.clone()]);
        while let Some(name) = queue.pop_front() {
            let (_, list) = reasons(&name)?;
            for reason in list {
                match reason {
                    Reason::Requested { origin, text } => {
                        let mut steps = Vec::new();
                        let mut at = name;
                        while let Some((child, dep)) = parent.get(&at) {
                            steps.push(Step {
                                package: reasons(&at)?.0,
                                dep: dep.clone(),
                            });
                            at = child.clone();
                        }
                        steps.reverse();
                        return Some(Self {
                            steps,
                            origin: origin.clone(),
                            request: text.clone(),
                        });
                    }
                    Reason::Dependency { by, dep } => {
                        if seen.insert(by.clone()) {
                            parent.insert(by.clone(), (name.clone(), dep.clone()));
                            queue.push_back(by.clone());
                        }
                    }
                }
            }
        }
        None
    }

    /// Writes one line per step, each indented two spaces more than the
    /// one before, starting at `indent` spaces.
    pub(crate) fn write_nested(&self, f: &mut fmt::Formatter<'_>, indent: usize) -> fmt::Result {
        let mut indent = indent;
        for step in &self.steps {
            writeln!(f, "{:indent$}needed by {} ({})", "", step.package, step.dep)?;
            indent += 2;
        }
        writeln!(f, "{:indent$}requested at {}", "", self.origin)
    }

    /// Writes the chain on one line: the nearest depending package and the
    /// request's location.
    pub(crate) fn write_short(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(step) = self.steps.first() {
            write!(f, "needed by {} ({}), ", step.package, step.dep)?;
        }
        write!(f, "requested at {}", self.origin)
    }
}

/// Why a package is in a view: the shortest chain to a request and the
/// other packages that need it.
///
/// Displayed, it reads:
///
/// ```text
/// libpcre2 10.45-r1 is in view "system" because:
///   ripgrep 14.1.1-r1 needs so:libpcre2-8.so.0
///     requested at /etc/laft/system.stav:8:22 (ripgrep)
/// also needed by: git 2.51.0-r1, grep 3.12-r1
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Why {
    /// The view name.
    pub view: String,
    /// The explained package.
    pub package: Package,
    /// The shortest chain to a request.
    pub chain: Chain,
    /// Other packages that depend on it directly, sorted.
    pub also: Vec<Package>,
}

impl fmt::Display for Why {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} is in view {:?} because:", self.package, self.view)?;
        let mut indent = 2;
        for step in &self.chain.steps {
            writeln!(f, "{:indent$}{} needs {}", "", step.package, step.dep)?;
            indent += 2;
        }
        writeln!(
            f,
            "{:indent$}requested at {} ({})",
            "", self.chain.origin, self.chain.request
        )?;
        if !self.also.is_empty() {
            writeln!(f, "also needed by: {}", list(&self.also))?;
        }
        Ok(())
    }
}

/// Joins items with `, `.
pub(crate) fn list<T: fmt::Display>(items: &[T]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The date part of a timestamp, `YYYY-MM-DD`.
pub(crate) fn date(time: Timestamp) -> String {
    let mut text = time.to_string();
    text.truncate(10);
    text
}

#[cfg(test)]
#[path = "explain_tests.rs"]
mod tests;
