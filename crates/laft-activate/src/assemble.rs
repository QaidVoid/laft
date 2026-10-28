//! Assembly of a complete generation.

use std::collections::BTreeMap;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use laft_store::{
    CarryOver, GenerationInput, Lock, PackageInput, Recovery, Root, Store, Transaction,
    TriggerRecord,
};
use laft_svc::ServiceSet;
use laft_svc::runit::{self, Entry};

use crate::config::{BootConfig, EtcConfig, Security};
use crate::error::{Error, Result, Warning, io};
use crate::initramfs::{self, BootRecord, Host};
use crate::privileged::{self, PrivilegedRecord};
use crate::render::{Node, render};
use crate::trigger::{self, Contributor};
use crate::{declare, fault, install_boot, state};

/// Everything a generation is assembled from, as plain values.
#[derive(Debug, Clone, Copy)]
pub struct Assembly<'a> {
    /// The store's input: the resolved packages, the initrd packages, the
    /// alternatives, the views and the records of `gen.json`. Its `etc` and
    /// `carry_over` fields are ignored, because assembly computes them.
    pub generation: &'a GenerationInput,
    /// The configuration of `/etc` and the accounts.
    pub etc: &'a EtcConfig,
    /// The validated services.
    pub services: &'a ServiceSet,
    /// The boot configuration.
    pub boot: &'a BootConfig,
    /// The security settings, with the privileged files.
    pub security: &'a Security,
    /// The machine running the assembly.
    pub host: &'a Host,
}

/// What assembling a generation produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembled {
    /// The generation number.
    pub generation: u64,
    /// The record of every trigger instance, as in `triggers.json`.
    pub triggers: BTreeMap<String, TriggerRecord>,
    /// The privileged copies, as in `privileged.json`.
    pub privileged: PrivilegedRecord,
    /// The kernel and initramfs, as in `boot/boot.json`.
    pub boot: BootRecord,
    /// What the user should know: failed optional triggers, inactive
    /// privileged entries, privileges that could not be applied without
    /// root, and warnings about declared files.
    pub warnings: Vec<Warning>,
}

/// Assembles the generation of the transaction `tx` and publishes it.
///
/// The generation directory gets, besides `gen.json` and `config.json`:
///
/// | Path | Contents |
/// |---|---|
/// | `usr/` | the system view, with the modules of the running kernel when no package ships them, the outputs of the triggers and the privileged copies |
/// | `etc/` | the static `/etc` tree, including the init system's entry points |
/// | `services/` | the rendered services |
/// | `boot/` | `boot.json` and the initramfs |
/// | `accounts.json` | the declared accounts, mode `0o600` |
/// | `triggers.json` | a record per trigger instance |
/// | `privileged.json` | the privileged copies and which views may receive them |
///
/// The accounts are declared first, which records their ids. Everything
/// else is built in the transaction's staging directory, so an
/// interruption at any point leaves nothing behind once the transaction is
/// recovered. The declared views must be assembled before, because the
/// privileged copies depend on them. A required trigger that fails fails
/// the assembly, and an optional one only adds a warning.
///
/// ```no_run
/// use std::collections::BTreeSet;
///
/// use laft_activate::{Assembly, BootConfig, EtcConfig, Host, Security, assemble};
/// use laft_store::{GenerationInput, Plan, Root, Transaction};
/// use laft_svc::{ServiceConfig, ServiceSet, standard_services};
///
/// # fn apply(input: &GenerationInput) -> Result<(), Box<dyn std::error::Error>> {
/// let root = Root::default();
/// let lock = root.lock("apply")?;
/// lock.recover()?;
/// let plan = Plan {
///     command: "apply".into(),
///     previous: root.default_generation()?,
///     views: BTreeSet::new(),
/// };
/// let mut tx = Transaction::begin(&lock, &plan)?;
/// let services = ServiceConfig { services: standard_services(), ..ServiceConfig::default() };
/// let boot = BootConfig::default();
/// let assembled = assemble(&lock, &mut tx, &Assembly {
///     generation: input,
///     etc: &EtcConfig::default(),
///     services: &ServiceSet::new(&services).expect("valid services"),
///     boot: &boot,
///     security: &Security::default(),
///     host: &Host::detect(&boot),
/// })?;
/// for warning in &assembled.warnings {
///     eprintln!("warning: {warning}");
/// }
/// tx.make_default()?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Fails as [`declare`], [`render`] and
/// [`Transaction::stage_generation`] do, when the services cannot be
/// rendered, when a required trigger fails, when a privileged entry is
/// invalid, when the kernel, a module index or a program of the initramfs
/// is missing, and when writing fails.
pub fn assemble(
    lock: &Lock,
    tx: &mut Transaction<'_>,
    assembly: &Assembly<'_>,
) -> Result<Assembled> {
    let root = lock.root();
    let store = root.store();
    let number = tx.generation();
    let declared = declare(lock, &assembly.etc.users)?;
    let rendered = runit::render(assembly.services, number).map_err(Error::Services)?;
    let mut generated = BTreeMap::new();
    let mut services = BTreeMap::new();
    for (path, entry) in rendered.entries() {
        match path.strip_prefix("etc/") {
            Some(rel) => {
                let node = match entry {
                    Entry::File { mode, bytes } => Node::File {
                        bytes: bytes.clone(),
                        mode: *mode,
                        uid: 0,
                        gid: 0,
                    },
                    Entry::Symlink(target) => Node::Symlink(target.clone()),
                };
                generated.insert(rel.to_owned(), node);
            }
            None => {
                services.insert(path.as_str(), entry);
            }
        }
    }
    let dirs: Vec<String> = assembly
        .generation
        .packages
        .iter()
        .map(|p| p.dir.clone())
        .collect();
    let etc = render(assembly.etc, &declared, &store, &dirs, &generated)?;
    let mut input = assembly.generation.clone();
    input.etc = etc.record();
    input.carry_over = carry_over(root, &store, assembly.host, &input.packages)?;

    let staged = tx.stage_generation(&input)?;
    fault::point("assemble.staged")?;
    let dir = staged.path().to_path_buf();
    etc.write(&dir)?;
    for (path, entry) in services {
        entry.write(&dir.join(path)).map_err(Error::Services)?;
    }
    declared.write(&dir)?;
    DirBuilder::new()
        .mode(0o755)
        .create(dir.join("boot"))
        .map_err(io(&dir.join("boot")))?;
    fault::point("assemble.etc")?;

    let previous = tx
        .previous()
        .map(|n| state::gen_dir(root, n))
        .filter(|d| d.join("gen.json").exists());
    let mut contributors = Vec::new();
    for dir in input
        .packages
        .iter()
        .map(|p| &p.dir)
        .chain(input.carry_over.iter().map(|c| &c.dir))
    {
        let meta = store.meta(dir)?;
        contributors.push(Contributor {
            id: store.manifest(dir)?.store_id(),
            triggers: meta.triggers.iter().map(ToString::to_string).collect(),
        });
    }
    let mut warnings = etc.warnings().to_vec();
    let triggers = trigger::run(
        &dir,
        &contributors,
        |path| staged.owner(path),
        previous.as_deref(),
        &mut warnings,
    )?;
    trigger::write_records(&dir, &triggers)?;
    fault::point("assemble.triggers")?;

    let privileged = privileged::apply(
        root,
        &dir,
        &assembly.security.privileged,
        &declared,
        staged.record(),
        |path| staged.owner(path),
        &mut warnings,
    )?;
    fault::point("assemble.privileged")?;

    let boot = initramfs::build(
        &initramfs::Inputs {
            tree: &dir,
            store: &store,
            generation: staged.record(),
            config: assembly.boot,
            host: assembly.host,
            previous: previous.as_deref(),
        },
        &mut warnings,
    )?;
    fault::point("assemble.boot")?;

    let generation = tx.publish_generation(staged)?;
    Ok(Assembled {
        generation,
        triggers,
        privileged,
        boot,
        warnings,
    })
}

