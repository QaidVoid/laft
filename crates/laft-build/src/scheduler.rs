//! The builds of one run, which start by readiness and run side by side.

use std::any::Any;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use laft_sandbox::terminated;

use crate::BuildError;
use crate::graph::Item;
use crate::jobserver::Jobserver;

/// How long a build without a record is expected to take.
const DEFAULT_SECONDS: u64 = 60;

/// What a build without a record is expected to need: one gibibyte.
pub(crate) const DEFAULT_MEMORY: u64 = 1 << 30;

/// The memory a build keeps free for the builds that run with it.
const RESERVE: u64 = 2 << 30;

/// How long a wait for memory lasts before the host is checked again.
const MEMORY_WAIT: Duration = Duration::from_secs(60);

/// The memory the builds of one run may claim, which decides which of the
/// ready ones starts.
pub(crate) struct Memory<'a> {
    /// What each build is expected to need, in bytes: the peak memory of
    /// its latest build, or a default when it never ran.
    pub estimates: BTreeMap<Item, u64>,
    /// The builds that may not sit next to each other: no two of them
    /// run at once, while other builds join whenever the memory check
    /// passes.
    pub big: BTreeSet<Item>,
    /// The memory the host holds for new work, in bytes.
    pub available: &'a (dyn Fn() -> Result<u64, BuildError> + Sync),
    /// Announces a build that waits for the memory of a host that no
    /// build uses.
    pub waiting: &'a (dyn Fn(u64, u64) + Sync),
}

/// Why a run ended before its last build.
#[derive(Debug)]
pub(crate) enum Failed {
    /// A build failed. It is the failure that comes first in the plan
    /// order, whichever build failed first.
    Build {
        /// The build that failed.
        item: Item,
        /// What went wrong.
        error: Box<BuildError>,
    },
    /// The memory of the host could not be read.
    Memory(BuildError),
    /// The process was asked to stop.
    Interrupted,
}

/// Records a finished build and returns the builds it made necessary.
pub(crate) type Settled<'a, T> = &'a mut dyn FnMut(&Item, &T) -> Result<Vec<Item>, BuildError>;

/// The builds of one run: which of them wait for which others, and which
/// of the ready ones starts next.
///
/// A build is ready when every build of the run it depends on has
/// finished. Among the ready ones, the one with the longest chain of
/// builds after it starts first, ties by the plan order. No more than
/// `workers` builds run at once, and after the first failure or an
/// interrupt nothing new starts, while the running builds finish.
pub(crate) struct Scheduler<'a> {
    /// The place of every item of the run in the plan order.
    position: BTreeMap<Item, usize>,
    /// The builds of the run that each build waits for.
    deps: BTreeMap<Item, BTreeSet<Item>>,
    /// The builds that wait for each build.
    dependents: BTreeMap<Item, BTreeSet<Item>>,
    /// How long each build took before, in seconds, 0 when unknown.
    durations: BTreeMap<Item, u64>,
    /// The memory the builds may claim.
    memory: Memory<'a>,
    /// How many builds may run at once.
    workers: usize,
    /// The builds that wait to start.
    pending: BTreeSet<Item>,
    /// The builds that run.
    running: BTreeSet<Item>,
    /// The running builds that want the host to itself.
    running_big: BTreeSet<Item>,
    /// The builds that need no waiting, because they are finished or
    /// because the run does not build them.
    settled: BTreeSet<Item>,
    /// The builds this run has built.
    built: BTreeSet<Item>,
    /// Whether nothing new may start.
    stopped: bool,
    /// How long a wait for memory lasts before the host is checked again.
    memory_wait: Duration,
}

