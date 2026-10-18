//! The background fetch loop.
//!
//! The worker runs on its own thread. It asks the [`Source`] for the runs of
//! every repository, sends each answer to the screen as an [`Update`], and
//! then waits for the next cycle. The screen sends [`Command`] values back.
//!
//! The screen also tells the worker which runs it shows expanded, with
//! [`Command::WatchJobs`]. The worker fetches the jobs of those runs. It fetches
//! the jobs of a new run at once, and it fetches the jobs of a watched run on
//! each cycle when the run is due. A run is due when it has no good job fetch
//! yet, when it is active, or when its `updated_at` changed since the last good
//! fetch. So `ha` fetches the jobs of a finished run one time only. Job requests
//! and run requests share the [`CONCURRENCY`] limit and the rate limit backoff.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::github::{RunQuery, Source, SourceError};
use crate::job::Job;
use crate::run::{Run, RunId};
use crate::settings::Settings;
use crate::status::RateLimit;
use crate::target::{OwnerName, RepoName, Target};

/// How many repositories the worker fetches at the same time.
pub const CONCURRENCY: usize = 8;

/// How often the worker lists the repositories of the owners and sweeps for
/// active runs. The sweep finds runs that started before the window. An owner
/// with no good list is listed on every cycle until a list succeeds.
pub const SLOW_CADENCE: Duration = Duration::from_secs(600);

/// One message from the worker to the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// The repositories to watch now, and the owners whose list failed or was cut.
    Repos {
        /// Every repository to watch, without duplicates.
        repos: BTreeSet<RepoName>,
        /// The owners whose list failed or was cut, with the text to show.
        owner_errors: Vec<(OwnerName, String)>,
    },
    /// The runs of one repository.
    Runs {
        /// The repository that holds the runs.
        repo: RepoName,
        /// The runs to show for the repository.
        runs: Vec<Run>,
    },
    /// The fetch for one repository failed.
    Failed {
        /// The repository that failed.
        repo: RepoName,
        /// The error text.
        error: String,
    },
    /// The jobs of one run that the screen shows expanded.
    Jobs {
        /// The run that holds the jobs.
        run: RunId,
        /// The jobs of the run.
        jobs: Vec<Job>,
    },
    /// The fetch of the jobs of one run failed.
    JobsFailed {
        /// The run whose jobs failed.
        run: RunId,
        /// The error text.
        error: String,
    },
    /// A cycle ended.
    CycleDone {
        /// When the cycle ended.
        finished_at: DateTime<Utc>,
        /// The rate limit that GitHub last stated.
        rate: Option<RateLimit>,
        /// Set while the worker waits for the rate limit to reset.
        backoff_until: Option<DateTime<Utc>>,
    },
}

/// One message from the screen to the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start the next cycle now.
    RefreshNow,
    /// The runs whose jobs the screen shows now. The set is whole: the screen
    /// sends the full set each time it changes.
    WatchJobs(BTreeSet<(RepoName, RunId)>),
}

/// The two ends of the worker thread.
#[derive(Debug)]
pub struct Worker {
    /// The messages from the worker.
    pub updates: Receiver<Update>,
    /// The messages to the worker. When this sender drops, the thread ends.
    pub commands: Sender<Command>,
}

/// Tells when a periodic job is due.
#[derive(Debug, Clone, Copy)]
pub struct Cadence {
    period: Duration,
    last: Option<Instant>,
}

impl Cadence {
    /// Makes a cadence that is due again `period` after each mark.
    #[must_use]
    pub fn new(period: Duration) -> Cadence {
        Cadence { period, last: None }
    }

    /// True when the job is due at `now`.
    #[must_use]
    pub fn due(&self, now: Instant) -> bool {
        self.last
            .is_none_or(|last| now.saturating_duration_since(last) >= self.period)
    }

    /// Records that the job ran at `now`.
    pub fn mark(&mut self, now: Instant) {
        self.last = Some(now);
    }
}

/// Decides how long the worker waits after a cycle.
///
/// Returns the delay and, while the worker slows down for a low rate limit,
/// the time the limit resets. The delay is never shorter than `interval`.
#[must_use]
pub fn next_delay(
    interval: Duration,
    rate: Option<RateLimit>,
    now: DateTime<Utc>,
) -> (Duration, Option<DateTime<Utc>>) {
    let Some(rate) = rate.filter(|rate| rate.is_low() && rate.reset > now) else {
        return (interval, None);
    };
    // A reset in the future always has a positive distance.
    let until_reset = (rate.reset - now).to_std().unwrap_or(interval);
    (until_reset.max(interval), Some(rate.reset))
}

/// Starts the worker thread with the real slow cadence.
#[must_use]
pub fn spawn<S: Source + 'static>(source: S, settings: &Settings) -> Worker {
    spawn_with(source, settings, SLOW_CADENCE)
}

fn spawn_with<S: Source + 'static>(source: S, settings: &Settings, slow: Duration) -> Worker {
    let (updates_tx, updates) = mpsc::channel();
    let (commands, commands_rx) = mpsc::channel();
    let settings = settings.clone();
    std::thread::Builder::new()
        .name("ha-worker".to_owned())
        .spawn(move || run_loop(&source, &settings, slow, &updates_tx, &commands_rx))
        .expect("the system can start the worker thread");
    Worker { updates, commands }
}

/// The receiver of the updates is gone, so the thread has no reason to run.
struct ReceiverGone;

/// A run, named by its repository and its ID.
type RunKey = (RepoName, RunId);

/// What the worker knows about one run from a list of runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RunFacts {
    /// True when the run is queued or running.
    active: bool,
    /// When GitHub last changed the run. A re-run changes this time.
    updated_at: DateTime<Utc>,
}

impl RunFacts {
    fn of(run: &Run) -> RunFacts {
        RunFacts {
            active: run.state.is_active(),
            updated_at: run.updated_at,
        }
    }
}

/// What the worker keeps from one cycle to the next.
struct State {
    owner_cadence: Cadence,
    owner_lists: BTreeMap<OwnerName, Vec<RepoName>>,
    /// The text to show for each owner whose last list failed or was cut.
    owner_errors: BTreeMap<OwnerName, String>,
    sweep_cadence: Cadence,
    /// The active runs of the last good sweep, for each repository.
    sweeps: Mutex<HashMap<RepoName, Vec<Run>>>,
    /// The runs whose jobs the screen shows now (the last `WatchJobs`).
    watched: BTreeSet<RunKey>,
    /// The facts of each run of the last good answer, for each repository.
    facts: Mutex<HashMap<RepoName, HashMap<RunId, RunFacts>>>,
    /// For each watched run, the facts of the run at its last good job fetch.
    job_marks: Mutex<HashMap<RunKey, RunFacts>>,
}

