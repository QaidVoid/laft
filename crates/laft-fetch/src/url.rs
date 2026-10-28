use std::fmt;
use std::path::PathBuf;

use crate::{Error, ErrorKind};

/// How a repository is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scheme {
    /// `https://`, the normal transport.
    Https,
    /// `http://`, accepted only for repositories that allow it. Signatures
    /// still protect integrity, but every observer learns what a machine
    /// downloads.
    Http,
    /// `file://`, a repository in a local directory, read in place.
    File,
}

/// The base URL of a repository, below which its files live.
///
/// It is parsed from `https://host[/path]`, `http://host[/path]` or
/// `file:///absolute/path`, and trailing slashes are dropped. A query or
/// fragment is refused, because repository paths are appended to the base.
/// A `file://` URL names a directory by its path as written, without
/// percent decoding.
///
/// ```
/// use laft_fetch::{BaseUrl, Scheme};
///
/// let base = BaseUrl::parse("https://pkgs.example.org/laft/", false)?;
/// assert_eq!(base.scheme(), Scheme::Https);
/// assert_eq!(base.as_str(), "https://pkgs.example.org/laft");
/// assert!(BaseUrl::parse("http://10.0.2.2:8000", false).is_err());
/// assert!(BaseUrl::parse("http://10.0.2.2:8000", true).is_ok());
/// # Ok::<(), laft_fetch::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BaseUrl {
    scheme: Scheme,
    url: String,
}

impl BaseUrl {
    /// Parses a base URL. `http://` is accepted only when `allow_http` is
    /// set.
    ///
    /// # Errors
    ///
    /// Fails with [`ErrorKind::UnsupportedScheme`] for any scheme other than
    /// `https`, `http` and `file`, with [`ErrorKind::HttpNotAllowed`] for
    /// `http` without `allow_http`, and with [`ErrorKind::InvalidUrl`] for a
    /// malformed URL.
    pub fn parse(url: &str, allow_http: bool) -> Result<Self, Error> {
        let invalid = |reason: &str| {
            Error::new(
                url,
                ErrorKind::InvalidUrl {
                    reason: reason.into(),
                },
            )
        };
        let (scheme, rest) = if let Some(rest) = url.strip_prefix("https://") {
            (Scheme::Https, rest)
        } else if let Some(rest) = url.strip_prefix("http://") {
            if !allow_http {
                return Err(Error::new(url, ErrorKind::HttpNotAllowed));
            }
            (Scheme::Http, rest)
        } else if let Some(rest) = url.strip_prefix("file://") {
            (Scheme::File, rest)
        } else {
            return Err(Error::new(url, ErrorKind::UnsupportedScheme));
        };
        if url.contains(['?', '#']) {
            return Err(invalid("a base URL has no query or fragment"));
        }
        let trimmed = url.trim_end_matches('/');
        match scheme {
            Scheme::File if !rest.starts_with('/') => {
                return Err(invalid("a file:// URL names an absolute path"));
            }
            Scheme::File => {}
            Scheme::Https | Scheme::Http => {
                if rest.starts_with('/') || rest.is_empty() {
                    return Err(invalid("the host is missing"));
                }
                ureq::http::Uri::try_from(trimmed).map_err(|e| invalid(&e.to_string()))?;
            }
        }
        // `file:///` names the root directory, which keeps its slash.
        let url = if scheme == Scheme::File && trimmed.len() < "file:///".len() {
            "file:///".to_owned()
        } else {
            trimmed.to_owned()
        };
        Ok(Self { scheme, url })
    }

    /// The scheme.
    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// The URL, without a trailing slash.
    pub fn as_str(&self) -> &str {
        &self.url
    }

    /// The URL of `path` below this base.
    pub(crate) fn join(&self, path: &str) -> String {
        format!("{}/{path}", self.url.trim_end_matches('/'))
    }

    /// The local directory of a `file://` base.
    pub(crate) fn dir(&self) -> Option<PathBuf> {
        let path = self.url.strip_prefix("file://")?;
        (self.scheme == Scheme::File).then(|| PathBuf::from(path))
    }
}

impl fmt::Display for BaseUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.url)
    }
}

/// The base URLs of one repository in the order they are tried: the
/// configured URL, then the configured mirrors. A URL that appears twice is
/// tried once, at its first place.
///
/// ```
/// use laft_fetch::Mirrors;
///
/// let mirrors = Mirrors::new(
///     "https://a.example.org",
///     &["https://b.example.org", "https://a.example.org/"],
///     false,
/// )?;
/// let urls: Vec<&str> = mirrors.bases().iter().map(|b| b.as_str()).collect();
/// assert_eq!(urls, ["https://a.example.org", "https://b.example.org"]);
/// # Ok::<(), laft_fetch::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mirrors {
    bases: Vec<BaseUrl>,
}

impl Mirrors {
    /// The configured URL and mirrors of a repository. `allow_http` is the
    /// repository's permission to use plain `http://`.
    ///
    /// # Errors
    ///
    /// Fails when any of the URLs is refused by [`BaseUrl::parse`].
    pub fn new(url: &str, mirrors: &[impl AsRef<str>], allow_http: bool) -> Result<Self, Error> {
        let mut this = Self { bases: Vec::new() };
        for url in std::iter::once(url).chain(mirrors.iter().map(AsRef::as_ref)) {
            this.push(BaseUrl::parse(url, allow_http)?);
        }
        Ok(this)
    }

    /// The same mirrors with `first` moved to the front, for example the
    /// URL a repository's index was accepted from, which package downloads
    /// try first. Nothing changes when `first` is not one of the mirrors.
    #[must_use]
    pub fn preferring(&self, first: &BaseUrl) -> Self {
        let mut bases = self.bases.clone();
        if let Some(i) = bases.iter().position(|b| b == first) {
            let base = bases.remove(i);
            bases.insert(0, base);
        }
        Self { bases }
    }

    /// The base URLs in the order they are tried.
    pub fn bases(&self) -> &[BaseUrl] {
        &self.bases
    }

    fn push(&mut self, base: BaseUrl) {
        if !self.bases.contains(&base) {
            self.bases.push(base);
        }
    }
}

/// Checks a path relative to a repository base. Paths come from signed
/// documents and from the layout itself, and they never need more than
/// these characters, so anything else is refused rather than escaped. This
/// also keeps a `file://` repository from reaching outside its directory.
pub(crate) fn check_path(path: &str) -> Result<(), ErrorKind> {
    let valid = !path.is_empty()
        && path.split('/').all(|c| {
            !c.is_empty()
                && c != "."
                && c != ".."
                && c.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        });
    if valid {
        Ok(())
    } else {
        Err(ErrorKind::InvalidPath { path: path.into() })
    }
}

#[cfg(test)]
#[path = "url_tests.rs"]
mod tests;
