use std::fmt;

use laft_core::{Dep, Name, Timestamp, Variant};

use crate::explain::{Chain, Learned, Package, date, list};

/// How many learned constraints an S009 message lists.
const LEARNED_SHOWN: usize = 10;

/// A resolution that failed, with everything needed to explain why.
///
/// Its codes start with `S`, from `S001` to `S013`, so that they never
/// collide with the codes of other Laft tools.
///
/// Its [`Display`](fmt::Display) form is the full message, for example:
///
/// ```text
/// error[S003]: view "system": nothing provides so:libicuuc.so.76
///   needed by libxml2 2.14.2-r1
///     needed by gtk3 3.24.49-r1 (so:libxml2.so.2)
///       requested at /etc/laft/system.stav:9:5
///   hint: the repository is missing a package or is mid-update; run `laft repo sync` later
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The view being resolved.
    pub view: String,
    /// What went wrong.
    pub kind: Box<ErrorKind>,
}

/// What made a resolution fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// S001: no candidate matches a request.
    NoCandidate {
        /// The request as written.
        request: String,
        /// Where the request was made.
        origin: String,
        /// Every candidate of the requested name, best first.
        available: Vec<Package>,
        /// The packages that replace the requested name, when the request
        /// cannot follow the rename: several packages replace the name, or
        /// the request has a version prefix, which the replacing package's
        /// versions need not share.
        replaced_by: Vec<Name>,
    },
    /// S002: requests for the same name disagree on the variant or on the
    /// version prefix.
    ConflictingRequests {
        /// The requested name.
        name: Name,
        /// Each request as written and where it was made, sorted.
        requests: Vec<(String, String)>,
        /// Whether the variants differ, rather than the prefixes.
        variants: bool,
    },
    /// S003: nothing satisfies a dependency.
    Unsatisfied {
        /// The dependency.
        dep: Dep,
        /// The package that has it.
        by: Package,
        /// How that package got into the view.
        chain: Chain,
        /// Every candidate of the depended-on name, for a dependency on a
        /// package name.
        available: Vec<Package>,
    },
    /// S004: several packages provide a dependency and nothing says which
    /// one to use.
    AmbiguousProvider {
        /// The dependency.
        dep: Dep,
        /// The names that could provide it, sorted.
        providers: Vec<Name>,
        /// The package that has the dependency.
        by: Package,
        /// How that package got into the view.
        chain: Chain,
    },
    /// S005: the constraints on a name leave no candidate.
    Unsatisfiable {
        /// The constrained name.
        name: Name,
        /// Every constraint and where it comes from.
        constraints: Vec<Bound>,
        /// Every candidate of the name, best first.
        available: Vec<Package>,
    },
    /// S006: two selected packages conflict, or one replaces the other.
    Conflict {
        /// The package that declares the conflict or replacement.
        package: Package,
        /// How it got into the view.
        package_chain: Chain,
        /// The package it conflicts with or replaces.
        other: Package,
        /// How that one got into the view.
        other_chain: Chain,
        /// Whether `package` replaces `other`, rather than conflicting with
        /// it.
        replaces: bool,
    },
    /// S007: the only packages that provide a dependency are selected in a
    /// version or variant that does not provide it.
    WrongProvider {
        /// The dependency.
        dep: Dep,
        /// The package that has the dependency.
        by: Package,
        /// A candidate that would provide it.
        provider: Package,
        /// The selected package of the same name.
        selected: Package,
        /// How the selected package got into the view.
        chain: Chain,
    },
    /// S008: several repositories of the same, highest priority list a name
    /// that the resolution needs.
    EqualPriority {
        /// The package name.
        name: Name,
        /// The repositories, sorted.
        repos: Vec<Name>,
        /// Their priority.
        priority: i64,
    },
    /// S009: the search gave up after a bounded number of passes without
    /// finding a resolution or meeting a failure. The message lists only
    /// the first few learned constraints.
    RestartLimit {
        /// The number of passes it made.
        passes: usize,
        /// The constraints it was searching with when it gave up, in the
        /// order they were learned.
        learned: Vec<Learned>,
    },
    /// S010: the installed package was yanked and no other candidate
    /// satisfies what needs it.
    Yanked {
        /// The yanked installed package.
        package: Package,
        /// Why it was yanked.
        reason: String,
        /// When it was yanked.
        time: Timestamp,
        /// What no other candidate satisfies, as written.
        wanted: String,
        /// The package whose dependency needs it, `None` for a request.
        by: Option<Package>,
        /// How the request or the depending package got into the view.
        chain: Chain,
    },
    /// S011: the selection needs a package nobody publishes, and local
    /// builds are not allowed.
    NothingPublished {
        /// The package name.
        name: Name,
        /// The values the build would use, as `option=value` texts, sorted
        /// by option name. Empty when it would be built with the defaults.
        values: Vec<String>,
        /// Where each value that deviates from the declared defaults comes
        /// from.
        sources: Vec<(String, ValueOrigin)>,
        /// The published candidates that match on version and variant, with
        /// the option values they were built with.
        published: Vec<(Package, String)>,
    },
    /// S012: two dependents in one view need option values that cannot
    /// hold together.
    OptionConflict {
        /// The constrained package name.
        name: Name,
        /// Each demand: the value as `option=value`, where it comes from,
        /// and the chain to the request.
        demands: Vec<Demand>,
    },
    /// S013: a dependency needs an option value that the configuration
    /// fixes otherwise.
    FixedConflict {
        /// The constrained package name.
        name: Name,
        /// The dependency or request that demands the value, as written.
        text: String,
        /// The package whose dependency demands it, `None` for a request.
        by: Option<Package>,
        /// How the demand got into the view, when it is selected.
        chain: Option<Chain>,
        /// The value it demands, as `option=value`.
        value: String,
        /// The value the configuration fixes, as `option=value`, and where
        /// it was set.
        fixed: (String, String),
    },
    /// The configuration chooses an alternatives member that is not in the
    /// view.
    AlternativeAbsent {
        /// The alternatives group.
        group: Name,
        /// The chosen package.
        chosen: Name,
        /// The members of the group in the view, sorted.
        present: Vec<Name>,
    },
    /// Several members of an alternatives group are in the view, neither
    /// the configuration nor a repository default chooses one, and more
    /// than one of them has the highest priority.
    AlternativeAmbiguous {
        /// The alternatives group.
        group: Name,
        /// The members of the group in the view, sorted.
        members: Vec<Name>,
    },
}