impl State {
    fn new(slow: Duration) -> State {
        State {
            owner_cadence: Cadence::new(slow),
            owner_lists: BTreeMap::new(),
            owner_errors: BTreeMap::new(),
            sweep_cadence: Cadence::new(slow),
            sweeps: Mutex::new(HashMap::new()),
            watched: BTreeSet::new(),
            facts: Mutex::new(HashMap::new()),
            job_marks: Mutex::new(HashMap::new()),
        }
    }

    /// Takes a new set of watched runs. Returns the runs that are new in it.
    /// A run that leaves the set loses its mark, so a later expand fetches again.
    fn watch(&mut self, set: BTreeSet<RunKey>) -> Vec<RunKey> {
        self.job_marks
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|key, _| set.contains(key));
        let added = set.difference(&self.watched).cloned().collect();
        self.watched = set;
        added
    }

    /// The watched runs whose jobs the next cycle fetches.
    fn due_jobs(&mut self) -> Vec<RunKey> {
        let facts = self.facts.get_mut().unwrap_or_else(PoisonError::into_inner);
        let marks = self
            .job_marks
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        self.watched
            .iter()
            .filter(|(repo, run)| match marks.get(&(repo.clone(), *run)) {
                None => true,
                Some(mark) if mark.active => true,
                Some(mark) => facts
                    .get(repo)
                    .and_then(|runs| runs.get(run))
                    .is_some_and(|now| now.updated_at != mark.updated_at),
            })
            .cloned()
            .collect()
    }
}

fn run_loop<S: Source>(
    source: &S,
    settings: &Settings,
    slow: Duration,
    updates: &Sender<Update>,
    commands: &Receiver<Command>,
) {
    let mut state = State::new(slow);
    while let Ok((delay, backoff)) = cycle(source, settings, &mut state, updates) {
        match wait_for_next_cycle(source, &mut state, updates, commands, delay, backoff) {
            Ok(Wake::Cycle) => {}
            Ok(Wake::Stop) | Err(ReceiverGone) => return,
        }
    }
}

/// Why the worker stops waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wake {
    /// A cycle is due now.
    Cycle,
    /// The command sender is gone, so the thread ends.
    Stop,
}

/// Waits for the next cycle.
///
/// The wait ends at a deadline, `delay` after the start. The function first
/// applies each command that arrived during the last cycle, in order. A refresh
/// command in that backlog starts the next cycle at once. A refresh command
/// also ends a wait for a rate limit reset, because the person asked for it.
///
/// A [`Command::WatchJobs`] changes the watched set. The function fetches the
/// jobs of each run that is new in the set at once, and it sends the answers.
/// The wait then goes on until the same deadline. It does not fetch at once
/// when a refresh is also due, because the cycle fetches the new runs. It does
/// not fetch at once while `backoff` is true, because job requests share the
/// rate limit backoff of the run requests. The next cycle fetches those runs.
fn wait_for_next_cycle<S: Source>(
    source: &S,
    state: &mut State,
    updates: &Sender<Update>,
    commands: &Receiver<Command>,
    delay: Duration,
    backoff: bool,
) -> Result<Wake, ReceiverGone> {
    let deadline = Instant::now() + delay;
    let mut requested = false;
    let mut added: Vec<RunKey> = Vec::new();
    loop {
        match commands.try_recv() {
            Ok(command) => apply_command(command, state, &mut requested, &mut added),
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => return Ok(Wake::Stop),
        }
    }
    loop {
        if requested {
            return Ok(Wake::Cycle);
        }
        added.retain(|key| state.watched.contains(key));
        if !backoff && !added.is_empty() {
            fetch_jobs(source, state, &added, updates)?;
        }
        added.clear();
        match commands.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(command) => apply_command(command, state, &mut requested, &mut added),
            Err(RecvTimeoutError::Timeout) => return Ok(Wake::Cycle),
            Err(RecvTimeoutError::Disconnected) => return Ok(Wake::Stop),
        }
    }
}

/// Applies one command. A refresh sets `requested`. A new watched set adds its
/// new runs to `added`.
fn apply_command(
    command: Command,
    state: &mut State,
    requested: &mut bool,
    added: &mut Vec<RunKey>,
) {
    match command {
        Command::RefreshNow => *requested = true,
        Command::WatchJobs(set) => added.extend(state.watch(set)),
    }
}

/// Runs one cycle: lists the owners that need it, fetches every repository,
/// fetches the jobs of the watched runs that are due, and reports the end of
/// the cycle. Returns how long to wait before the next one, and whether the
/// worker waits for a rate limit reset.
fn cycle<S: Source>(
    source: &S,
    settings: &Settings,
    state: &mut State,
    updates: &Sender<Update>,
) -> Result<(Duration, bool), ReceiverGone> {
    let repos = refresh_repos(source, settings, state, updates)?;
    let now = Instant::now();
    let sweep_due = state.sweep_cadence.due(now);
    if sweep_due {
        state.sweep_cadence.mark(now);
    }
    state
        .sweeps
        .get_mut()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|repo, _| repos.contains(repo));
    let fetch = Fetch {
        since: window_start(settings.window),
        sweep_due,
        sweeps: &state.sweeps,
        facts: &state.facts,
    };
    fetch_all(source, &repos, &fetch, updates)?;
    state
        .facts
        .get_mut()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|repo, _| repos.contains(repo));
    let due = state.due_jobs();
    fetch_jobs(source, state, &due, updates)?;
    let now = Utc::now();
    let rate = source.rate_limit();
    let (delay, backoff_until) = next_delay(settings.interval, rate, now);
    updates
        .send(Update::CycleDone {
            finished_at: now,
            rate,
            backoff_until,
        })
        .map_err(|_| ReceiverGone)?;
    Ok((delay, backoff_until.is_some()))
}

/// What every fetch of one cycle shares.
struct Fetch<'a> {
    /// The oldest creation time to ask for.
    since: DateTime<Utc>,
    /// True when this cycle also asks for the active runs.
    sweep_due: bool,
    /// The stored sweep results. No thread holds this lock during a call.
    sweeps: &'a Mutex<HashMap<RepoName, Vec<Run>>>,
    /// The facts of the runs of each repository. A good fetch replaces the
    /// entry of its repository. No thread holds this lock during a call.
    facts: &'a Mutex<HashMap<RepoName, HashMap<RunId, RunFacts>>>,
}

