use std::fmt;

use laft_core::{Dep, Name, Timestamp, Variant};

use crate::explain::{Chain, Learned, Package, date, list};

/// How many learned constraints an R009 message lists.
const LEARNED_SHOWN: usize = 10;

/// A resolution that failed, with everything needed to explain why.
///
/// Its [`Display`](fmt::Display) form is the full message, for example:
///
/// ```text
/// error[R003]: view "system": nothing provides so:libicuuc.so.76
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
    /// R001: no candidate matches a request.
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
    /// R002: requests for the same name disagree on the variant or on the
    /// version prefix.
    ConflictingRequests {
        /// The requested name.
        name: Name,
        /// Each request as written and where it was made, sorted.
        requests: Vec<(String, String)>,
        /// Whether the variants differ, rather than the prefixes.
        variants: bool,
    },
    /// R003: nothing satisfies a dependency.
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
    /// R004: several packages provide a dependency and nothing says which
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
    /// R005: the constraints on a name leave no candidate.
    Unsatisfiable {
        /// The constrained name.
        name: Name,
        /// Every constraint and where it comes from.
        constraints: Vec<Bound>,
        /// Every candidate of the name, best first.
        available: Vec<Package>,
    },
    /// R006: two selected packages conflict, or one replaces the other.
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
    /// R007: the only packages that provide a dependency are selected in a
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
    /// R008: several repositories of the same, highest priority list a name
    /// that the resolution needs.
    EqualPriority {
        /// The package name.
        name: Name,
        /// The repositories, sorted.
        repos: Vec<Name>,
        /// Their priority.
        priority: i64,
    },
    /// R009: the search gave up after a bounded number of passes without
    /// finding a resolution or meeting a failure. The message lists only
    /// the first few learned constraints.
    RestartLimit {
        /// The number of passes it made.
        passes: usize,
        /// The constraints it was searching with when it gave up, in the
        /// order they were learned.
        learned: Vec<Learned>,
    },
    /// R010: the installed package was yanked and no other candidate
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
}

impl Error {
    /// The error code, `R001` to `R010`, or `None` for alternatives errors.
    pub fn code(&self) -> Option<&'static str> {
        Some(match *self.kind {
            ErrorKind::NoCandidate { .. } => "R001",
            ErrorKind::ConflictingRequests { .. } => "R002",
            ErrorKind::Unsatisfied { .. } => "R003",
            ErrorKind::AmbiguousProvider { .. } => "R004",
            ErrorKind::Unsatisfiable { .. } => "R005",
            ErrorKind::Conflict { .. } => "R006",
            ErrorKind::WrongProvider { .. } => "R007",
            ErrorKind::EqualPriority { .. } => "R008",
            ErrorKind::RestartLimit { .. } => "R009",
            ErrorKind::Yanked { .. } => "R010",
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
