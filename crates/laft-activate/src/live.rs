//! Activation of a running system, recorded step by step in the store's
//! journal so that an interruption is finished or undone.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use laft_store::{Activation, Lock, Root};
use laft_svc::reconcile::{self, Applied};

use crate::error::{Error, Result, io};
use crate::etc::{OnConflict, reconcile_etc};
use crate::{Warning, apply_accounts, fault};

/// The steps of a live activation, in the order they run: mount the
/// generation below `run/laft/mounts`, point `run/laft/system` at it, point
/// the views at their new versions, reconcile `/etc`, write the accounts,
/// reconcile the services, and detach the mounts nothing uses any more.
pub const LIVE_STEPS: &[&str] = &[
    "mount", "switch", "views", "etc", "accounts", "services", "unmount",
];

/// The position of `switch` in [`LIVE_STEPS`]: the running system uses the
/// generation once that step is done, and only the steps before it can be
/// undone.
const SWITCH: usize = 1;

/// The mounts of a running system, which live activation needs but cannot
/// make itself, because they belong to the initial mount namespace.
pub trait Mounts {
    /// Mounts generation `number` at `run/laft/mounts/<number>`. A
    /// generation that is mounted already stays as it is.
    ///
    /// # Errors
    ///
    /// Returns why the mount failed.
    fn mount(&mut self, number: u64) -> std::result::Result<(), String>;

    /// Detaches every generation mount except those of `keep`.
    ///
    /// # Errors
    ///
    /// Returns why a mount could not be detached.
    fn unmount(&mut self, keep: &[u64]) -> std::result::Result<(), String>;
}

/// What happened to the services during a live activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Services {
    /// The running init is not the generation's, so nothing changed and
    /// the new services take effect at the next boot.
    Skipped {
        /// The init backend the generation uses.
        backend: String,
    },
    /// The running services were brought in line with the generation.
    Reconciled {
        /// Services whose change takes effect only at the next boot.
        reboot: Vec<String>,
        /// What applying the change did.
        applied: Applied,
    },
}

/// What a live activation did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveActivation {
    /// The generation that was activated.
    pub generation: u64,
    /// Whether the running system uses the generation now. When it does
    /// not, the activation was rolled back.
    pub active: bool,
    /// The steps that ran and finished.
    pub done: Vec<&'static str>,
    /// The steps after the switch that failed, with why. The steps after a
    /// failed one still run.
    pub failed: Vec<(&'static str, String)>,
    /// Warnings of reconciling `/etc` and writing the accounts.
    pub warnings: Vec<Warning>,
    /// What happened to the services, when that step ran and succeeded.
    pub services: Option<Services>,
}

impl LiveActivation {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            active: false,
            done: Vec::new(),
            failed: Vec::new(),
            warnings: Vec::new(),
            services: None,
        }
    }
}

/// Runs the live activation `activation`, which must have been started
/// with [`LIVE_STEPS`] under `lock`, and makes its generation the default.
/// Callers check `/etc` with [`crate::check_etc`] first, because a
/// conflict found here no longer stops the switch.
///
/// Every step is recorded in the journal once it finished, and each one
/// can run again, so [`resume_live`] continues an interrupted activation.
/// When mounting the generation or switching to it fails, the activation
/// is rolled back: the mount is detached, the journal ends and the
/// running system stays on its generation. After the switch, every step
/// runs even when an earlier one failed, and the failures are reported.
///
/// ```no_run
/// use std::collections::BTreeSet;
///
/// use laft_activate::{LIVE_STEPS, Mounts, activate_live};
/// use laft_store::Root;
///
/// struct Coordinator;
///
/// impl Mounts for Coordinator {
///     fn mount(&mut self, number: u64) -> Result<(), String> {
///         Ok(())
///     }
///     fn unmount(&mut self, keep: &[u64]) -> Result<(), String> {
///         Ok(())
///     }
/// }
///
/// let lock = Root::default().lock("switch")?;
/// let activation = lock.activate("switch", 42, Some(41), LIVE_STEPS)?;
/// let done = activate_live(&lock, activation, &BTreeSet::new(), &mut Coordinator)?;
/// assert!(done.active);
/// # Ok::<(), laft_activate::Error>(())
/// ```
///
/// # Errors
///
/// Fails with [`Error::Live`] when the generation cannot be mounted or
/// switched to, when the activation does not have [`LIVE_STEPS`], and when
/// the journal cannot be written.
pub fn activate_live(
    lock: &Lock,
    activation: Activation<'_>,
    adopt: &BTreeSet<String>,
    mounts: &mut dyn Mounts,
) -> Result<LiveActivation> {
    run(lock, activation, adopt, OnConflict::Stop, mounts)
}

