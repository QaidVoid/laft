//! Transport, mirror failover and download cache for Laft repositories.
//!
//! A repository is a tree of static files below a base URL ([`BaseUrl`]),
//! reached over `https://`, over a local `file://` directory, or over plain
//! `http://` when the repository allows it. [`Mirrors`] orders the URLs of
//! one repository: the configured URL, then the configured mirrors.
//!
//! A [`Fetcher`] moves bytes and caches them. [`Fetcher::fetch`] reads small
//! signed documents into memory with a size limit per [`ObjectKind`].
//! [`Fetcher::download_package`] makes a package available as a local file
//! whose size and BLAKE3 hash match its index entry, resuming interrupted
//! downloads and failing over between mirrors. [`Fetcher::sync`] syncs one
//! repository channel: it downloads `keys.json` and the index, hands them
//! to `laft-index` for every trust decision, and falls back to the cached
//! index when no URL works.
//!
//! Every error names the URL and the reason. Nothing is printed. Progress
//! goes to a callback.
//!
//! ```no_run
//! use laft_core::Timestamp;
//! use laft_fetch::{Config, Fetcher, Mirrors, Source, SyncRequest};
//! use laft_index::{IndexState, Repo};
//!
//! let fetcher = Fetcher::new(Config::new("/laft/cache"));
//! let state = IndexState::new("/laft/state/index");
//! let repo = Repo {
//!     name: "core".parse()?,
//!     channel: "current".parse()?,
//!     arch: laft_pkg::Arch::X86_64V3,
//!     key: "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3".parse()?,
//!     keys: vec![],
//! };
//! let mirrors = Mirrors::new("https://pkgs.example.org/laft", &[] as &[&str], false)?;
//! let request = SyncRequest {
//!     repo: &repo,
//!     mirrors: &mirrors,
//!     state: &state,
//!     now: Timestamp::now(),
//!     allow_expired: false,
//! };
//! let synced = fetcher.sync(&request, &mut |_| {})?;
//! let mirrors = match &synced.source {
//!     Source::Mirror { base, .. } => mirrors.preferring(base),
//!     Source::Cache { .. } => mirrors,
//! };
//! for entry in &synced.index.index.packages {
//!     let path = fetcher.download_package(&mirrors, entry, &mut |p| {
//!         let _ = (p.done, p.total);
//!     })?;
//!     println!("{}", path.display());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]

mod error;
mod fetch;
mod fs;
mod sync;
mod url;

#[cfg(test)]
mod test_support;

pub use error::{Error, ErrorKind};
pub use fetch::{Config, Fetcher, ObjectKind, Progress};
pub use sync::{Source, SyncRequest, Synced};
pub use url::{BaseUrl, Mirrors, Scheme};