/// Fetches the runs of one repository and merges in the last sweep.
///
/// The result holds every run of the recent answer. It also holds each run of
/// the last sweep that the recent answer lacks. Those runs started before the
/// window and still run.
fn fetch_repo<S: Source>(
    source: &S,
    repo: &RepoName,
    fetch: &Fetch<'_>,
) -> Result<Vec<Run>, SourceError> {
    let mut runs = source.runs_of(repo, RunQuery::CreatedSince(fetch.since))?;
    if fetch.sweep_due {
        let active = source.runs_of(repo, RunQuery::Active)?;
        fetch
            .sweeps
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(repo.clone(), active);
    }
    let known: HashSet<RunId> = runs.iter().map(|run| run.id).collect();
    if let Some(swept) = fetch
        .sweeps
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(repo)
    {
        runs.extend(swept.iter().filter(|run| !known.contains(&run.id)).cloned());
    }
    Ok(runs)
}

/// Fetches the runs of every repository with a pool of threads.
///
/// After a good fetch the function stores the facts of the runs of that
/// repository. A failed fetch keeps the old facts.
fn fetch_all<S: Source>(
    source: &S,
    repos: &BTreeSet<RepoName>,
    fetch: &Fetch<'_>,
    updates: &Sender<Update>,
) -> Result<(), ReceiverGone> {
    let repos: Vec<&RepoName> = repos.iter().collect();
    run_pool(&repos, updates, |repo| {
        match fetch_repo(source, repo, fetch) {
            Ok(runs) => {
                let facts = runs.iter().map(|run| (run.id, RunFacts::of(run))).collect();
                fetch
                    .facts
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert((*repo).clone(), facts);
                Update::Runs {
                    repo: (*repo).clone(),
                    runs,
                }
            }
            Err(error) => Update::Failed {
                repo: (*repo).clone(),
                error: error.to_string(),
            },
        }
    })
}

/// Fetches the jobs of the given runs with the shared pool of threads.
///
/// A good fetch sends [`Update::Jobs`] and marks the run with its current
/// facts. A run with no facts gets no mark. A failed fetch sends
/// [`Update::JobsFailed`] and removes the mark, so the next cycle tries again.
fn fetch_jobs<S: Source>(
    source: &S,
    state: &State,
    runs: &[RunKey],
    updates: &Sender<Update>,
) -> Result<(), ReceiverGone> {
    run_pool(runs, updates, |(repo, run)| {
        let result = source.jobs_of(repo, *run);
        let facts = state
            .facts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(repo)
            .and_then(|runs| runs.get(run))
            .copied();
        let mut marks = state
            .job_marks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match (result, facts) {
            (Ok(jobs), Some(facts)) => {
                marks.insert((repo.clone(), *run), facts);
                Update::Jobs { run: *run, jobs }
            }
            (Ok(jobs), None) => {
                marks.remove(&(repo.clone(), *run));
                Update::Jobs { run: *run, jobs }
            }
            (Err(error), _) => {
                marks.remove(&(repo.clone(), *run));
                Update::JobsFailed {
                    run: *run,
                    error: error.to_string(),
                }
            }
        }
    })
}

/// Turns each item into one [`Update`] with a pool of threads, and sends it.
///
/// The threads take items from one shared queue. No thread holds the queue
/// lock while it works. The pool uses [`CONCURRENCY`] threads at most, so every
/// kind of request shares the same limit. When the receiver of the updates is
/// gone, the threads stop and the function returns [`ReceiverGone`].
fn run_pool<T: Sync>(
    items: &[T],
    updates: &Sender<Update>,
    work: impl Fn(&T) -> Update + Sync,
) -> Result<(), ReceiverGone> {
    let queue = Mutex::new(items.iter().collect::<VecDeque<_>>());
    let gone = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..CONCURRENCY.min(items.len()) {
            scope.spawn(|| {
                while !gone.load(Ordering::Relaxed) {
                    let next = queue
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .pop_front();
                    let Some(item) = next else { break };
                    if updates.send(work(item)).is_err() {
                        gone.store(true, Ordering::Relaxed);
                    }
                }
            });
        }
    });
    if gone.load(Ordering::Relaxed) {
        Err(ReceiverGone)
    } else {
        Ok(())
    }
}

/// Builds the repository set and sends it when it can have changed.
///
/// When the owner cadence is due, the function lists every owner again. On
/// every cycle it also lists each owner that has no good list yet, so the `r`
/// key tries a failed owner again at once. An owner with a good list waits for
/// the cadence. The function keeps the last list error of each owner in
/// `state`, and it clears that error when a list succeeds. It sends
/// [`Update::Repos`] when the cadence is due or when it listed an owner.
fn refresh_repos<S: Source>(
    source: &S,
    settings: &Settings,
    state: &mut State,
    updates: &Sender<Update>,
) -> Result<BTreeSet<RepoName>, ReceiverGone> {
    let now = Instant::now();
    let due = state.owner_cadence.due(now);
    if due {
        state.owner_cadence.mark(now);
    }
    let mut listed = false;
    for target in &settings.targets {
        let Target::Owner(owner) = target else {
            continue;
        };
        if !due && state.owner_lists.contains_key(owner) {
            continue;
        }
        listed = true;
        match source.repos_of(owner) {
            Ok(list) => {
                match list.warning() {
                    Some(text) => {
                        state.owner_errors.insert(owner.clone(), text);
                    }
                    None => {
                        state.owner_errors.remove(owner);
                    }
                }
                state.owner_lists.insert(owner.clone(), list.repos);
            }
            Err(error) => {
                state.owner_errors.insert(owner.clone(), error.to_string());
            }
        }
    }
    let mut repos = BTreeSet::new();
    for target in &settings.targets {
        match target {
            Target::Repo(repo) => {
                repos.insert(repo.clone());
            }
            Target::Owner(owner) => {
                repos.extend(state.owner_lists.get(owner).into_iter().flatten().cloned());
            }
        }
    }
    if due || listed {
        let owner_errors = settings
            .targets
            .iter()
            .filter_map(|target| match target {
                Target::Owner(owner) => state
                    .owner_errors
                    .get(owner)
                    .map(|error| (owner.clone(), error.clone())),
                Target::Repo(_) => None,
            })
            .collect();
        updates
            .send(Update::Repos {
                repos: repos.clone(),
                owner_errors,
            })
            .map_err(|_| ReceiverGone)?;
    }
    Ok(repos)
}

