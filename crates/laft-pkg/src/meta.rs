use std::collections::BTreeMap;

use laft_core::{
    Blake3Hash, Dep, Name, Provide, Soname, StoreId, Variant, Version, system_gid, system_uid,
    to_canonical_json,
};
use serde::{Deserialize, Serialize};

use crate::manifest::{FORMAT, MAX_SAFE_INTEGER, account_id};
use crate::{Arch, Error, Identity, path};

/// The longest summary, in characters.
const MAX_SUMMARY_CHARS: usize = 120;

/// Package metadata: identity, dependencies and everything else a client
/// needs to know about a package without looking at its payload.
///
/// Metadata is a deterministic function of the recipe and the payload. It
/// holds no timestamps, host names or other details of the build
/// environment, so building the same recipe twice with the same output gives
/// identical bytes and therefore the same store id. It is stored as canonical
/// JSON (RFC 8785).
///
/// Fields are public so that the builder can fill them in. [`Self::validate`]
/// checks every rule, and [`Self::to_json`] and [`Self::from_json`] always
/// validate. The derived `Serialize` and `Deserialize` implementations do
/// not, so a document that embeds metadata through them must call
/// [`Self::validate`] itself. Every list must be sorted without duplicates,
/// as described on each field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    /// The metadata format version, always [`FORMAT`].
    pub format: u32,
    /// The package name.
    pub name: Name,
    /// The upstream version.
    pub version: Version,
    /// The Laft revision, starting at 1.
    pub rev: u32,
    /// The build variant.
    pub variant: Variant,
    /// The architecture.
    pub arch: Arch,
    /// The recipe that produced the package.
    pub origin: Name,
    /// One line of at most 120 characters, without control characters.
    pub summary: String,
    /// The upstream home page, an `http` or `https` URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// License identifiers, sorted. Each is written like an SPDX license
    /// identifier or a `LicenseRef-` reference: letters, digits, `.` and `-`,
    /// optionally followed by `+`. Only that syntax is checked, not
    /// membership in the SPDX license list, so identifiers added to the list
    /// later stay valid.
    pub license: Vec<String>,
    /// Runtime dependencies, sorted by their text form.
    pub depends: Vec<Dep>,
    /// Capabilities offered to other packages, sorted by their text form.
    pub provides: Vec<Provide>,
    /// Packages and virtual capabilities that cannot share a view with this
    /// package, sorted by their text form.
    pub conflicts: Vec<Dep>,
    /// Packages this package replaces, sorted by their text form.
    pub replaces: Vec<Dep>,
    /// Alternative groups this package is a member of, sorted by group.
    pub alternatives: Vec<Alternative>,
    /// Names of the triggers the payload activates, sorted.
    pub triggers: Vec<Name>,
    /// Static libraries linked into the payload, sorted by name.
    pub static_deps: Vec<StaticDep>,
    /// The ABI of each shared library, keyed by its payload path.
    pub abi: BTreeMap<String, Abi>,
    /// Incremented by the recipe when the ABI changes in a way the ABI digest
    /// cannot see.
    pub abi_epoch: u32,
    /// Privileges the recipe asked for, which the store never grants on its
    /// own files, sorted by path.
    pub requested_privileges: Vec<PrivRequest>,
    /// The sum of all file sizes in the payload, in bytes.
    pub installed_size: u64,
}

/// Membership of a package in an alternative group: a set of packages that
/// ship the same paths, of which a view contains one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alternative {
    /// The group name.
    pub group: Name,
    /// The payload paths this package ships for the group, sorted.
    pub paths: Vec<String>,
}

/// A static library package whose code is linked into this package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticDep {
    /// The library package name.
    pub name: Name,
    /// Its upstream version.
    pub version: Version,
    /// Its revision, starting at 1.
    pub rev: u32,
    /// Its store id.
    pub id: StoreId,
}

/// The ABI of a shared library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Abi {
    /// The library's `DT_SONAME`.
    pub soname: Soname,
    /// The digest of its exported dynamic symbols.
    pub digest: Blake3Hash,
}

/// A privilege the recipe asked for on one file.
///
/// Store files never carry setuid or setgid bits or capabilities. The system
/// configuration decides whether a generation gets a privileged copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivRequest {
    /// The payload path of the file.
    pub path: String,
    /// The mode the install step produced, including setuid and setgid bits.
    pub mode: u32,
    /// The owning user, `None` for root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The owning group, `None` for root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// File capabilities in the text syntax of `setcap`, for example
    /// `cap_net_raw=ep`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caps: Option<String>,
}

impl Meta {
    /// The identity fields.
    pub fn identity(&self) -> Identity {
        Identity {
            name: self.name.clone(),
            version: self.version.clone(),
            rev: self.rev,
            variant: self.variant,
            arch: self.arch,
        }
    }

    /// Checks every field rule.
    ///
    /// # Errors
    ///
    /// Fails, naming the field, when a rule is broken.
    pub fn validate(&self) -> Result<(), Error> {
        let identity = self.identity();
        identity.validate()?;
        self.check().map_err(|e| e.in_package(&identity))
    }