impl<'a> Scheduler<'a> {
    /// The scheduler of the run `order`, whose builds wait for `deps` within
    /// the run and whose builds took `durations` before, in seconds, with
    /// 0 for unknown. Every build of `order` that is not handed to
    /// [`Scheduler::run`] counts as settled. `memory` decides which of the
    /// ready builds starts.
    pub(crate) fn new(
        order: Vec<Item>,
        deps: BTreeMap<Item, BTreeSet<Item>>,
        durations: BTreeMap<Item, u64>,
        workers: usize,
        memory: Memory<'a>,
    ) -> Self {
        let mut dependents: BTreeMap<Item, BTreeSet<Item>> = BTreeMap::new();
        for (item, waits) in &deps {
            for dep in waits {
                dependents
                    .entry(dep.clone())
                    .or_default()
                    .insert(item.clone());
            }
        }
        Self {
            position: order
                .iter()
                .cloned()
                .enumerate()
                .map(|(i, item)| (item, i))
                .collect(),
            deps,
            dependents,
            durations,
            memory,
            workers: workers.max(1),
            pending: BTreeSet::new(),
            running: BTreeSet::new(),
            running_big: BTreeSet::new(),
            settled: order.into_iter().collect(),
            built: BTreeSet::new(),
            stopped: false,
            memory_wait: MEMORY_WAIT,
        }
    }

    /// Sets how long a wait for memory lasts before the host is checked
    /// again, for tests that cannot wait a minute.
    #[cfg(test)]
    pub(crate) fn memory_wait(mut self, step: Duration) -> Self {
        self.memory_wait = step;
        self
    }