/// The kernel modules of the running kernel to carry into a generation of
/// `packages`: none when the kernel is unknown or a package ships its
/// modules, otherwise the module trees of every package of the booted
/// generation that ships some.
fn carry_over(
    root: &Root,
    store: &Store,
    host: &Host,
    packages: &[PackageInput],
) -> Result<Vec<CarryOver>> {
    let Some(kernel) = &host.kernel else {
        return Ok(Vec::new());
    };
    let modules = format!("usr/lib/modules/{kernel}");
    let ships = |dir: &str| -> Result<bool> {
        let manifest = store.manifest(dir)?;
        Ok(manifest
            .entries()
            .iter()
            .any(|e| e.path() == modules || e.path().starts_with(&format!("{modules}/"))))
    };
    for package in packages {
        if ships(&package.dir)? {
            return Ok(Vec::new());
        }
    }
    let Some(booted) = booted(root)? else {
        return Ok(Vec::new());
    };
    let generation = root.generation(booted)?;
    let mut dirs: Vec<String> = generation.packages.iter().map(|p| p.dir.clone()).collect();
    let carried: Vec<String> = generation
        .carried_modules
        .iter()
        .filter(|c| &c.kernel == kernel)
        .map(|c| format!("{}-", c.id))
        .collect();
    if !carried.is_empty() {
        let entries = store.list()?;
        for prefix in &carried {
            dirs.extend(entries.iter().filter(|d| d.starts_with(prefix)).cloned());
        }
    }
    let mut carry = Vec::new();
    for dir in dirs {
        if ships(&dir)? && !carry.iter().any(|c: &CarryOver| c.dir == dir) {
            carry.push(CarryOver {
                kernel: kernel.clone(),
                dir,
            });
        }
    }
    Ok(carry)
}

/// The generation that `run/laft/booted` points at, or `None` when the
/// link does not exist, as on a system being installed.
fn booted(root: &Root) -> Result<Option<u64>> {
    let link = root.path().join("run/laft/booted");
    let target = match fs::read_link(&link) {
        Ok(target) => target,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(&link)(e)),
    };
    target
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.parse().ok())
        .map(Some)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "{} does not name a generation: {}",
                link.display(),
                target.display()
            ))
        })
}

/// Finishes or rolls back every interrupted transaction, as
/// [`Lock::recover`] does, and then installs the boot files again when it
/// recovered anything, so that the boot menu on the EFI system partition
/// at `esp` lists exactly the generations that exist.
///
/// # Errors
///
/// Fails as [`Lock::recover`] and [`install_boot`] do.
pub fn recover(lock: &Lock, esp: &Path) -> Result<Vec<Recovery>> {
    let recovered = lock.recover()?;
    if !recovered.is_empty() {
        install_boot(lock.root(), esp)?;
    }
    Ok(recovered)
}

#[cfg(test)]
#[path = "assemble_tests.rs"]
mod tests;
