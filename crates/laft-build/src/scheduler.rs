//! The builds of one run, which start by readiness and run side by side.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;
use std::sync::Mutex;
use std::sync::mpsc;

use laft_sandbox::terminated;

use crate::BuildError;
use crate::graph::Item;

/// How long a build without a record is expected to take.
const DEFAULT_SECONDS: u64 = 60;

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
pub(crate) struct Scheduler {
    /// The place of every item of the run in the plan order.
    position: BTreeMap<Item, usize>,
    /// The builds of the run that each build waits for.
    deps: BTreeMap<Item, BTreeSet<Item>>,
    /// The builds that wait for each build.
    dependents: BTreeMap<Item, BTreeSet<Item>>,
    /// How long each build took before, in seconds, 0 when unknown.
    durations: BTreeMap<Item, u64>,
    /// How many builds may run at once.
    workers: usize,
    /// The builds that wait to start.
    pending: BTreeSet<Item>,
    /// The builds that run.
    running: BTreeSet<Item>,
    /// The builds that need no waiting, because they are finished or
    /// because the run does not build them.
    settled: BTreeSet<Item>,
    /// The builds this run has built.
    built: BTreeSet<Item>,
    /// Whether nothing new may start.
    stopped: bool,
}

impl Scheduler {
    /// The scheduler of the run `order`, whose builds wait for `deps` within
    /// the run and whose builds took `durations` before, in seconds, with
    /// 0 for unknown. Every build of `order` that is not handed to
    /// [`Scheduler::run`] counts as settled.
    pub(crate) fn new(
        order: Vec<Item>,
        deps: BTreeMap<Item, BTreeSet<Item>>,
        durations: BTreeMap<Item, u64>,
        workers: usize,
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
            workers: workers.max(1),
            pending: BTreeSet::new(),
            running: BTreeSet::new(),
            settled: order.into_iter().collect(),
            built: BTreeSet::new(),
            stopped: false,
        }
    }

    /// Runs the builds of `initial` with `build`, up to `workers` at once.
    /// `prepare` makes the data of one build as it starts, `settled`
    /// records a finished build and returns the builds it made necessary,
    /// which join the run unless they already ran.
    ///
    /// Returns every build it built, in the order they finished.
    pub(crate) fn run<J, T>(
        &mut self,
        initial: impl IntoIterator<Item = Item>,
        prepare: &mut dyn FnMut(&Item) -> J,
        build: &(dyn Fn(&Item, J) -> Result<T, BuildError> + Sync),
        settled: Settled<'_, T>,
    ) -> Result<Vec<(Item, T)>, Failed>
    where
        J: Send,
        T: Send,
    {
        for item in initial {
            self.admit(item);
        }
        let (jobs, waiting) = mpsc::channel::<(Item, J)>();
        let waiting = Mutex::new(waiting);
        let (finished, results) = mpsc::channel::<(Item, Result<T, BuildError>)>();
        let mut jobs = Some(jobs);
        let mut outcomes = Vec::new();
        let mut failures: Vec<(usize, Item, Box<BuildError>)> = Vec::new();
        std::thread::scope(|scope| {
            for _ in 0..self.workers {
                scope.spawn(|| {
                    loop {
                        let job = {
                            let waiting = waiting.lock().expect("the job queue cannot be poisoned");
                            waiting.recv()
                        };
                        match job {
                            Ok((item, data)) => {
                                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                                    build(&item, data)
                                }))
                                .unwrap_or_else(|_| {
                                    Err(BuildError::problem(
                                        "B040",
                                        format!("the build of {item} panicked"),
                                    ))
                                });
                                let _ = finished.send((item, result));
                            }
                            Err(_) => break,
                        }
                    }
                });
            }
            loop {
                self.dispatch(
                    prepare,
                    jobs.as_ref()
                        .expect("the job sender runs until the run ends"),
                );
                if self.running.is_empty() {
                    break;
                }
                let Ok((item, result)) = results.recv() else {
                    break;
                };
                self.running.remove(&item);
                match result {
                    Ok(outcome) => {
                        match settled(&item, &outcome) {
                            Ok(made) => {
                                for item in made {
                                    self.admit(item);
                                }
                            }
                            Err(error) => {
                                self.stopped = true;
                                failures.push((self.place(&item), item.clone(), Box::new(error)));
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
            drop(jobs.take());
        });
        if let Some((_, item, error)) = failures.into_iter().min_by_key(|(place, _, _)| *place) {
            return Err(Failed::Build { item, error });
        }
        if !self.pending.is_empty() {
            return Err(Failed::Interrupted);
        }
        Ok(outcomes)
    }

    /// Starts every ready build until `workers` run or nothing new may
    /// start.
    fn dispatch<J>(&mut self, prepare: &mut dyn FnMut(&Item) -> J, jobs: &mpsc::Sender<(Item, J)>) {
        self.stopped = self.stopped || terminated();
        if self.stopped {
            return;
        }
        while self.running.len() < self.workers
            && let Some(item) = self.next_ready()
        {
            self.pending.remove(&item);
            self.running.insert(item.clone());
            let data = prepare(&item);
            if jobs.send((item, data)).is_err() {
                break;
            }
        }
    }

    /// The ready build with the longest chain of builds after it, ties by
    /// the plan order.
    fn next_ready(&self) -> Option<Item> {
        let weights = self.weights();
        self.pending
            .iter()
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

    /// Adds `item` to the run, unless it already ran or runs.
    fn admit(&mut self, item: Item) {
        if self.built.contains(&item) || self.running.contains(&item) {
            return;
        }
        self.settled.remove(&item);
        self.pending.insert(item);
    }
}