/// Finishes or undoes the live activation that was interrupted during the
/// current boot, if there is one.
///
/// The decision depends only on how far the activation came. When the
/// running system already uses the new generation, because the switch
/// finished or `run/laft/system` points at it, the remaining steps run
/// and the generation becomes the default. `/etc` conflicts are skipped
/// with a warning then, because nobody chose how to resolve them.
/// Otherwise the activation is rolled back as [`activate_live`] does when
/// the switch fails, and the result says the generation is not active.
///
/// # Errors
///
/// Fails when the journal cannot be read or written, and with
/// [`Error::Live`] when the activation's steps are not [`LIVE_STEPS`].
pub fn resume_live(lock: &Lock, mounts: &mut dyn Mounts) -> Result<Option<LiveActivation>> {
    let Some(mut activation) = lock.interrupted_activation()? else {
        return Ok(None);
    };
    check_steps(&activation)?;
    let number = activation.generation();
    let switched = activation.finished() > SWITCH || active(lock.root()) == Some(number);
    if !switched {
        rollback(lock.root(), activation, mounts)?;
        return Ok(Some(LiveActivation::new(number)));
    }
    while activation.finished() <= SWITCH {
        activation.step_finished()?;
    }
    run(lock, activation, &BTreeSet::new(), OnConflict::Skip, mounts).map(Some)
}

fn check_steps(activation: &Activation<'_>) -> Result<()> {
    if activation.steps() != LIVE_STEPS {
        return Err(Error::Live {
            step: "resume",
            reason: format!(
                "the journal lists the steps {:?}, which are not those of a live activation",
                activation.steps()
            ),
        });
    }
    Ok(())
}

fn run(
    lock: &Lock,
    mut activation: Activation<'_>,
    adopt: &BTreeSet<String>,
    on_conflict: OnConflict,
    mounts: &mut dyn Mounts,
) -> Result<LiveActivation> {
    check_steps(&activation)?;
    let root = lock.root();
    let number = activation.generation();
    let mut report = LiveActivation::new(number);
    report.active = activation.finished() > SWITCH;
    while let Some(&step) = LIVE_STEPS.get(activation.finished()) {
        let result = match step {
            "mount" => mounts.mount(number),
            "switch" => replace_link(
                &root.path().join("run/laft/system"),
                &format!("mounts/{number}"),
            )
            .map_err(|e| e.to_string()),
            "views" => views(root, number),
            "etc" => reconcile_etc(lock, number, adopt, on_conflict)
                .map(|etc| report.warnings.extend(etc.warnings))
                .map_err(|e| e.to_string()),
            "accounts" => apply_accounts(lock, number)
                .map(|accounts| report.warnings.extend(accounts.warnings))
                .map_err(|e| e.to_string()),
            "services" => services(root, activation.previous(), number)
                .map(|services| report.services = Some(services)),
            _ => {
                let keep: Vec<u64> = std::iter::once(number).chain(booted(root)).collect();
                mounts.unmount(&keep)
            }
        };
        match result {
            Ok(()) => report.done.push(step),
            Err(reason) if activation.finished() <= SWITCH => {
                rollback(root, activation, mounts)?;
                return Err(Error::Live { step, reason });
            }
            Err(reason) => report.failed.push((step, reason)),
        }
        fault::point(step_point(step))?;
        activation.step_finished()?;
        report.active |= step == "switch";
    }
    activation.finish()?;
    Ok(report)
}