    /// The stored form: canonical JSON (RFC 8785).
    ///
    /// # Errors
    ///
    /// Fails when the metadata is not valid.
    ///
    /// # Panics
    ///
    /// Panics if valid metadata held an integer above 2^53, which
    /// [`Self::validate`] rules out.
    pub fn to_json(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        Ok(to_canonical_json(self)
            .expect("valid metadata has no integer above 2^53, so it is canonical JSON"))
    }

    /// Parses metadata from its stored form.
    ///
    /// # Errors
    ///
    /// Fails when `bytes` are not the canonical JSON of valid metadata of the
    /// supported format.
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        let meta: Self = serde_json::from_slice(bytes)
            .map_err(|e| Error::invalid(format!("malformed metadata: {e}")))?;
        if meta.to_json()? != bytes {
            return Err(
                Error::invalid("the metadata is not canonical JSON").in_package(meta.identity())
            );
        }
        Ok(meta)
    }

    fn check(&self) -> Result<(), Error> {
        if self.format != FORMAT {
            return Err(Error::invalid(format!(
                "metadata format {} is not supported, only {FORMAT} is",
                self.format
            )));
        }
        if self.summary.chars().count() > MAX_SUMMARY_CHARS
            || self.summary.chars().any(char::is_control)
        {
            return Err(Error::invalid(
                "`summary` is one line of at most 120 characters",
            ));
        }
        if let Some(homepage) = &self.homepage {
            check_url(homepage)?;
        }
        for license in &self.license {
            let id = license.strip_suffix('+').unwrap_or(license);
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
            {
                return Err(Error::invalid(format!(
                    "`license` entry {license:?} is not one license identifier made of letters, digits, `.` and `-`, optionally followed by `+`"
                )));
            }
        }
        check_sorted("license", &self.license, Clone::clone)?;
        check_sorted("depends", &self.depends, ToString::to_string)?;
        check_sorted("provides", &self.provides, ToString::to_string)?;
        check_sorted("conflicts", &self.conflicts, ToString::to_string)?;
        check_sorted("replaces", &self.replaces, ToString::to_string)?;
        for dep in &self.conflicts {
            if !matches!(dep, Dep::Pkg(..) | Dep::Virtual(_)) {
                return Err(Error::invalid(format!(
                    "`conflicts` entry `{dep}` names neither a package nor a virtual capability"
                )));
            }
        }
        for dep in &self.replaces {
            if !matches!(dep, Dep::Pkg(..)) {
                return Err(Error::invalid(format!(
                    "`replaces` entry `{dep}` does not name a package"
                )));
            }
        }
        check_sorted("alternatives", &self.alternatives, |a| a.group.clone())?;
        for alternative in &self.alternatives {
            if alternative.paths.is_empty() {
                return Err(Error::invalid(format!(
                    "alternative group `{}` lists no paths",
                    alternative.group
                )));
            }
            check_sorted("alternatives.paths", &alternative.paths, Clone::clone)?;
            for path in &alternative.paths {
                check_path(path)?;
            }
        }
        check_sorted("triggers", &self.triggers, Clone::clone)?;
        check_sorted("static_deps", &self.static_deps, |d| d.name.clone())?;
        if let Some(dep) = self.static_deps.iter().find(|d| d.rev == 0) {
            return Err(Error::invalid(format!(
                "static dependency `{}` has revision 0, but revisions start at 1",
                dep.name
            )));
        }
        for path in self.abi.keys() {
            check_path(path)?;
        }
        check_sorted("requested_privileges", &self.requested_privileges, |p| {
            p.path.clone()
        })?;
        for request in &self.requested_privileges {
            check_path(&request.path)?;
            let at = |e: Error| e.at(&request.path);
            if request.mode > 0o7777 {
                return Err(at(Error::invalid(format!(
                    "requested mode {:#o} has bits other than permission bits",
                    request.mode
                ))));
            }
            account_id("owner", request.owner.as_deref(), system_uid).map_err(at)?;
            account_id("group", request.group.as_deref(), system_gid).map_err(at)?;
            if request.caps.as_ref().is_some_and(|caps| {
                caps.is_empty() || !caps.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
            }) {
                return Err(at(Error::invalid(
                    "requested capabilities are printable ASCII text",
                )));
            }
        }
        if self.installed_size > MAX_SAFE_INTEGER {
            return Err(Error::invalid("`installed_size` is at most 2^53"));
        }
        Ok(())
    }
}

/// Fails unless every key is strictly greater than the one before.
fn check_sorted<T, K: Ord>(field: &str, items: &[T], key: impl Fn(&T) -> K) -> Result<(), Error> {
    let keys: Vec<K> = items.iter().map(key).collect();
    if keys.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(Error::invalid(format!(
            "`{field}` must be sorted without duplicates"
        )));
    }
    Ok(())
}

fn check_path(p: &str) -> Result<(), Error> {
    path::check(p).map_err(|reason| Error::invalid(reason).at(p))
}

fn check_url(url: &str) -> Result<(), Error> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    if !rest.is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_graphic())) {
        return Err(Error::invalid(format!(
            "`homepage` {url:?} is not an http or https URL"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "meta_tests.rs"]
mod tests;