    /// Runs the builds of `initial` with `build`, up to `workers` at once.
    /// `prepare` makes the data of one build as it starts, `settled`
    /// records a finished build and returns the builds it made necessary,
    /// which join the run unless they already ran. When a build joins a
    /// `jobserver`, the pipe is drained and refilled whenever no build
    /// runs, which restores the tokens a killed build lost. A build starts
    /// only when `memory` says the host holds what it needs. A panic of
    /// `prepare` or `settled` fails the run with the panic's message
    /// instead of leaving the workers waiting. Release builds run with
    /// `panic = "abort"`, so there a panic of the coordinator ends the
    /// process; the conversion to a failed run covers the profiles that
    /// unwind, like the tests.
    ///
    /// Returns every build it built, in the order they finished.
    pub(crate) fn run<J, T>(
        &mut self,
        initial: impl IntoIterator<Item = Item>,
        prepare: &mut dyn FnMut(&Item) -> J,
        build: &(dyn Fn(&Item, J) -> Result<T, BuildError> + Sync),
        settled: Settled<'_, T>,
        jobserver: Option<&Jobserver>,
    ) -> Result<Vec<(Item, T)>, Failed>
    where
        J: Send,
        T: Send,
    {
        for item in initial {
            self.admit(item);
        }
        let (jobs, waiting_queue) = mpsc::channel::<(Item, J)>();
        let waiting_queue = Mutex::new(waiting_queue);
        let (finished, results) = mpsc::channel::<(Item, Result<T, BuildError>)>();
        let mut outcomes = Vec::new();
        let mut failures: Vec<(usize, Item, Box<BuildError>)> = Vec::new();
        let mut lost = None;
        let mut panicking = None;
        std::thread::scope(|scope| {
            for _ in 0..self.workers {
                scope.spawn(|| {
                    loop {
                        let job = {
                            let waiting_queue = waiting_queue
                                .lock()
                                .expect("the job queue cannot be poisoned");
                            waiting_queue.recv()
                        };
                        match job {
                            Ok((item, data)) => {
                                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                                    build(&item, data)
                                }))
                                .unwrap_or_else(|payload| {
                                    Err(BuildError::problem(
                                        "B040",
                                        format!(
                                            "the build of {item} panicked{}",
                                            panic_reason(payload)
                                        ),
                                    ))
                                });
                                let _ = finished.send((item, result));
                            }
                            Err(_) => break,
                        }
                    }
                });
            }
            // The sender lives in the scope, so unwinding closes the
            // channel before the workers are joined, which wakes them.
            let mut jobs = Some(jobs);
            let mut settling_item = None;
            let finished_loop = std::panic::catch_unwind(AssertUnwindSafe(|| {
                loop {
                    if let Err(error) = self.dispatch(
                        prepare,
                        jobs.as_ref()
                            .expect("the job sender runs until the run ends"),
                    ) {
                        self.stopped = true;
                        lost = Some(error);
                        break;
                    }
                    if self.running.is_empty() {
                        break;
                    }
                    let Ok((item, result)) = results.recv() else {
                        break;
                    };
                    self.running.remove(&item);
                    self.running_big.remove(&item);
                    if self.running.is_empty()
                        && let Some(jobserver) = jobserver
                    {
                        jobserver.refill();
                    }
                    match result {
                        Ok(outcome) => {
                            settling_item = Some(item.clone());
                            match settled(&item, &outcome) {
                                Ok(made) => {
                                    for item in made {
                                        self.admit(item);
                                    }
                                }
                                Err(error) => {
                                    self.stopped = true;
                                    failures.push((
                                        self.place(&item),
                                        item.clone(),
                                        Box::new(error),
                                    ));
                                }
                            }
                            self.built.insert(item.clone());
                            self.settled.insert(item.clone());
                            outcomes.push((item, outcome));
                        }
                        Err(error) => {
                            self.stopped = true;
                            failures.push((self.place(&item), item, Box::new(error)));
                        }
                    }
                }
            }));
            drop(jobs.take());
            if let Err(payload) = finished_loop {
                let item = self
                    .unfinished()
                    .or(settling_item)
                    .expect("the run panicked over a build it held");
                panicking = Some((item, panic_reason(payload)));
            }
        });
        if let Some((item, reason)) = panicking {
            return Err(Failed::Build {
                item,
                error: Box::new(BuildError::problem(
                    "B040",
                    format!("the run panicked{reason}"),
                )),
            });
        }
        if let Some(error) = lost {
            return Err(Failed::Memory(error));
        }
        if let Some((_, item, error)) = failures.into_iter().min_by_key(|(place, _, _)| *place) {
            return Err(Failed::Build { item, error });
        }
        if !self.pending.is_empty() {
            return Err(Failed::Interrupted);
        }
        Ok(outcomes)
    }

    /// Starts every ready build that fits until `workers` run or nothing
    /// new may start, and reports whether a big build waits for the
    /// memory of a host that no build uses.
    fn fill<J>(
        &mut self,
        prepare: &mut dyn FnMut(&Item) -> J,
        jobs: &mpsc::Sender<(Item, J)>,
    ) -> Result<bool, BuildError> {
        let available = self.memory.available;
        let mut deferred = BTreeSet::new();
        let mut waits = false;
        while self.running.len() < self.workers
            && let Some(item) = self.next_ready(&deferred)
        {
            let estimate = self.estimate(&item);
            let big = self.memory.big.contains(&item);
            let fits = if self.running.is_empty() && !big {
                true
            } else {
                let free = available()?;
                if self.running.is_empty() {
                    free >= estimate
                } else {
                    (!big || self.running_big.is_empty()) && free >= estimate + RESERVE
                }
            };
            if !fits {
                deferred.insert(item);
                waits |= big;
                continue;
            }
            self.pending.remove(&item);
            self.running.insert(item.clone());
            if big {
                self.running_big.insert(item.clone());
            }
            let data = prepare(&item);
            if jobs.send((item, data)).is_err() {
                break;
            }
        }
        Ok(waits)
    }

    /// Starts every build that fits until `workers` run or nothing new
    /// may start. When only big builds are ready and none fits a host
    /// that no build uses, waits for the memory of the one that comes
    /// first, checking every minute, and keeps waiting and starting until
    /// a build runs, the run is asked to stop, or the memory cannot be
    /// read, so memory that another tenant of the host takes away while
    /// the run waits never ends the run.
    fn dispatch<J>(
        &mut self,
        prepare: &mut dyn FnMut(&Item) -> J,
        jobs: &mpsc::Sender<(Item, J)>,
    ) -> Result<(), BuildError> {
        self.stopped = self.stopped || terminated();
        if self.stopped {
            return Ok(());
        }
        let mut waits = self.fill(prepare, jobs)?;
        while self.running.is_empty() && waits && !self.stopped {
            let item = self
                .next_ready(&BTreeSet::new())
                .expect("a big build waits for an idle host");
            let needed = self.estimate(&item);
            let (available, waiting) = (self.memory.available, self.memory.waiting);
            loop {
                if terminated() {
                    self.stopped = true;
                    break;
                }
                let free = available()?;
                if free >= needed {
                    break;
                }
                waiting(needed, free);
                std::thread::sleep(self.memory_wait);
            }
            if self.stopped {
                break;
            }
            waits = self.fill(prepare, jobs)?;
            // The host looked free a moment ago, but the read of the fill
            // deferred the build again, so the memory came and went
            // between the reads. A wait bounds the spin before the next
            // look.
            if self.running.is_empty() && waits && !self.stopped {
                std::thread::sleep(self.memory_wait);
            }
        }
        Ok(())
    }

    /// What `item` is expected to need, in bytes.
    fn estimate(&self, item: &Item) -> u64 {
        self.memory.estimates.get(item).copied().unwrap_or(0)
    }

    /// The ready build with the longest chain of builds after it, ties by
    /// the plan order, leaving out the builds in `deferred`, which wait
    /// for memory.
    fn next_ready(&self, deferred: &BTreeSet<Item>) -> Option<Item> {
        let weights = self.weights();
        self.pending
            .iter()
            .filter(|item| !deferred.contains(*item))
            .filter(|item| {
                self.deps
                    .get(*item)
                    .into_iter()
                    .flatten()
                    .all(|dep| self.settled.contains(dep))
            })
            .min_by_key(|item| {
                (
                    Reverse(weights.get(*item).copied().unwrap_or(0)),
                    self.place(item),
                )
            })
            .cloned()
    }

    /// The length of the longest chain of builds that starts at each build
    /// that is not settled, its own build included.
    fn weights(&self) -> BTreeMap<Item, u64> {
        let remaining: BTreeSet<Item> = self.pending.union(&self.running).cloned().collect();
        let mut waiting: BTreeMap<&Item, usize> = remaining.iter().map(|i| (i, 0)).collect();
        for item in &remaining {
            for dep in self.deps.get(item).into_iter().flatten() {
                if remaining.contains(dep) {
                    *waiting
                        .get_mut(item)
                        .expect("every remaining build is counted") += 1;
                }
            }
        }
        let mut order: Vec<&Item> = waiting
            .iter()
            .filter(|(_, waits)| **waits == 0)
            .map(|(item, _)| *item)
            .collect();
        let mut ready: Vec<&Item> = order.clone();
        while let Some(item) = ready.pop() {
            for follower in self.dependents.get(item).into_iter().flatten() {
                if let Some(waits) = waiting.get_mut(follower) {
                    *waits -= 1;
                    if *waits == 0 {
                        order.push(follower);
                        ready.push(follower);
                    }
                }
            }
        }
        let mut weights = BTreeMap::new();
        for item in order.into_iter().rev() {
            let longest = self
                .dependents
                .get(item)
                .into_iter()
                .flatten()
                .filter(|f| remaining.contains(*f))
                .filter_map(|f| weights.get(f).copied())
                .max()
                .unwrap_or(0);
            weights.insert(item.clone(), self.duration(item) + longest);
        }
        weights
    }

    /// How long `item` is expected to take.
    fn duration(&self, item: &Item) -> u64 {
        match self.durations.get(item) {
            Some(0) | None => DEFAULT_SECONDS,
            Some(seconds) => *seconds,
        }
    }

    /// The place of `item` in the plan order.
    fn place(&self, item: &Item) -> usize {
        self.position.get(item).copied().unwrap_or(usize::MAX)
    }

    /// The first build that has not finished, in the plan order.
    fn unfinished(&self) -> Option<Item> {
        self.pending
            .union(&self.running)
            .min_by_key(|item| self.place(item))
            .cloned()
    }

    /// Adds `item` to the run, unless it already ran or runs.
    fn admit(&mut self, item: Item) {
        if self.built.contains(&item) || self.running.contains(&item) {
            return;
        }
        self.settled.remove(&item);
        self.pending.insert(item);
    }
}

/// The message of a panic payload, when it carries one.
fn panic_reason(payload: Box<dyn Any + Send>) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned());
    match message {
        Some(message) => format!(": {message}"),
        None => String::new(),
    }
}
