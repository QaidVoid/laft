//! Activation of a running system: the steps run in `laft-activate`, each
//! recorded in the store's journal, and the mounts in the initial mount
//! namespace go through the coordinator.

use std::collections::BTreeSet;

use laft_activate::{InitialNamespace, LiveActivation, Services};
use laft_store::{Activation as Journaled, Lock};

use crate::activate::Activation;
use crate::env::Env;
use crate::failure::Failure;
use crate::worker::{self, Request};

/// The steps of a live activation that the coordinator carries out in the
/// initial mount namespace.
pub(crate) struct Coordinator<'e> {
    env: &'e Env,
}

impl<'e> Coordinator<'e> {
    pub(crate) fn new(env: &'e Env) -> Self {
        Self { env }
    }
}

impl InitialNamespace for Coordinator<'_> {
    fn mount(&mut self, number: u64) -> Result<(), String> {
        worker::request(self.env, &Request::Mount { generation: number })
            .map(drop)
            .map_err(|f| f.message)
    }

    fn unmount(&mut self, keep: &[u64]) -> Result<(), String> {
        worker::request(
            self.env,
            &Request::Unmount {
                keep: keep.to_vec(),
            },
        )
        .map(drop)
        .map_err(|f| f.message)
    }

    fn services(&mut self, previous: Option<u64>, number: u64) -> Result<Services, String> {
        let request = Request::Services {
            previous,
            generation: number,
        };
        worker::request(self.env, &request)
            .map_err(|f| f.message)?
            .ok_or_else(|| "the coordinator did not report on the services".to_owned())
    }
}

/// Runs the live activation `journaled` of a generation and makes it the
/// default.
///
/// Conflicts in `/etc` stop the switch before anything changes. Then the
/// generation is mounted, and renaming `/run/laft/system` is the moment
/// the running system uses it. After that, every remaining step runs even
/// when one fails, and failures are reported in the result.
pub(crate) fn activate(
    env: &Env,
    lock: &Lock,
    journaled: Journaled<'_>,
    adopt: &BTreeSet<String>,
) -> Result<Activation, Failure> {
    let conflicts = laft_activate::check_etc(lock, journaled.generation(), adopt)?;
    if !conflicts.is_empty() {
        journaled.abandon()?;
        return Err(laft_activate::Error::Conflicts(conflicts).into());
    }
    let done = laft_activate::activate_live(lock, journaled, adopt, &mut Coordinator::new(env))?;
    Ok(report(env, done))
}

/// Finishes or undoes a live activation that an earlier command left
/// interrupted, and says what it did.
pub(crate) fn resume(env: &Env, lock: &Lock) -> Result<(), Failure> {
    let Some(done) = laft_activate::resume_live(lock, &mut Coordinator::new(env))? else {
        return Ok(());
    };
    let number = done.generation;
    if done.active {
        let activation = report(env, done);
        for (step, why) in &activation.failed {
            env.ui.warn(format!("activation step {step} failed: {why}"));
        }
        env.ui.note(format!(
            "recovered   the interrupted switch to generation {number} was finished"
        ));
    } else {
        env.ui.note(format!(
            "recovered   the interrupted switch to generation {number} was rolled back"
        ));
    }
    Ok(())
}

/// Prints the warnings of `done` and turns it into the command's report.
fn report(env: &Env, done: LiveActivation) -> Activation {
    for warning in &done.warnings {
        env.ui.warn(warning.to_string());
    }
    let mut activation = Activation {
        active: done.active,
        done: done.done,
        failed: done.failed,
        ..Activation::default()
    };
    match done.services {
        Some(Services::Skipped { backend }) => env.ui.note(format!(
            "services    skipped because the running init is not {backend}. Reboot to use it."
        )),
        Some(Services::Reconciled { reboot, applied }) => {
            for name in &reboot {
                env.ui
                    .note(format!("services    {name} changes on the next boot"));
            }
            for (name, why) in &applied.failed {
                env.ui.warn(format!("service {name}: {why}"));
            }
            activation.services_failed = applied
                .not_ready
                .into_iter()
                .chain(applied.failed.into_iter().map(|(n, _)| n))
                .collect();
        }
        None => {}
    }
    activation
}