/// The oldest creation time the worker asks for: the window plus one day, so
/// a run that started before the window and ended inside it still shows.
fn window_start(window: Duration) -> DateTime<Utc> {
    chrono::TimeDelta::from_std(window)
        .ok()
        .and_then(|window| Utc::now().checked_sub_signed(window + chrono::TimeDelta::hours(24)))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, PoisonError};

    use super::*;
    use crate::github::{RepoList, RunQuery, SourceError};
    use crate::job::Job;
    use crate::run::{RunId, RunNumber, RunState};
    use crate::target::Target;

    /// The longest a test waits for one update.
    const WAIT: Duration = Duration::from_secs(30);

    /// The answers and the call log of a [`FakeSource`].
    #[derive(Default)]
    struct Script {
        owners: HashMap<OwnerName, Result<Vec<RepoName>, SourceError>>,
        /// The owners whose list the fake gives as cut.
        cut: std::collections::HashSet<OwnerName>,
        created: HashMap<RepoName, Result<Vec<Run>, SourceError>>,
        active: HashMap<RepoName, Result<Vec<Run>, SourceError>>,
        /// The jobs of each run. A run with no entry has no jobs.
        jobs: HashMap<RunId, Result<Vec<Job>, SourceError>>,
        calls: Vec<(&'static str, String)>,
        since: Vec<DateTime<Utc>>,
        rate: Option<RateLimit>,
        in_flight: usize,
        max_in_flight: usize,
    }

    /// A [`Source`] that answers from a script and logs each call.
    #[derive(Default)]
    struct FakeSource {
        script: Mutex<Script>,
        /// How long each `runs_of` and `jobs_of` call sleeps.
        delay: Duration,
    }

    impl FakeSource {
        fn script(&self) -> std::sync::MutexGuard<'_, Script> {
            self.script.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl Source for Arc<FakeSource> {
        fn repos_of(&self, owner: &OwnerName) -> Result<RepoList, SourceError> {
            let mut script = self.script();
            script.calls.push(("owner", owner.to_string()));
            let cut = script.cut.contains(owner);
            script
                .owners
                .get(owner)
                .cloned()
                .unwrap_or(Ok(Vec::new()))
                .map(|repos| RepoList { repos, cut })
        }

        fn runs_of(&self, repo: &RepoName, query: RunQuery) -> Result<Vec<Run>, SourceError> {
            let mut script = self.script();
            let what = match query {
                RunQuery::CreatedSince(since) => {
                    script.since.push(since);
                    "created"
                }
                RunQuery::Active => "active",
            };
            script.calls.push((what, repo.to_string()));
            let answers = match query {
                RunQuery::CreatedSince(_) => &script.created,
                RunQuery::Active => &script.active,
            };
            let answer = answers.get(repo).cloned().unwrap_or(Ok(Vec::new()));
            script.in_flight += 1;
            script.max_in_flight = script.max_in_flight.max(script.in_flight);
            drop(script);
            std::thread::sleep(self.delay);
            self.script().in_flight -= 1;
            answer
        }

        fn jobs_of(&self, _repo: &RepoName, run: RunId) -> Result<Vec<Job>, SourceError> {
            let mut script = self.script();
            script.calls.push(("jobs", run.0.to_string()));
            let answer = script.jobs.get(&run).cloned().unwrap_or(Ok(Vec::new()));
            script.in_flight += 1;
            script.max_in_flight = script.max_in_flight.max(script.in_flight);
            drop(script);
            std::thread::sleep(self.delay);
            self.script().in_flight -= 1;
            answer
        }

        fn rate_limit(&self) -> Option<RateLimit> {
            self.script().rate
        }
    }

    fn repo(text: &str) -> RepoName {
        RepoName::parse(text).expect("test repository name")
    }

    fn owner(text: &str) -> OwnerName {
        OwnerName::parse(text).expect("test owner name")
    }

    fn run(repo_text: &str, id: u64) -> Run {
        let at = Utc::now();
        Run {
            id: RunId(id),
            number: RunNumber(id + 100),
            repo: repo(repo_text),
            workflow: "ci".to_owned(),
            branch: "main".to_owned(),
            title: format!("run {id}"),
            state: RunState::Passed,
            started_at: at,
            updated_at: at,
            url: format!("https://example.test/{id}"),
        }
    }

    fn active_run(repo_text: &str, id: u64) -> Run {
        Run {
            state: RunState::Running,
            ..run(repo_text, id)
        }
    }

    fn settings(targets: &[&str]) -> Settings {
        Settings {
            targets: targets
                .iter()
                .map(|text| Target::parse(text).expect("test target"))
                .collect(),
            window: Duration::from_hours(8),
            interval: Duration::from_hours(1),
        }
    }

    /// Reads updates until `CycleDone` and returns them, `CycleDone` last.
    fn one_cycle(worker: &Worker) -> Vec<Update> {
        let mut updates = Vec::new();
        loop {
            match worker.updates.recv_timeout(WAIT) {
                Ok(update) => {
                    let done = matches!(update, Update::CycleDone { .. });
                    updates.push(update);
                    if done {
                        return updates;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!("no update in {WAIT:?}; got {updates:?}")
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("the worker ended before CycleDone; got {updates:?}")
                }
            }
        }
    }

    #[test]
    fn cadence_is_due_when_never_marked() {
        let cadence = Cadence::new(Duration::from_secs(60));
        assert!(cadence.due(Instant::now()));
    }

    #[test]
    fn cadence_is_not_due_right_after_a_mark() {
        let now = Instant::now();
        let mut cadence = Cadence::new(Duration::from_secs(60));
        cadence.mark(now);
        assert!(!cadence.due(now));
        assert!(!cadence.due(now + Duration::from_secs(59)));
    }

    #[test]
    fn cadence_is_due_again_after_the_period() {
        let now = Instant::now();
        let mut cadence = Cadence::new(Duration::from_secs(60));
        cadence.mark(now);
        assert!(cadence.due(now + Duration::from_secs(60)));
    }

    #[test]
    fn cadence_with_a_zero_period_is_always_due() {
        let now = Instant::now();
        let mut cadence = Cadence::new(Duration::ZERO);
        cadence.mark(now);
        assert!(cadence.due(now));
    }

    fn rate(remaining: u64, reset: DateTime<Utc>) -> RateLimit {
        RateLimit {
            limit: 5000,
            remaining,
            reset,
        }
    }

    const INTERVAL: Duration = Duration::from_secs(60);

    #[test]
    fn the_delay_is_the_interval_when_the_rate_is_not_low() {
        let now = Utc::now();
        let reset = now + chrono::TimeDelta::hours(1);
        assert_eq!(
            next_delay(INTERVAL, Some(rate(4000, reset)), now),
            (INTERVAL, None)
        );
    }

    #[test]
    fn the_delay_is_the_interval_without_a_rate() {
        assert_eq!(next_delay(INTERVAL, None, Utc::now()), (INTERVAL, None));
    }

    #[test]
    fn a_low_rate_waits_for_a_reset_that_is_longer_than_the_interval() {
        let now = Utc::now();
        let reset = now + chrono::TimeDelta::minutes(30);
        assert_eq!(
            next_delay(INTERVAL, Some(rate(10, reset)), now),
            (Duration::from_mins(30), Some(reset))
        );
    }

    #[test]
    fn a_low_rate_keeps_the_interval_when_the_reset_is_sooner() {
        let now = Utc::now();
        let reset = now + chrono::TimeDelta::seconds(20);
        assert_eq!(
            next_delay(INTERVAL, Some(rate(10, reset)), now),
            (INTERVAL, Some(reset))
        );
    }

    #[test]
    fn a_low_rate_with_a_reset_in_the_past_changes_nothing() {
        let now = Utc::now();
        let reset = now - chrono::TimeDelta::seconds(5);
        assert_eq!(
            next_delay(INTERVAL, Some(rate(10, reset)), now),
            (INTERVAL, None)
        );
    }

    #[test]
    fn the_first_cycle_sends_the_repositories_then_the_runs_then_done() {
        let fake = Arc::new(FakeSource::default());
        fake.script()
            .owners
            .insert(owner("acme"), Ok(vec![repo("ACME/A"), repo("acme/c")]));
        fake.script()
            .created
            .insert(repo("acme/a"), Ok(vec![run("acme/a", 1)]));
        let worker = spawn_with(
            Arc::clone(&fake),
            &settings(&["acme", "acme/a", "other/b"]),
            SLOW_CADENCE,
        );

        let updates = one_cycle(&worker);

        let Update::Repos {
            repos,
            owner_errors,
        } = &updates[0]
        else {
            panic!("the first update is not Repos: {updates:?}");
        };
        assert_eq!(repos.len(), 3, "{repos:?}");
        assert!(owner_errors.is_empty());
        assert_eq!(updates.len(), 5, "{updates:?}");
        let mut with_runs = Vec::new();
        for update in &updates[1..4] {
            let Update::Runs { repo, runs } = update else {
                panic!("not a Runs update: {update:?}");
            };
            with_runs.push((repo.clone(), runs.len()));
        }
        with_runs.sort();
        assert_eq!(
            with_runs,
            vec![
                (repo("acme/a"), 1),
                (repo("acme/c"), 0),
                (repo("other/b"), 0)
            ]
        );
        assert!(matches!(updates[4], Update::CycleDone { .. }));
    }

    #[test]
    fn a_failing_repository_gives_failed_and_the_others_still_give_runs() {
        let fake = Arc::new(FakeSource::default());
        fake.script().created.insert(
            repo("acme/a"),
            Err(SourceError::Http {
                status: 502,
                message: "Bad Gateway".to_owned(),
            }),
        );
        fake.script()
            .created
            .insert(repo("acme/b"), Ok(vec![run("acme/b", 7)]));
        let worker = spawn_with(
            Arc::clone(&fake),
            &settings(&["acme/a", "acme/b"]),
            SLOW_CADENCE,
        );

        let updates = one_cycle(&worker);

        let error = SourceError::Http {
            status: 502,
            message: "Bad Gateway".to_owned(),
        }
        .to_string();
        let failed: Vec<_> = updates
            .iter()
            .filter(|update| matches!(update, Update::Failed { .. }))
            .collect();
        assert_eq!(
            failed,
            vec![&Update::Failed {
                repo: repo("acme/a"),
                error
            }]
        );
        assert!(updates.iter().any(
            |update| matches!(update, Update::Runs { repo: r, runs } if *r == repo("acme/b") && runs.len() == 1)
        ));
        assert!(!updates
            .iter()
            .any(|update| matches!(update, Update::Runs { repo: r, .. } if *r == repo("acme/a"))));
    }

    #[test]
    fn the_created_since_time_is_the_window_plus_one_day_back() {
        let fake = Arc::new(FakeSource::default());
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);

        let _ = one_cycle(&worker);

        let expected = Utc::now() - chrono::TimeDelta::hours(8 + 24);
        let since = fake.script().since.clone();
        assert_eq!(since.len(), 1, "{since:?}");
        let off = (since[0] - expected).abs();
        assert!(off < chrono::TimeDelta::seconds(60), "off by {off}");
    }

    #[test]
    fn cycle_done_carries_the_rate_limit_and_the_backoff() {
        let fake = Arc::new(FakeSource::default());
        let reset = Utc::now() + chrono::TimeDelta::hours(1);
        let low = RateLimit {
            limit: 5000,
            remaining: 10,
            reset,
        };
        fake.script().rate = Some(low);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);

        let updates = one_cycle(&worker);

        let Some(Update::CycleDone {
            rate,
            backoff_until,
            ..
        }) = updates.last()
        else {
            panic!("the last update is not CycleDone: {updates:?}");
        };
        assert_eq!(*rate, Some(low));
        assert_eq!(*backoff_until, Some(reset));
    }

    #[test]
    fn the_worker_fetches_several_repositories_at_once_but_not_too_many() {
        let fake = Arc::new(FakeSource {
            delay: Duration::from_millis(100),
            ..FakeSource::default()
        });
        let names: Vec<String> = (0..20).map(|n| format!("acme/r{n}")).collect();
        let targets: Vec<&str> = names.iter().map(String::as_str).collect();
        let worker = spawn_with(Arc::clone(&fake), &settings(&targets), SLOW_CADENCE);

        let updates = one_cycle(&worker);

        assert_eq!(updates.len(), 22, "Repos, 20 Runs, CycleDone");
        let most = fake.script().max_in_flight;
        assert!(most >= 2, "only {most} call in flight");
        assert!(most <= CONCURRENCY, "{most} calls in flight at once");
    }

    #[test]
    fn a_refresh_during_a_cycle_gives_exactly_one_more_cycle() {
        let fake = Arc::new(FakeSource {
            delay: Duration::from_millis(300),
            ..FakeSource::default()
        });
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        worker
            .commands
            .send(Command::RefreshNow)
            .expect("the worker listens");

        let first = one_cycle(&worker);
        let second = one_cycle(&worker);

        assert!(matches!(first.last(), Some(Update::CycleDone { .. })));
        assert!(matches!(second.last(), Some(Update::CycleDone { .. })));
        let third = worker.updates.recv_timeout(Duration::from_secs(1));
        assert_eq!(third, Err(RecvTimeoutError::Timeout), "a third cycle ran");
    }

    #[test]
    fn dropping_the_command_sender_ends_the_thread() {
        let fake = Arc::new(FakeSource::default());
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let _ = one_cycle(&worker);
        let Worker { updates, commands } = worker;

        drop(commands);

        let mut extra_cycles = 0;
        loop {
            match updates.recv_timeout(WAIT) {
                Ok(Update::CycleDone { .. }) => {
                    extra_cycles += 1;
                    assert!(
                        extra_cycles <= 1,
                        "the worker keeps cycling without a sender"
                    );
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("the worker thread did not end"),
            }
        }
        assert_eq!(extra_cycles, 0, "a cycle ran after the sender dropped");
    }

    #[test]
    fn an_owner_that_fails_to_list_keeps_its_last_good_list() {
        let fake = Arc::new(FakeSource::default());
        fake.script()
            .owners
            .insert(owner("acme"), Ok(vec![repo("acme/a"), repo("acme/b")]));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme"]), Duration::ZERO);
        let first = one_cycle(&worker);

        let failure = SourceError::Failed("boom".to_owned());
        fake.script()
            .owners
            .insert(owner("acme"), Err(failure.clone()));
        worker
            .commands
            .send(Command::RefreshNow)
            .expect("the worker listens");
        let second = one_cycle(&worker);

        let expected: BTreeSet<RepoName> = [repo("acme/a"), repo("acme/b")].into();
        assert_eq!(
            first[0],
            Update::Repos {
                repos: expected.clone(),
                owner_errors: Vec::new()
            }
        );
        assert_eq!(
            second[0],
            Update::Repos {
                repos: expected,
                owner_errors: vec![(owner("acme"), failure.to_string())]
            }
        );
    }

    #[test]
    fn the_active_sweep_runs_on_the_slow_cadence_and_its_runs_are_kept() {
        let fake = Arc::new(FakeSource::default());
        fake.script()
            .created
            .insert(repo("acme/a"), Ok(vec![run("acme/a", 1), run("acme/a", 3)]));
        fake.script().active.insert(
            repo("acme/a"),
            Ok(vec![active_run("acme/a", 1), active_run("acme/a", 2)]),
        );
        let worker = spawn_with(
            Arc::clone(&fake),
            &settings(&["acme/a"]),
            Duration::from_hours(1),
        );
        let first = one_cycle(&worker);
        worker
            .commands
            .send(Command::RefreshNow)
            .expect("the worker listens");
        let second = one_cycle(&worker);

        let sweeps = fake
            .script()
            .calls
            .iter()
            .filter(|(what, _)| *what == "active")
            .count();
        assert_eq!(sweeps, 1, "the second cycle must not sweep again");
        for updates in [&first, &second] {
            let Some(Update::Runs { runs, .. }) = updates
                .iter()
                .find(|update| matches!(update, Update::Runs { .. }))
            else {
                panic!("no Runs update: {updates:?}");
            };
            let mut found: Vec<(u64, RunState)> =
                runs.iter().map(|run| (run.id.0, run.state)).collect();
            found.sort_by_key(|(id, _)| *id);
            assert_eq!(
                found,
                vec![
                    (1, RunState::Passed),
                    (2, RunState::Running),
                    (3, RunState::Passed)
                ]
            );
        }
    }

    #[test]
    fn a_failing_sweep_gives_one_failed_and_no_runs() {
        let fake = Arc::new(FakeSource::default());
        let failure = SourceError::Failed("boom".to_owned());
        fake.script()
            .active
            .insert(repo("acme/a"), Err(failure.clone()));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);

        let updates = one_cycle(&worker);

        let for_repo: Vec<&Update> = updates
            .iter()
            .filter(|update| matches!(update, Update::Runs { .. } | Update::Failed { .. }))
            .collect();
        assert_eq!(
            for_repo,
            vec![&Update::Failed {
                repo: repo("acme/a"),
                error: failure.to_string()
            }]
        );
    }

    #[test]
    fn a_cut_owner_list_shows_an_owner_error_and_keeps_its_repositories() {
        let fake = Arc::new(FakeSource::default());
        fake.script()
            .owners
            .insert(owner("acme"), Ok(vec![repo("acme/a")]));
        fake.script().cut.insert(owner("acme"));
        fake.script()
            .created
            .insert(repo("acme/a"), Ok(vec![run("acme/a", 1)]));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme"]), SLOW_CADENCE);
        let first = one_cycle(&worker);

        assert_eq!(
            first[0],
            Update::Repos {
                repos: [repo("acme/a")].into(),
                owner_errors: vec![(
                    owner("acme"),
                    "more than 1000 repositories, ha watches the first 1000".to_owned()
                )]
            }
        );
        assert!(first.iter().any(|update| matches!(
            update,
            Update::Runs { repo: r, .. } if *r == repo("acme/a")
        )));
    }

    #[test]
    fn an_owner_whose_first_list_fails_is_listed_again_on_the_next_cycle() {
        let fake = Arc::new(FakeSource::default());
        let failure = SourceError::Failed("boom".to_owned());
        fake.script()
            .owners
            .insert(owner("acme"), Err(failure.clone()));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme"]), SLOW_CADENCE);
        let first = one_cycle(&worker);

        fake.script()
            .owners
            .insert(owner("acme"), Ok(vec![repo("acme/a")]));
        fake.script()
            .created
            .insert(repo("acme/a"), Ok(vec![run("acme/a", 1)]));
        worker
            .commands
            .send(Command::RefreshNow)
            .expect("the worker listens");
        let second = one_cycle(&worker);
        worker
            .commands
            .send(Command::RefreshNow)
            .expect("the worker listens");
        let third = one_cycle(&worker);

        assert_eq!(
            first[0],
            Update::Repos {
                repos: BTreeSet::new(),
                owner_errors: vec![(owner("acme"), failure.to_string())]
            }
        );
        assert_eq!(
            second[0],
            Update::Repos {
                repos: [repo("acme/a")].into(),
                owner_errors: Vec::new()
            }
        );
        assert!(
            second.iter().any(|update| matches!(
                update,
                Update::Runs { repo: r, runs } if *r == repo("acme/a") && runs.len() == 1
            )),
            "no Runs update for acme/a: {second:?}"
        );
        assert!(matches!(third.last(), Some(Update::CycleDone { .. })));
        let owner_calls = fake
            .script()
            .calls
            .iter()
            .filter(|(what, _)| *what == "owner")
            .count();
        assert_eq!(
            owner_calls, 2,
            "an owner with a good list waits for the cadence"
        );
    }

    #[test]
    fn a_retry_keeps_the_error_of_an_owner_that_kept_its_old_list() {
        let fake = Arc::new(FakeSource::default());
        let failure = SourceError::Failed("boom".to_owned());
        fake.script()
            .owners
            .insert(owner("acme-a"), Err(failure.clone()));
        let mut cadence = Cadence::new(SLOW_CADENCE);
        cadence.mark(Instant::now());
        let mut state = State {
            owner_cadence: cadence,
            owner_lists: BTreeMap::from([(owner("acme-b"), vec![repo("acme-b/x")])]),
            owner_errors: BTreeMap::from([(owner("acme-b"), "old error".to_owned())]),
            ..State::new(SLOW_CADENCE)
        };
        let (updates_tx, updates_rx) = mpsc::channel();

        let repos = refresh_repos(
            &Arc::clone(&fake),
            &settings(&["acme-a", "acme-b"]),
            &mut state,
            &updates_tx,
        )
        .unwrap_or_else(|_| panic!("the receiver is alive"));

        assert_eq!(repos, BTreeSet::from([repo("acme-b/x")]));
        assert_eq!(
            updates_rx.try_recv(),
            Ok(Update::Repos {
                repos: BTreeSet::from([repo("acme-b/x")]),
                owner_errors: vec![
                    (owner("acme-a"), failure.to_string()),
                    (owner("acme-b"), "old error".to_owned()),
                ]
            })
        );
        assert_eq!(updates_rx.try_recv(), Err(TryRecvError::Empty));
        let owner_calls: Vec<String> = fake
            .script()
            .calls
            .iter()
            .filter(|(what, _)| *what == "owner")
            .map(|(_, name)| name.clone())
            .collect();
        assert_eq!(owner_calls, vec!["acme-a".to_owned()]);
    }

    fn job(id: u64) -> Job {
        Job {
            id: crate::job::JobId(id),
            name: format!("job {id}"),
            state: RunState::Passed,
            started_at: None,
            completed_at: None,
            url: String::new(),
        }
    }

    /// Sends the whole set of watched runs, all in `acme/a`.
    fn watch(worker: &Worker, ids: &[u64]) {
        let set = ids.iter().map(|id| (repo("acme/a"), RunId(*id))).collect();
        worker
            .commands
            .send(Command::WatchJobs(set))
            .expect("the worker listens");
    }

    fn refresh(worker: &Worker) {
        worker
            .commands
            .send(Command::RefreshNow)
            .expect("the worker listens");
    }

    fn next_update(worker: &Worker) -> Update {
        worker
            .updates
            .recv_timeout(WAIT)
            .unwrap_or_else(|error| panic!("no update: {error:?}"))
    }

    fn job_calls(fake: &FakeSource) -> usize {
        fake.script()
            .calls
            .iter()
            .filter(|(what, _)| *what == "jobs")
            .count()
    }

    fn has_jobs(updates: &[Update]) -> bool {
        updates
            .iter()
            .any(|update| matches!(update, Update::Jobs { .. } | Update::JobsFailed { .. }))
    }

    /// A fake with one repository `acme/a` that holds the given runs.
    fn fake_with_runs(runs: Vec<Run>) -> Arc<FakeSource> {
        let fake = Arc::new(FakeSource::default());
        fake.script().created.insert(repo("acme/a"), Ok(runs));
        fake
    }

    #[test]
    fn a_new_watched_run_gets_its_jobs_at_once_without_a_cycle() {
        let fake = fake_with_runs(vec![run("acme/a", 5)]);
        fake.script()
            .jobs
            .insert(RunId(5), Ok(vec![job(1), job(2)]));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let _ = one_cycle(&worker);

        watch(&worker, &[5]);

        assert_eq!(
            next_update(&worker),
            Update::Jobs {
                run: RunId(5),
                jobs: vec![job(1), job(2)]
            }
        );
        assert_eq!(job_calls(&fake), 1);
    }

    #[test]
    fn a_failed_job_fetch_gives_jobs_failed_with_the_error_text() {
        let fake = fake_with_runs(vec![run("acme/a", 5)]);
        let failure = SourceError::Failed("boom".to_owned());
        fake.script().jobs.insert(RunId(5), Err(failure.clone()));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let _ = one_cycle(&worker);

        watch(&worker, &[5]);

        assert_eq!(
            next_update(&worker),
            Update::JobsFailed {
                run: RunId(5),
                error: failure.to_string()
            }
        );
    }

    #[test]
    fn the_jobs_of_a_finished_run_are_fetched_one_time_only() {
        let fake = fake_with_runs(vec![run("acme/a", 5)]);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let _ = one_cycle(&worker);
        watch(&worker, &[5]);
        assert!(matches!(next_update(&worker), Update::Jobs { .. }));

        refresh(&worker);
        let second = one_cycle(&worker);

        assert!(!has_jobs(&second), "{second:?}");
        assert_eq!(job_calls(&fake), 1);
    }

    #[test]
    fn the_jobs_of_an_active_run_are_fetched_on_each_cycle() {
        let fake = fake_with_runs(vec![active_run("acme/a", 5)]);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let _ = one_cycle(&worker);
        watch(&worker, &[5]);
        assert!(matches!(next_update(&worker), Update::Jobs { .. }));

        for _ in 0..2 {
            refresh(&worker);
            let cycle = one_cycle(&worker);
            let at = cycle
                .iter()
                .position(|update| matches!(update, Update::Jobs { run: RunId(5), .. }))
                .unwrap_or_else(|| panic!("no Jobs in the cycle: {cycle:?}"));
            assert_eq!(at, cycle.len() - 2, "Jobs comes right before CycleDone");
        }
        assert_eq!(job_calls(&fake), 3);
    }

    #[test]
    fn a_finished_run_that_changed_gets_its_jobs_again() {
        let fake = fake_with_runs(vec![run("acme/a", 5)]);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let _ = one_cycle(&worker);
        watch(&worker, &[5]);
        assert!(matches!(next_update(&worker), Update::Jobs { .. }));
        let mut rerun = run("acme/a", 5);
        rerun.updated_at += chrono::TimeDelta::minutes(1);
        fake.script()
            .created
            .insert(repo("acme/a"), Ok(vec![rerun]));

        refresh(&worker);
        let changed = one_cycle(&worker);
        refresh(&worker);
        let same = one_cycle(&worker);

        assert!(has_jobs(&changed), "{changed:?}");
        assert!(!has_jobs(&same), "{same:?}");
        assert_eq!(job_calls(&fake), 2);
    }

    #[test]
    fn a_run_whose_job_fetch_failed_is_fetched_again_on_the_next_cycle() {
        let fake = fake_with_runs(vec![run("acme/a", 5)]);
        fake.script()
            .jobs
            .insert(RunId(5), Err(SourceError::Failed("boom".to_owned())));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let _ = one_cycle(&worker);
        watch(&worker, &[5]);
        assert!(matches!(next_update(&worker), Update::JobsFailed { .. }));
        fake.script().jobs.insert(RunId(5), Ok(vec![job(9)]));

        refresh(&worker);
        let cycle = one_cycle(&worker);

        assert!(
            cycle.contains(&Update::Jobs {
                run: RunId(5),
                jobs: vec![job(9)]
            }),
            "{cycle:?}"
        );
    }

    #[test]
    fn a_collapsed_run_is_not_fetched_and_an_expanded_again_run_is_fetched_at_once() {
        let fake = fake_with_runs(vec![active_run("acme/a", 5)]);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let _ = one_cycle(&worker);
        watch(&worker, &[5]);
        assert!(matches!(next_update(&worker), Update::Jobs { .. }));

        watch(&worker, &[]);
        refresh(&worker);
        let collapsed = one_cycle(&worker);
        watch(&worker, &[5]);
        let again = next_update(&worker);

        assert!(!has_jobs(&collapsed), "{collapsed:?}");
        assert!(
            matches!(again, Update::Jobs { run: RunId(5), .. }),
            "{again:?}"
        );
        assert_eq!(job_calls(&fake), 2);
    }

    /// The three commands arrive during the first cycle, thus the backlog adds
    /// run 5 two times. The run must still get one request and one answer. The
    /// first answer proves that the fetch started. The test then drops the
    /// command sender. The worker ends its fetch, sees the drop, and stops, so
    /// the test counts everything that the worker sent. Each wait has a bound.
    #[test]
    fn a_run_that_the_backlog_adds_two_times_is_fetched_one_time() {
        let fake = Arc::new(FakeSource {
            delay: Duration::from_millis(300),
            ..FakeSource::default()
        });
        fake.script()
            .created
            .insert(repo("acme/a"), Ok(vec![run("acme/a", 5)]));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        watch(&worker, &[5]);
        watch(&worker, &[]);
        watch(&worker, &[5]);
        let mut answers = 0;
        loop {
            match worker.updates.recv_timeout(WAIT) {
                Ok(Update::Jobs { run: RunId(5), .. }) => {
                    answers += 1;
                    break;
                }
                Ok(_) => {}
                Err(error) => panic!("no Jobs update: {error:?}"),
            }
        }
        let Worker { updates, commands } = worker;
        drop(commands);
        loop {
            match updates.recv_timeout(WAIT) {
                Ok(Update::Jobs { run: RunId(5), .. }) => answers += 1,
                Ok(_) => {}
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("the worker did not stop in {WAIT:?}"),
            }
        }

        assert_eq!(job_calls(&fake), 1, "requests for the jobs of run 5");
        assert_eq!(answers, 1, "Jobs updates for run 5");
    }

    #[test]
    fn a_finished_run_is_fetched_again_after_a_collapse_and_an_expand() {
        let fake = fake_with_runs(vec![run("acme/a", 5)]);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let _ = one_cycle(&worker);
        watch(&worker, &[5]);
        assert!(matches!(next_update(&worker), Update::Jobs { .. }));

        watch(&worker, &[]);
        watch(&worker, &[5]);

        assert!(matches!(
            next_update(&worker),
            Update::Jobs { run: RunId(5), .. }
        ));
        assert_eq!(job_calls(&fake), 2);
    }

    #[test]
    fn job_fetches_share_the_concurrency_limit_of_the_run_fetches() {
        let runs: Vec<Run> = (1..=20).map(|id| active_run("acme/a", id)).collect();
        let fake = Arc::new(FakeSource {
            delay: Duration::from_millis(50),
            ..FakeSource::default()
        });
        fake.script().created.insert(repo("acme/a"), Ok(runs));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let _ = one_cycle(&worker);
        let ids: Vec<u64> = (1..=20).collect();

        fake.script().max_in_flight = 0;
        watch(&worker, &ids);
        for _ in 0..20 {
            assert!(matches!(next_update(&worker), Update::Jobs { .. }));
        }
        let at_once = fake.script().max_in_flight;
        fake.script().max_in_flight = 0;
        refresh(&worker);
        let cycle = one_cycle(&worker);
        let in_cycle = fake.script().max_in_flight;

        assert_eq!(cycle.len(), 22, "Runs, 20 Jobs, CycleDone: {cycle:?}");
        for most in [at_once, in_cycle] {
            assert!(most >= 2, "only {most} call in flight");
            assert!(most <= CONCURRENCY, "{most} calls in flight at once");
        }
    }

    /// A run that is watched during a rate limit backoff gets no request until
    /// the next cycle. A slow machine can only make this test pass, never fail
    /// it wrongly, because the test waits for something that must not happen.
    #[test]
    fn during_a_backoff_a_new_watched_run_waits_for_the_next_cycle() {
        let fake = fake_with_runs(vec![run("acme/a", 5)]);
        let reset = Utc::now() + chrono::TimeDelta::hours(1);
        fake.script().rate = Some(rate(10, reset));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let first = one_cycle(&worker);
        assert!(matches!(
            first.last(),
            Some(Update::CycleDone {
                backoff_until: Some(_),
                ..
            })
        ));

        watch(&worker, &[5]);
        let quiet = worker.updates.recv_timeout(Duration::from_millis(300));
        let calls_during_backoff = job_calls(&fake);
        refresh(&worker);
        let cycle = one_cycle(&worker);

        assert_eq!(quiet, Err(RecvTimeoutError::Timeout));
        assert_eq!(calls_during_backoff, 0);
        assert!(
            cycle
                .iter()
                .any(|update| matches!(update, Update::Jobs { run: RunId(5), .. })),
            "{cycle:?}"
        );
    }
}
