//! The Laft system configuration.
//!
//! A machine is described by `/etc/laft/system.stav` and the files it
//! imports or includes, written in Stav. This crate supplies everything the
//! language needs to check that description: the prelude with the `System`
//! schema, the base part that every system includes, the std catalog
//! (`std/services` and `std/desktop`), the symbol type `Pkg`, the
//! implicit primary groups and a loader confined to the configuration
//! directory. [`load`] evaluates the configuration, validates it and
//! converts it into the plain values the rest of Laft works with.
//! [`evaluate`] only type checks and evaluates a file with the same prelude,
//! base part and catalog, for tools that print or inspect the value.
//!
//! # Validation
//!
//! Stav checks types, refinements, required fields and references. After
//! evaluation, [`load`] resolves every package reference with the caller's
//! resolver and applies the rules below. Every problem is reported as a
//! diagnostic with a code and the source it is about, in a [`Report`].
//!
//! | Code | Problem |
//! |---|---|
//! | `C0405` | invalid user or group name |
//! | `C0406` | both `password_hash` and `password_file` set |
//! | `C0407` | explicit id in a range reserved for the static accounts |
//! | `C0408` | changed id of `root` or a static account |
//! | `C0414` | two declared accounts with the same id |
//! | `C0415` | a regular user would take a system group as its primary group |
//! | `C0420` | `file` path not an absolute, normalized path below `/etc/` |
//! | `C0421` | `file` path reserved for Laft |
//! | `C0422` | enabled `file` without exactly one content field |
//! | `C0425` | a path both a file and the directory of another entry |
//! | `C0426` | structured content that its format cannot express |
//! | `C0427` | `env` key that is not a variable name |
//! | `C0431` | malformed sysctl key |
//! | `C0433` | kernel module both loaded and blacklisted |
//! | `C0434` | sysctl value not an integer, string or boolean |
//! | `C0450` | init backend not available in this version |
//! | `C0490` | a value that cannot be written into the file it belongs to |
//! | `C0491` | no free id left for an account |
//! | `C0492` | a reference to an account that is not declared |
//! | `C0801` | repository URL with an unsupported scheme, or `http://` without `allow_http` |
//! | `C0802` | repository key that is not a minisign public key |
//! | `C0803` | repository name that is not a package name |
//! | `C0901` | package reference that does not resolve |
//! | `C0902` | alternatives group that is not a name, or a member that is not a bare package name |
//! | `C0903` | view name that is not a package name, or `system` or `initrd` |
//! | `C0904` | view user or group that is not declared |
//! | `C1101` | no root filesystem |
//! | `C1102` | mountpoint not an absolute, normalized path |
//! | `C1103` | mountpoint managed by Laft |
//! | `C1104` | root filesystem type that the initramfs cannot mount |
//! | `C1105` | device of the root, `/laft`, the ESP or a LUKS block that the initramfs cannot resolve, or an invalid LUKS name |
//! | `C1106` | root filesystem mounted read-only |
//! | `C1107` | `boot.esp` without a `vfat` filesystem |
//! | `C1108` | `/dev/mapper/<name>` device without `luks <name>` |
//! | `C1109` | filesystem without a device |
//! | `C1110` | swap device that is not a path or a `UUID=`, `LABEL=`, `PARTUUID=` or `PARTLABEL=` tag |
//! | `C1201` | invalid or reserved service name |
//! | `C1202` | invalid or repeated getty terminal |
//! | `C1203` | service setting that does not fit its kind |
//! | `C1204` | command or path that is not absolute |
//! | `C1205` | dependency on a disabled service, or of an early service on a later one |
//! | `C1206` | dependency cycle between services |
//! | `C1207` | environment variable that cannot be passed to a program |
//! | `C1208` | directory shared by services with different owners or modes |
//! | `C1209` | service package that is not a bare package name |
//! | `C1210` | (warning) `early` set on a service outside the base part |
//! | `C1301` | privileged file outside `/usr/bin` and `/usr/lib` |
//! | `C1302` | privileged file that grants nothing |
//! | `C1303` | invalid capability text |

#![forbid(unsafe_code)]

mod convert;
mod host;
mod loader;
mod validate;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use laft_activate::{BootConfig, EtcConfig, Security};
use laft_core::{Blake3Hash, Name, PkgRef};
use laft_index::PublicKey;
use laft_resolve::Request;
use laft_store::{ConfigFile, GcPolicy};
use laft_svc::ServiceConfig;
use serde::Deserialize;

