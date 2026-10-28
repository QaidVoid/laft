//! Dependency resolution of Laft views.
//!
//! A view contains exactly one version of each package name. The resolver
//! turns the requests of one view into that exact set, chosen from the
//! indexes of the enabled repositories. It is pure: callers hand it the
//! accepted indexes and the installed packages, and it does no I/O.
//!
//! 1. [`Universe::new`] prepares the indexes once per command. For each
//!    name, only the repository with the highest priority is used.
//! 2. [`Universe::resolve`] resolves one view from a [`ResolveInput`]: the
//!    requests, the alternatives choices, the installed packages and the
//!    upgrade mode. The initrd closure, which belongs to no view, is
//!    resolved the same way under the name `initrd`.
//! 3. The [`Resolution`] lists one [`Selected`] candidate per name, with the
//!    reasons it is there. [`Resolution::why`] explains a package, and an
//!    [`Error`] explains a failure with the chain back to a request.
//!
//! Selection is greedy and deterministic. Without an upgrade, installed
//! versions are kept whenever they still satisfy everything, including
//! versions a repository pruned from its index. Versions a repository
//! yanked are never selected. When a dependency contradicts an earlier
//! choice, the resolver learns the dependency's constraint and starts over.
//!
//! ```
//! use laft_core::Timestamp;
//! use laft_index::{Defaults, Index, IndexEntry};
//! use laft_pkg::{Arch, Meta};
//! use laft_resolve::{Repository, ResolveInput, Request, Universe};
//!
//! fn entry(name: &str, depends: &[&str], provides: &[&str]) -> IndexEntry {
//!     let meta = Meta {
//!         format: laft_pkg::FORMAT,
//!         name: name.parse().unwrap(),
//!         version: "1.0".parse().unwrap(),
//!         rev: 1,
//!         variant: Default::default(),
//!         arch: Arch::X86_64V3,
//!         origin: name.parse().unwrap(),
//!         summary: String::new(),
//!         homepage: None,
//!         license: vec![],
//!         depends: depends.iter().map(|d| d.parse().unwrap()).collect(),
//!         provides: provides.iter().map(|p| p.parse().unwrap()).collect(),
//!         conflicts: vec![],
//!         replaces: vec![],
//!         alternatives: vec![],
//!         triggers: vec![],
//!         static_deps: vec![],
//!         abi: Default::default(),
//!         abi_epoch: 0,
//!         requested_privileges: vec![],
//!         installed_size: 0,
//!     };
//!     let id = laft_core::StoreId::from_manifest_hash(&laft_core::Blake3Hash::of(name.as_bytes()));
//!     IndexEntry { meta, id, file: format!("pool/{id}.lpkg"), size: 0, blake3: laft_core::Blake3Hash::of(b"") }
//! }
//!
//! let index = Index {
//!     format: Index::FORMAT,
//!     repo: "core".parse()?,
//!     channel: "current".parse()?,
//!     arch: Arch::X86_64V3,
//!     generation: 1,
//!     timestamp: Timestamp::from_secs(0),
//!     expires: Timestamp::from_secs(86_400),
//!     defaults: Defaults::default(),
//!     packages: vec![
//!         entry("ripgrep", &["so:libpcre2-8.so.0"], &[]),
//!         entry("pcre2", &[], &["so:libpcre2-8.so.0"]),
//!     ],
//!     yanked: vec![],
//! };
//! let universe = Universe::new(&[Repository { index: &index, priority: 0 }]);
//! let mut input = ResolveInput::new("system");
//! input.requests.push(Request {
//!     pkg: "ripgrep".parse()?,
//!     origin: "/etc/laft/system.stav:8:22".into(),
//! });
//! let resolution = universe.resolve(&input)?;
//! assert_eq!(resolution.packages.len(), 2);
//! assert_eq!(
//!     resolution.why(&"pcre2".parse()?).unwrap().to_string(),
//!     "pcre2 1.0-r1 is in view \"system\" because:\n  \
//!      ripgrep 1.0-r1 needs so:libpcre2-8.so.0\n    \
//!      requested at /etc/laft/system.stav:8:22 (ripgrep)\n"
//! );
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]

mod error;
mod explain;
mod resolve;
#[cfg(test)]
mod test_support;
mod universe;

pub use error::{Bound, BoundSource, Error, ErrorKind};
pub use explain::{Chain, Learned, Package, Reason, Step, Warning, Why};
pub use resolve::{Installed, Request, Resolution, ResolveInput, Selected, Upgrade};
pub use universe::{Candidate, Repository, Source, Universe};
