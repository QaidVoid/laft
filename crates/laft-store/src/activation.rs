//! Live activation, recorded in the transaction journal step by step.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::journal::{Journal, boot_id, txid};
use crate::transaction::{same_boot, set_default};
use crate::{Lock, State, Transaction, fault, fsx};

/// The live activation of a generation on a running system, recorded in
/// the journal step by step.
///
/// The store knows the steps only by name. The caller lists them when the
/// activation starts and records each one as it finishes, so that after an
/// interruption [`Activation::finished`] tells where it stopped. Recovery
/// keeps the journal of an activation interrupted during the current boot,
/// and [`Lock::interrupted_activation`] hands it back to be finished with
/// [`Activation::finish`] or given up with [`Activation::abandon`]. An
/// activation interrupted before a reboot is dropped by recovery, because
/// the machine then runs the generation it booted.
///
/// Dropping an activation without finishing or abandoning it leaves the
/// journal in place, exactly as a crash would.
///
/// ```no_run
/// use laft_store::Root;
///
/// let root = Root::default();
/// let lock = root.lock("switch")?;
/// lock.recover()?;
/// let mut activation = lock.activate("switch", 7, Some(6), &["mount", "switch"])?;
/// while activation.finished() < activation.steps().len() {
///     // Run the step `activation.steps()[activation.finished()]` here.
///     activation.step_finished()?;
/// }
/// activation.finish()?;
/// # Ok::<(), laft_store::Error>(())
/// ```
#[derive(Debug)]
pub struct Activation<'l> {
    lock: &'l Lock,
    journal: Journal,
    generation: u64,
}

impl<'l> Activation<'l> {
    /// Records `journal` as an activation with `steps` and none finished.
    fn start(
        lock: &'l Lock,
        mut journal: Journal,
        generation: u64,
        steps: &[&str],
    ) -> Result<Self> {
        if steps.is_empty() {
            return Err(Error::Invalid("an activation has at least one step".into()));
        }
        journal.state = State::Activating;
        journal.activation = steps.iter().map(|s| (*s).to_owned()).collect();
        journal.finished = 0;
        journal.save(lock.root())?;
        fault::point("activate.started")?;
        Ok(Self {
            lock,
            journal,
            generation,
        })
    }

    /// The generation being activated.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The generation the running system used before, if one is known.
    pub fn previous(&self) -> Option<u64> {
        self.journal.previous
    }

    /// The names of the steps, in the order they run.
    pub fn steps(&self) -> &[String] {
        &self.journal.activation
    }

    /// How many steps have finished, which is also the position of the
    /// next one in [`Activation::steps`].
    pub fn finished(&self) -> usize {
        self.journal.finished
    }

    /// Records that the next step finished.
    ///
    /// # Errors
    ///
    /// Fails when every step has finished already, and when the journal
    /// cannot be written.
    pub fn step_finished(&mut self) -> Result<()> {
        if self.journal.finished >= self.journal.activation.len() {
            return Err(Error::Invalid("every activation step has finished".into()));
        }
        self.journal.finished += 1;
        self.journal.save(self.lock.root())?;
        fault::point("activate.step")
    }

    /// Records that the running system uses the generation, makes it the
    /// default with one rename and ends the journal. Returns the
    /// generation's number.
    ///
    /// # Errors
    ///
    /// Fails when the journal cannot be written or the default cannot be
    /// replaced. Recovery then makes the generation the default.
    pub fn finish(mut self) -> Result<u64> {
        let number = self.generation();
        let root = self.lock.root();
        self.journal.state = State::Activated;
        self.journal.save(root)?;
        fault::point("activated")?;
        set_default(root, number, &self.journal.txid)?;
        fault::point("default.linked")?;
        self.journal.state = State::Complete;
        self.journal.save(root)?;
        fault::point("complete")?;
        self.journal.remove(root)?;
        Ok(number)
    }

    /// Ends the activation without making the generation the default. The
    /// generation stays, and nothing else changes in the store.
    ///
    /// # Errors
    ///
    /// Fails when the journal cannot be removed.
    pub fn abandon(self) -> Result<()> {
        let root = self.lock.root();
        fsx::clear_dir(&root.tmp_dir())?;
        self.journal.remove(root)
    }
}

impl<'l> Transaction<'l> {
    /// Starts the live activation of the generation this transaction
    /// built, with the step names `steps` in the order they run. The
    /// transaction continues as the returned [`Activation`].
    ///
    /// # Errors
    ///
    /// Fails when the generation was not assembled, when `steps` is empty,
    /// and when the journal cannot be written.
    pub fn activate(self, steps: &[&str]) -> Result<Activation<'l>> {
        let generation = self.generation();
        let (lock, journal) = self.into_activation()?;
        Activation::start(lock, journal, generation, steps)
    }
}

impl Lock {
    /// Starts the live activation of the existing generation `number`, which
    /// replaces `previous` on the running system, for `command` and with
    /// the step names `steps` in the order they run.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::Pending`] while an interrupted transaction awaits
    /// recovery, when the generation has no valid `gen.json`, when `steps`
    /// is empty, and when the journal cannot be written.
    pub fn activate(
        &self,
        command: &str,
        number: u64,
        previous: Option<u64>,
        steps: &[&str],
    ) -> Result<Activation<'_>> {
        let root = self.root();
        if let Some(journal) = Journal::all(root)?.into_iter().next() {
            return Err(Error::Pending {
                txid: journal.txid,
                state: journal.state,
            });
        }
        root.generation(number)?;
        let journal = Journal::new(
            txid(),
            command.to_owned(),
            number,
            previous,
            BTreeMap::new(),
        );
        Activation::start(self, journal, number, steps)
    }

    /// The activation that was interrupted during the current boot, which
    /// [`Lock::recover`] keeps for its caller to finish or abandon.
    ///
    /// # Errors
    ///
    /// Fails when a journal cannot be read.
    pub fn interrupted_activation(&self) -> Result<Option<Activation<'_>>> {
        let current = boot_id();
        Ok(Journal::all(self.root())?
            .into_iter()
            .filter(|j| j.state == State::Activating && same_boot(&j.boot_id, &current))
            .find_map(|journal| {
                Some(Activation {
                    lock: self,
                    generation: journal.generation?,
                    journal,
                })
            }))
    }
}

#[cfg(test)]
#[path = "activation_tests.rs"]
mod tests;