/// A version constraint on a name and where it comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    /// The constraint, for example `foo>=2` or `foo@2.1`, or a capability
    /// the name must provide, for example `pc:zlib>=1.3`.
    pub dep: Dep,
    /// Where it comes from.
    pub from: BoundSource,
}

/// Where a [`Bound`] comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundSource {
    /// A dependency of this package.
    Package(Package),
    /// A request made at this location.
    Request(String),
    /// The configuration fixed an option value at this location.
    Config(String),
}

/// Where a demanded option value comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueOrigin {
    /// The configuration set it at this location.
    Config(String),
    /// A request made at this location demands it.
    Request(String),
    /// A dependency of this package demands it, as written.
    Dependency(Package, String),
}

/// One demand on an option of a package, with the chain to its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Demand {
    /// The demanded value, as `option=value`.
    pub value: String,
    /// Where the demand comes from.
    pub origin: ValueOrigin,
    /// How the demanding package got into the view, when it is selected.
    /// `None` when the pass that failed never selected it, so the ancestry
    /// is unknown.
    pub chain: Option<Chain>,
}

impl Error {
    /// The error code, `S001` to `S013`, or `None` for alternatives errors.
    pub fn code(&self) -> Option<&'static str> {
        Some(match *self.kind {
            ErrorKind::NoCandidate { .. } => "S001",
            ErrorKind::ConflictingRequests { .. } => "S002",
            ErrorKind::Unsatisfied { .. } => "S003",
            ErrorKind::AmbiguousProvider { .. } => "S004",
            ErrorKind::Unsatisfiable { .. } => "S005",
            ErrorKind::Conflict { .. } => "S006",
            ErrorKind::WrongProvider { .. } => "S007",
            ErrorKind::EqualPriority { .. } => "S008",
            ErrorKind::RestartLimit { .. } => "S009",
            ErrorKind::Yanked { .. } => "S010",
            ErrorKind::NothingPublished { .. } => "S011",
            ErrorKind::OptionConflict { .. } => "S012",
            ErrorKind::FixedConflict { .. } => "S013",
            ErrorKind::AlternativeAbsent { .. } | ErrorKind::AlternativeAmbiguous { .. } => {
                return None;
            }
        })
    }
}