/// The fault point a test interrupts after `step`.
fn step_point(step: &str) -> &'static str {
    match step {
        "mount" => "live.mount",
        "switch" => "live.switch",
        "views" => "live.views",
        "etc" => "live.etc",
        "accounts" => "live.accounts",
        "services" => "live.services",
        _ => "live.unmount",
    }
}

/// Undoes an activation that has not switched: points `run/laft/system`
/// back at the generation it used when it was moved, detaches every mount
/// the running system does not use, and ends the journal.
fn rollback(root: &Root, activation: Activation<'_>, mounts: &mut dyn Mounts) -> Result<()> {
    let number = activation.generation();
    if active(root) == Some(number)
        && let Some(previous) = activation.previous()
    {
        replace_link(
            &root.path().join("run/laft/system"),
            &format!("mounts/{previous}"),
        )
        .map_err(io(&root.path().join("run/laft/system")))?;
    }
    let keep: Vec<u64> = active(root).into_iter().chain(booted(root)).collect();
    let unmounted = mounts.unmount(&keep);
    activation.abandon()?;
    unmounted.map_err(|reason| Error::Live {
        step: "rollback",
        reason,
    })
}

/// Points every declared view of generation `number` at its version.
fn views(root: &Root, number: u64) -> std::result::Result<(), String> {
    let generation = root.generation(number).map_err(|e| e.to_string())?;
    let mut failed = Vec::new();
    for (name, version) in &generation.views {
        let link = root
            .path()
            .join("laft/views")
            .join(name.as_str())
            .join("current");
        if let Err(e) = replace_link(&link, &version.to_string()) {
            failed.push(format!("{}: {e}", link.display()));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}

/// Brings the running services in line with generation `number`, when the
/// running init is the generation's.
fn services(
    root: &Root,
    previous: Option<u64>,
    number: u64,
) -> std::result::Result<Services, String> {
    let running = fs::read_to_string(root.path().join("run/laft/init-backend")).unwrap_or_default();
    let backend = root
        .generation(number)
        .map_err(|e| e.to_string())?
        .init
        .backend;
    if running.trim() != backend {
        return Ok(Services::Skipped { backend });
    }
    let units = |n: u64| {
        laft_svc::runit::load_units(&root.path().join(format!("laft/gens/{n}/services")))
            .map_err(|e| e.to_string())
    };
    let old = previous.map_or_else(|| Ok(Default::default()), units)?;
    let plan = reconcile::plan(&old, &units(number)?);
    let applied = reconcile::apply(
        &plan,
        root.path(),
        &root.path().join("run/laft/system/services"),
    );
    Ok(Services::Reconciled {
        reboot: plan.reboot,
        applied,
    })
}

/// The generation `run/laft/system` points at, such as `mounts/41`.
fn active(root: &Root) -> Option<u64> {
    generation_link(&root.path().join("run/laft/system"))
}

/// The generation the machine booted, which `run/laft/booted` names.
fn booted(root: &Root) -> Option<u64> {
    generation_link(&root.path().join("run/laft/booted"))
}

fn generation_link(link: &Path) -> Option<u64> {
    fs::read_link(link)
        .ok()?
        .file_name()?
        .to_str()?
        .parse()
        .ok()
}

/// Points the symlink `link` at `target` with one rename, and syncs its
/// directory so that the new link survives a crash.
fn replace_link(link: &Path, target: &str) -> std::io::Result<()> {
    let name = link.file_name().and_then(|n| n.to_str()).unwrap_or("link");
    let tmp = link.with_file_name(format!(".{name}.new"));
    match fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    symlink(target, &tmp)?;
    fs::rename(&tmp, link)?;
    fs::File::open(link.parent().unwrap_or(Path::new("/")))?.sync_all()
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