/// Diagnostics about a configuration, with the names and texts of the
/// files they point at, ready for a front end to render.
pub type Report = stav::Diagnostics;

/// The evaluated, validated system configuration as plain values.
#[derive(Debug, Clone)]
pub struct SystemConfig {
    /// Identity, locale, accounts, files in `/etc`, kernel modules and
    /// sysctls. Every implicit primary group is an entry of
    /// `etc.users.groups`.
    pub etc: EtcConfig,
    /// Locales that must be available in addition to `etc.locale`.
    pub extra_locales: BTreeSet<String>,
    /// Login session tracking.
    pub session: Session,
    /// The boot loader, kernel, initramfs, filesystems and LUKS devices.
    pub boot: BootConfig,
    /// Swap devices, by device.
    pub swap: BTreeMap<String, Swap>,
    /// The init system, the services and the gettys.
    pub services: ServiceConfig,
    /// The privileged files.
    pub security: Security,
    /// The package requests of the system view.
    pub packages: Vec<Request>,
    /// The chosen member of each alternatives group of the system view.
    pub alternatives: BTreeMap<Name, Name>,
    /// The views besides the system view, by name.
    pub views: BTreeMap<Name, View>,
    /// Retention of generations and automatic collection.
    pub gc: Gc,
    /// The package repositories, by name.
    pub repos: BTreeMap<Name, Repo>,
}

/// A configuration that passed every check, and what was suspicious but
/// allowed.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The configuration.
    pub config: SystemConfig,
    /// The warnings of the language and of validation. It holds no errors.
    pub warnings: Report,
    /// The evaluated value of `system` as canonical JSON, which a
    /// generation keeps as `config.json`.
    pub value: Vec<u8>,
    /// Every file of the configuration directory that was read, with the
    /// hash of its content, sorted by path.
    pub files: Vec<ConfigFile>,
}

/// A configuration file that was type checked and evaluated, but neither
/// validated nor converted.
#[derive(Debug)]
pub struct Evaluation {
    /// The checked program, which also describes its schema.
    pub checked: stav::Checked,
    /// The evaluated value.
    pub evaluated: stav::Evaluated,
    /// The warnings of the language. It holds no errors.
    pub warnings: Report,
}

/// Why a package reference does not resolve, as the caller's resolver
/// reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkgError {
    /// What is wrong, starting lowercase and without a final period, for
    /// example ``no package `htpo` in the index``.
    pub message: String,
    /// A suggested fix, for example ``did you mean `htop`?``.
    pub help: Option<String>,
}

/// Login session tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Session {
    /// Who creates and removes `/run/user/<uid>`.
    pub rundir: RundirOwner,
}

/// Who creates and removes `/run/user/<uid>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RundirOwner {
    /// The session tracker turnstile.
    Turnstile,
    /// The login manager elogind.
    Elogind,
}

/// A swap device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Swap {
    /// The swap priority, from -1 to 32767, or `None` for the kernel's
    /// choice.
    pub priority: Option<i32>,
}

/// Retention of generations and automatic collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gc {
    /// Which generations collection keeps.
    pub policy: GcPolicy,
    /// Whether collection runs after every successful apply.
    pub auto: bool,
    /// Whether downloaded package files are kept after unpacking.
    pub keep_downloads: bool,
}

/// A view besides the system view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// Whether the view starts from the system's package requests and
    /// alternatives. Its own requests replace system requests of the same
    /// package name, and its alternatives override the system's per group.
    pub inherit: bool,
    /// The view's own package requests.
    pub packages: Vec<Request>,
    /// The view's own choices of alternatives, by group.
    pub alternatives: BTreeMap<Name, Name>,
    /// Users allowed to enter the view, in addition to root.
    pub users: BTreeSet<String>,
    /// Groups whose members may enter the view.
    pub groups: BTreeSet<String>,
}

/// A package repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The primary base URL.
    pub url: String,
    /// More base URLs serving the same content, tried in order after `url`.
    pub mirrors: Vec<String>,
    /// The root key.
    pub key: PublicKey,
    /// More accepted root keys, used while a root key is replaced.
    pub keys: Vec<PublicKey>,
    /// The channel the client follows.
    pub channel: Name,
    /// Higher wins when several repositories offer a package.
    pub priority: i64,
    /// Whether the repository is used.
    pub enable: bool,
    /// Whether `http://` URLs are permitted.
    pub allow_http: bool,
}