/// Pads `text` with spaces to `width` characters.
fn pad(text: &str, width: usize) -> String {
    format!("{text:width$}")
}

/// The column that follows a list of request or constraint texts: three
/// spaces after the longest, and never before column 14.
fn column<'t>(texts: impl Iterator<Item = &'t str>) -> usize {
    texts
        .map(str::len)
        .max()
        .unwrap_or(0)
        .saturating_add(3)
        .max(14)
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let view = &self.view;
        let code = self.code().unwrap_or_default();
        let head = |f: &mut fmt::Formatter<'_>| write!(f, "error[{code}]: view {view:?}: ");
        match &*self.kind {
            ErrorKind::NoCandidate {
                request,
                origin,
                available,
                replaced_by,
            } => {
                head(f)?;
                writeln!(f, "no package matches {request}")?;
                writeln!(f, "  requested at {origin}")?;
                if let [one] = replaced_by.as_slice() {
                    writeln!(f, "  replaced by {one}")?;
                    write!(
                        f,
                        "  hint: request {one} instead, with a version prefix of its own"
                    )?;
                } else if !replaced_by.is_empty() {
                    writeln!(f, "  replaced by several packages: {}", list(replaced_by))?;
                    write!(f, "  hint: request one of them")?;
                } else if available.is_empty() {
                    write!(f, "  available: none")?;
                } else {
                    write!(f, "  available: {}", list(available))?;
                }
            }
            ErrorKind::ConflictingRequests {
                name,
                requests,
                variants,
            } => {
                head(f)?;
                writeln!(f, "conflicting requests for {name}")?;
                let width = column(requests.iter().map(|(t, _)| t.as_str()));
                for (text, origin) in requests {
                    writeln!(f, "  {}at {origin}", pad(text, width))?;
                }
                let what = if *variants { "variant" } else { "version" };
                write!(f, "  hint: request one {what}")?;
            }
            ErrorKind::Unsatisfied {
                dep,
                by,
                chain,
                available,
            } => {
                head(f)?;
                writeln!(f, "nothing provides {dep}")?;
                writeln!(f, "  needed by {by}")?;
                chain.write_nested(f, 4)?;
                if !available.is_empty() {
                    writeln!(f, "  available: {}", list(available))?;
                }
                write!(
                    f,
                    "  hint: the repository is missing a package or is mid-update; run `laft repo sync` later"
                )?;
            }
            ErrorKind::AmbiguousProvider {
                dep,
                providers,
                by,
                chain,
            } => {
                head(f)?;
                writeln!(f, "several packages provide {dep}: {}", list(providers))?;
                writeln!(f, "  needed by {by}")?;
                chain.write_nested(f, 4)?;
                write!(f, "  hint: request one of them in packages")?;
            }
            ErrorKind::Unsatisfiable {
                name,
                constraints,
                available,
            } => {
                head(f)?;
                writeln!(f, "no version of {name} satisfies every constraint")?;
                let texts: Vec<String> = constraints.iter().map(|b| b.dep.to_string()).collect();
                let width = column(texts.iter().map(String::as_str));
                for (text, bound) in texts.iter().zip(constraints) {
                    match &bound.from {
                        BoundSource::Package(p) => writeln!(f, "  {}from {p}", pad(text, width))?,
                        BoundSource::Request(origin) => {
                            writeln!(f, "  {}requested at {origin}", pad(text, width))?;
                        }
                        BoundSource::Config(origin) => {
                            writeln!(f, "  {}set at {origin}", pad(text, width))?;
                        }
                    }
                }
                if available.is_empty() {
                    write!(f, "  available: none")?;
                } else {
                    write!(f, "  available: {}", list(available))?;
                }
            }
            ErrorKind::Conflict {
                package,
                package_chain,
                other,
                other_chain,
                replaces,
            } => {
                head(f)?;
                let verb = if *replaces {
                    "replaces"
                } else {
                    "conflicts with"
                };
                writeln!(f, "{package} {verb} {other}")?;
                let (a, b) = (package.name.as_str(), other.name.as_str());
                let width = a.len().max(b.len()) + 1;
                write!(f, "  {}", pad(a, width))?;
                package_chain.write_short(f)?;
                write!(f, "\n  {}", pad(b, width))?;
                other_chain.write_short(f)?;
                if *replaces {
                    write!(
                        f,
                        "\n  hint: {} takes the place of {}, so remove {} from this view",
                        package.name, other.name, other.name
                    )?;
                }
            }
            ErrorKind::WrongProvider {
                dep,
                by,
                provider,
                selected,
                chain,
            } => {
                head(f)?;
                writeln!(f, "{dep} is needed by {by}")?;
                let variants_differ = provider.variant != selected.variant;
                if variants_differ {
                    writeln!(
                        f,
                        "  {} provides it in the {} variant, but {} is selected",
                        provider.name,
                        provider.variant,
                        selected.reference()
                    )?;
                } else {
                    writeln!(
                        f,
                        "  {} provides it in {}-r{}, but {selected} is selected",
                        provider.name, provider.version, provider.rev
                    )?;
                }
                chain.write_nested(f, 4)?;
                if variants_differ
                    && selected.variant == Variant::Static
                    && matches!(dep, Dep::So(_))
                {
                    write!(
                        f,
                        "  hint: static library variants do not provide shared libraries; remove {} from this view",
                        selected.reference()
                    )?;
                } else {
                    write!(
                        f,
                        "  hint: request {} in a version or variant that provides {dep}",
                        selected.name
                    )?;
                }
            }
            ErrorKind::EqualPriority {
                name,
                repos,
                priority,
            } => {
                let both = if repos.len() == 2 { "both" } else { "all" };
                let names = match repos.split_last() {
                    Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", list(rest)),
                    _ => list(repos),
                };
                head(f)?;
                writeln!(
                    f,
                    "package {name} is in repositories {names}, {both} with priority {priority}"
                )?;
                write!(f, "  hint: give one repository a higher priority")?;
            }
            ErrorKind::RestartLimit { passes, learned } => {
                head(f)?;
                writeln!(f, "resolution did not settle after {passes} passes")?;
                for l in learned.iter().take(LEARNED_SHOWN) {
                    writeln!(f, "  {l}")?;
                }
                if learned.len() > LEARNED_SHOWN {
                    writeln!(f, "  and {} more", learned.len() - LEARNED_SHOWN)?;
                }
                write!(
                    f,
                    "  hint: request explicit versions of the packages above to narrow the search"
                )?;
            }
            ErrorKind::Yanked {
                package,
                reason,
                time,
                wanted,
                by,
                chain,
            } => {
                head(f)?;
                writeln!(
                    f,
                    "{package} was yanked and no other version matches {wanted}"
                )?;
                writeln!(f, "  yank reason: {reason} ({})", date(*time))?;
                match by {
                    Some(by) => {
                        writeln!(f, "  needed by {by}")?;
                        chain.write_nested(f, 4)?;
                    }
                    None => chain.write_nested(f, 2)?,
                }
                write!(
                    f,
                    "  hint: relax the version prefix, or wait for a fixed revision"
                )?;
            }
            ErrorKind::NothingPublished {
                name,
                values,
                sources,
                published,
            } => {
                head(f)?;
                if values.is_empty() {
                    writeln!(
                        f,
                        "no repository publishes {name} with its default option values"
                    )?;
                } else {
                    writeln!(f, "no repository publishes {name} with {}", list(values))?;
                }
                for (text, origin) in sources {
                    match origin {
                        ValueOrigin::Config(at) => writeln!(f, "  {text} set at {at}")?,
                        ValueOrigin::Request(at) => writeln!(f, "  {text} requested at {at}")?,
                        ValueOrigin::Dependency(by, dep) => {
                            writeln!(f, "  {text} needed by {by} ({dep})")?;
                        }
                    }
                }
                if !published.is_empty() {
                    let sets = published
                        .iter()
                        .map(|(p, set)| format!("{p} ({set})"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    writeln!(f, "  published: {sets}")?;
                }
                write!(
                    f,
                    "  hint: set `build.missing = build` to build it on this machine,\n        or run `laft apply --build-missing` once"
                )?;
            }
            ErrorKind::OptionConflict { name, demands } => {
                head(f)?;
                let values: Vec<&str> = demands.iter().map(|d| d.value.as_str()).collect();
                match values.as_slice() {
                    [] => writeln!(f, "no value of {name} satisfies every dependent")?,
                    values => writeln!(f, "{name} cannot have {}", values.join(" and "))?,
                }
                for demand in demands {
                    write!(f, "  {} ", demand.value)?;
                    match &demand.origin {
                        ValueOrigin::Config(at) => writeln!(f, "set at {at}")?,
                        ValueOrigin::Request(at) => writeln!(f, "requested at {at}")?,
                        ValueOrigin::Dependency(by, dep) => {
                            writeln!(f, "needed by {by} ({dep})")?;
                        }
                    }
                    if let Some(chain) = &demand.chain {
                        chain.write_nested(f, 4)?;
                    }
                }
                write!(
                    f,
                    "  hint: put one of the dependents into another view, where it can have its own {name}"
                )?;
            }
            ErrorKind::FixedConflict {
                name: _,
                text,
                by,
                chain,
                value,
                fixed,
            } => {
                head(f)?;
                match (by, chain.as_ref()) {
                    (Some(by), _) => writeln!(f, "{by} needs {text}")?,
                    (None, Some(chain)) => writeln!(f, "{text} requested at {}", chain.origin)?,
                    (None, None) => writeln!(f, "{text}")?,
                }
                writeln!(
                    f,
                    "  demands {value}, but {} is set at {}",
                    fixed.0, fixed.1
                )?;
                if let Some(chain) = &chain {
                    chain.write_nested(f, 2)?;
                }
                let option = fixed.0.split('=').next().unwrap_or_default();
                write!(
                    f,
                    "  hint: remove the setting, or set {option} with default(...) to let dependencies choose"
                )?;
            }
            ErrorKind::AlternativeAbsent {
                group,
                chosen,
                present,
            } => {
                writeln!(
                    f,
                    "error: alternatives.{group} = {chosen}, but {chosen} is not in view {view:?}"
                )?;
                write!(f, "  members present: {}", list(present))?;
            }
            ErrorKind::AlternativeAmbiguous { group, members } => {
                writeln!(
                    f,
                    "error: alternatives group {:?} has several members in view {view:?}: {}",
                    group.as_str(),
                    list(members)
                )?;
                if let Some(first) = members.first() {
                    write!(
                        f,
                        "  hint: choose one with `alternatives.{group} = {first}`"
                    )?;
                }
            }
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
