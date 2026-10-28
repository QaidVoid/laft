//! Transport, mirror failover and download cache for Laft repositories.
//!
//! A repository is a tree of static files below a base URL ([`BaseUrl`]),
//! reached over `https://`, over a local `file://` directory, or over plain
//! `http://` when the repository allows it. [`Mirrors`] orders the URLs of
//! one repository: the configured URL, the configured mirrors, then the
//! mirrors of its verified signed mirror list.
//!
//! A [`Fetcher`] moves bytes and caches them. [`Fetcher::fetch`] reads small
//! signed documents into memory with a size limit per [`ObjectKind`].
//! [`Fetcher::download_package`] makes a package available as a local file
//! whose size and BLAKE3 hash match its index entry, resuming interrupted
//! downloads and failing over between mirrors.
//!
//! Every error names the URL and the reason. Nothing is printed. Progress
//! goes to a callback.
//!
//! ```no_run
//! use laft_fetch::{Config, Fetcher, Mirrors};
//! use laft_index::IndexEntry;
//!
//! # fn entries() -> Vec<IndexEntry> { vec![] }
//! let fetcher = Fetcher::new(Config::new("/laft/cache"));
//! let mirrors = Mirrors::new("https://pkgs.example.org/laft", &[] as &[&str], false)?;
//! for entry in &entries() {
//!     let path = fetcher.download_package(&mirrors, entry, &mut |p| {
//!         let _ = (p.done, p.total);
//!     })?;
//!     println!("{}", path.display());
//! }
//! # Ok::<(), laft_fetch::Error>(())
//! ```

#![forbid(unsafe_code)]

mod error;
mod fetch;
mod fs;
mod url;

#[cfg(test)]
mod test_support;

pub use error::{Error, ErrorKind};
pub use fetch::{Config, Fetcher, ObjectKind, Progress};
pub use url::{BaseUrl, Mirrors, Scheme};