/// Loads the system configuration whose entry file is
/// `<root_dir>/system.stav`.
///
/// The configuration is type checked and evaluated together with the
/// prelude, the base part and the std catalog. Files can be imported or
/// included only from inside `root_dir`, and never from its `secrets/`
/// directory. Every package reference is passed to `packages`, which
/// resolves it against an index snapshot. Then the configuration is
/// validated and converted. Every reported problem points at the source
/// it is about.
///
/// # Errors
///
/// Returns a [`Report`] with every error found, together with the
/// warnings, when the configuration does not load, type check or evaluate,
/// when a package reference does not resolve, or when it breaks a rule of
/// the crate documentation.
///
/// # Examples
///
/// ```
/// let dir = laft_test::tempdir();
/// std::fs::write(
///     dir.path().join("system.stav"),
///     "system {\n  hostname = \"forge\"\n  fs \"/\" { device = \"UUID=3f1c\", fstype = ext4 }\n  \
///      fs \"/boot\" { device = \"UUID=1A2B\", fstype = vfat }\n}\n",
/// )?;
/// let loaded = laft_config::load(dir.path(), &|_| Ok(())).expect("the configuration is valid");
/// assert_eq!(loaded.config.etc.hostname, "forge");
/// assert!(loaded.config.services.services.contains_key("boot-complete"));
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn load(
    root_dir: &Path,
    packages: &dyn Fn(&PkgRef) -> Result<(), PkgError>,
) -> Result<Loaded, Report> {
    let Evaluation {
        checked, evaluated, ..
    } = evaluate(root_dir, &root_dir.join("system.stav"))?;
    let cx = convert::Cx::new(&checked, &evaluated);
    let mut found = checked.warnings().iter().cloned().collect::<Vec<_>>();
    let config = validate::validate(&cx, packages, &mut found);
    let report = evaluated.diagnostics(found);
    let files = checked
        .inputs()
        .filter(|(path, _)| path.starts_with(root_dir))
        .map(|(path, text)| ConfigFile {
            path: path.to_string_lossy().into_owned(),
            hash: Blake3Hash::of(text.as_bytes()),
        })
        .collect();
    match config {
        Some(config) if !report.has_errors() => Ok(Loaded {
            config,
            warnings: report,
            value: system_json(&evaluated),
            files,
        }),
        _ => Err(report),
    }
}

/// The canonical JSON of the evaluated `system` value.
///
/// # Panics
///
/// Panics when the evaluation has no `system` value, which the schema's
/// default for it rules out.
fn system_json(evaluated: &stav::Evaluated) -> Vec<u8> {
    evaluated
        .value_at(&stav::AttrPath::new(["system"]))
        .map(|v| v.to_canonical_json().into_bytes())
        .expect("a checked configuration has a system value, by its default")
}

/// Type checks and evaluates the file `entry` of the configuration
/// directory `root_dir`, with the prelude, the base part and the std
/// catalog, as [`load`] does, but resolves no package and applies none of
/// the rules of the crate documentation. Files can be imported or included
/// only from inside `root_dir`.
///
/// # Errors
///
/// Returns a [`Report`] with every error found, together with the
/// warnings, when the file does not load, type check or evaluate.
///
/// # Examples
///
/// ```
/// let dir = laft_test::tempdir();
/// std::fs::write(dir.path().join("part.stav"), "system { hostname = \"forge\" }\n")?;
/// let evaluation = laft_config::evaluate(dir.path(), &dir.path().join("part.stav"))
///     .expect("the file evaluates");
/// let hostname: String = evaluation
///     .evaluated
///     .get(&stav::AttrPath::new(["system", "hostname"]))
///     .expect("the hostname is a string");
/// assert_eq!(hostname, "forge");
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn evaluate(root_dir: &Path, entry: &Path) -> Result<Evaluation, Report> {
    let host = host::host(root_dir);
    let root = stav::RootSpec {
        root_struct: "Root".to_owned(),
        entries: vec![entry.to_path_buf()],
        import_roots: vec![stav::ImportRoot::new(root_dir)],
    };
    let checked = stav::check(&host, &root)?;
    let evaluated = checked.eval(&stav::Limits::default()).map_err(|errors| {
        checked.diagnostics(errors.iter().chain(checked.warnings().iter()).cloned())
    })?;
    let warnings = checked.warnings().clone();
    Ok(Evaluation {
        checked,
        evaluated,
        warnings,
    })
}
