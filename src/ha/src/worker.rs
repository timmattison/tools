//! The background fetch loop.
//!
//! The worker runs on its own thread. It asks the [`Source`] for the runs of
//! every repository, sends each answer to the screen as an [`Update`], and
//! then waits for the next cycle. The screen sends [`Command`] values back.
//!
//! The screen also tells the worker which runs it shows expanded, with
//! [`Command::WatchJobs`]. The worker fetches the jobs of those watched runs,
//! and it fetches the jobs of every active run (queued or running), also when
//! the screen shows the run collapsed. GitHub reports a run as queued while one
//! job waits for a runner, also when other jobs run, so the screen needs the
//! job states of a collapsed run to draw the right marker. The worker fetches
//! the jobs of a new watched run at once, and it fetches the jobs of the other
//! runs on each cycle when the run is due. A run is due only while the list of
//! runs holds it. Then it is due when it has no good job fetch yet, when the
//! run or one of its jobs was active at the last good fetch, or when its
//! `updated_at` changed since the last good fetch. So `ha` fetches
//! the jobs of a finished run one time only, when all of its jobs are finished.
//! Each active run costs one more jobs request on each cycle. An `ETag` 304
//! answer makes that request free. Job requests and run requests share the
//! [`CONCURRENCY`] limit and the rate limit backoff.
//!
//! GitHub can answer with success and still leave runs out. The worker keeps
//! the last good created answer of each repository. A run that the new answer
//! leaves out, and that still exists, makes the answer suspect. The worker
//! then keeps the last good runs and sends [`Update::Suspect`].

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::github::{RepoList, RunList, RunQuery, Source, SourceError};
use crate::job::Job;
use crate::run::{Run, RunId};
use crate::settings::Settings;
use crate::status::{OwnerProblem, RateLimit};
use crate::suspect::{first_left_out, Presence, Suspicion};
use crate::target::{OwnerName, RepoName, Target};

/// How many repositories the worker fetches at the same time.
pub const CONCURRENCY: usize = 8;

/// How often the worker lists the repositories of the owners and sweeps for
/// active runs. The sweep finds runs that started before the window. An owner
/// with no good list, an owner whose good list is empty, and an owner whose
/// last list failed or was suspect, is listed on every cycle until a good
/// list that holds a repository replaces the problem. An empty list needs
/// this, because the comparison with the last good list cannot judge a first
/// empty list.
pub const SLOW_CADENCE: Duration = Duration::from_secs(600);

/// One message from the worker to the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// The repositories to watch now, and the owners whose list failed, was cut, or was suspect.
    Repos {
        /// Every repository to watch, without duplicates.
        repos: BTreeSet<RepoName>,
        /// The owners whose list failed, was cut, or was suspect, with the problem of each.
        owner_errors: Vec<(OwnerName, OwnerProblem)>,
        /// When the last good list of each owner that has one was made.
        listed_at: BTreeMap<OwnerName, DateTime<Utc>>,
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
        /// The error of the fetch.
        error: SourceError,
    },
    /// The answer for one repository was suspect. The board keeps the last
    /// good runs of the repository.
    Suspect {
        /// The repository whose answer was suspect.
        repo: RepoName,
        /// Why the worker does not trust the answer.
        reason: Suspicion,
    },
    /// The jobs of one run. The worker sends them for a run that the screen
    /// shows expanded and for every active run.
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

/// The last good list of one owner.
struct OwnerList {
    repos: Vec<RepoName>,
    /// When the worker made the list.
    listed_at: DateTime<Utc>,
}

/// What the worker keeps from one cycle to the next.
struct State {
    owner_cadence: Cadence,
    /// The last good list of each owner, with the time of that list.
    owner_lists: BTreeMap<OwnerName, OwnerList>,
    /// The problem of each owner whose last list failed or was cut.
    owner_errors: BTreeMap<OwnerName, OwnerProblem>,
    sweep_cadence: Cadence,
    /// The active runs of the last good sweep, for each repository.
    sweeps: Mutex<HashMap<RepoName, Vec<Run>>>,
    /// The last good created answer of each repository. No thread holds this
    /// lock during a call of the source.
    created_answers: Mutex<HashMap<RepoName, Vec<Run>>>,
    /// The runs whose jobs the screen shows now (the last `WatchJobs`).
    watched: BTreeSet<RunKey>,
    /// The facts of each run of the last good answer, for each repository.
    facts: Mutex<HashMap<RepoName, HashMap<RunId, RunFacts>>>,
    /// For each run with a job fetch, the facts of the run at that fetch. The
    /// mark is active when the run or one of its jobs was active, or when the
    /// fetch failed. A run keeps its mark when it leaves the watched set, so it
    /// gets one last fetch.
    job_marks: Mutex<HashMap<RunKey, RunFacts>>,
    /// The runs whose job pages the source can still hold. It is each run that
    /// the worker fetched since it last told the source to forget that run.
    held_jobs: Mutex<BTreeSet<RunKey>>,
}

impl State {
    fn new(slow: Duration) -> State {
        State {
            owner_cadence: Cadence::new(slow),
            owner_lists: BTreeMap::new(),
            owner_errors: BTreeMap::new(),
            sweep_cadence: Cadence::new(slow),
            sweeps: Mutex::new(HashMap::new()),
            created_answers: Mutex::new(HashMap::new()),
            watched: BTreeSet::new(),
            facts: Mutex::new(HashMap::new()),
            job_marks: Mutex::new(HashMap::new()),
            held_jobs: Mutex::new(BTreeSet::new()),
        }
    }

    /// Takes a new set of watched runs. Returns the runs that are new in it.
    /// A run that joins the set loses its mark, so the next cycle fetches it
    /// even when the fetch at once does not happen. A run that leaves the set
    /// keeps its mark, so it still gets its last fetch when it finishes.
    fn watch(&mut self, set: BTreeSet<RunKey>) -> Vec<RunKey> {
        let added: Vec<RunKey> = set.difference(&self.watched).cloned().collect();
        let marks = self
            .job_marks
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        for key in &added {
            marks.remove(key);
        }
        self.watched = set;
        added
    }

    /// The runs whose jobs the next cycle fetches, each one time, in a stable
    /// order. The function considers three sets: the watched runs, every run
    /// whose facts say it is active, and every run that has a mark. It applies
    /// the due rule to each of them. The third set gives a run that is not
    /// watched its last fetch: its mark is active, so it is due one more time.
    /// A run whose fetch failed is due again through its active mark.
    ///
    /// A run with no facts is never due. It left the list of runs, or its
    /// repository left the repository set. The board ignores a job answer for
    /// a run that it does not hold, so a fetch of such a run is wasted.
    fn due_jobs(&mut self) -> Vec<RunKey> {
        let facts = self.facts.get_mut().unwrap_or_else(PoisonError::into_inner);
        let marks = self
            .job_marks
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        let mut considered: BTreeSet<RunKey> = self.watched.clone();
        considered.extend(marks.keys().cloned());
        for (repo, runs) in facts.iter() {
            considered.extend(
                runs.iter()
                    .filter(|(_, run_facts)| run_facts.active)
                    .map(|(run, _)| (repo.clone(), *run)),
            );
        }
        considered
            .into_iter()
            .filter(|(repo, run)| {
                let Some(now) = facts.get(repo).and_then(|runs| runs.get(run)) else {
                    return false;
                };
                match marks.get(&(repo.clone(), *run)) {
                    None => true,
                    Some(mark) => mark.active || now.updated_at != mark.updated_at,
                }
            })
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
/// A run gets one fetch, even when the backlog adds it more than one time.
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
    let mut added: BTreeSet<RunKey> = BTreeSet::new();
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
            let fetch_now: Vec<RunKey> = added.iter().cloned().collect();
            fetch_jobs(source, state, &fetch_now, updates)?;
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
/// new runs to `added`. The set holds a run one time, even when two commands
/// add it.
fn apply_command(
    command: Command,
    state: &mut State,
    requested: &mut bool,
    added: &mut BTreeSet<RunKey>,
) {
    match command {
        Command::RefreshNow => *requested = true,
        Command::WatchJobs(set) => added.extend(state.watch(set)),
    }
}

/// Runs one cycle: lists the owners that need it, fetches every repository,
/// fetches the jobs of the runs that are due (the watched runs, the active
/// runs, and the runs with a mark, when the list of runs holds the run), and
/// reports the end of the cycle. A mark whose run left the list of runs is
/// removed. The reconcile at the end of the job fetch then tells the source to
/// forget the pages of each run whose mark went away. A failed or suspect
/// repository fetch keeps its old facts, thus its marks stay. Returns how long to wait
/// before the next one, and whether the worker waits for a rate limit reset.
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
    state
        .created_answers
        .get_mut()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|repo, _| repos.contains(repo));
    let fetch = Fetch {
        since: window_start(settings.window),
        sweep_due,
        sweeps: &state.sweeps,
        created_answers: &state.created_answers,
        facts: &state.facts,
    };
    fetch_all(source, &repos, &fetch, updates)?;
    let facts = state
        .facts
        .get_mut()
        .unwrap_or_else(PoisonError::into_inner);
    facts.retain(|repo, _| repos.contains(repo));
    state
        .job_marks
        .get_mut()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|(repo, run), _| facts.get(repo).is_some_and(|runs| runs.contains_key(run)));
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
    /// The last good created answer of each repository. A good answer replaces
    /// the entry of its repository. No thread holds this lock during a call.
    created_answers: &'a Mutex<HashMap<RepoName, Vec<Run>>>,
    /// The facts of the runs of each repository. A good fetch replaces the
    /// entry of its repository. No thread holds this lock during a call.
    facts: &'a Mutex<HashMap<RepoName, HashMap<RunId, RunFacts>>>,
}

/// What the fetch of one repository gives.
enum Fetched {
    /// The runs to show: the recent answer and the runs of the last sweep.
    Good(Vec<Run>),
    /// GitHub answered with success, but `ha` does not trust the answer.
    Suspect(Suspicion),
    /// A request failed.
    Failed(SourceError),
}

/// Fetches the runs of one repository and merges in the last sweep.
///
/// A good result holds every run of the recent answer. It also holds each run
/// of the last sweep that the recent answer lacks. Those runs started before
/// the window and still run. The recent answer goes through
/// [`suspicion_of_created`]. A suspect answer keeps the last good answer, and
/// the function tells the source to forget the cached pages of the query, so
/// the next cycle asks GitHub again. The sweep still runs and its result is
/// stored, also when the recent answer is suspect. A failed request gives
/// [`Fetched::Failed`].
fn fetch_repo<S: Source>(source: &S, repo: &RepoName, fetch: &Fetch<'_>) -> Fetched {
    let query = RunQuery::CreatedSince(fetch.since);
    let list = match source.runs_of(repo, query) {
        Ok(list) => list,
        Err(error) => return Fetched::Failed(error),
    };
    if fetch.sweep_due {
        match source.runs_of(repo, RunQuery::Active) {
            Ok(active) => {
                fetch
                    .sweeps
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(repo.clone(), active.runs);
            }
            Err(error) => return Fetched::Failed(error),
        }
    }
    if let Some(suspicion) = suspicion_of_created(source, repo, fetch, &list) {
        source.forget_runs(repo, query);
        return Fetched::Suspect(suspicion);
    }
    let mut runs = list.runs;
    fetch
        .created_answers
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(repo.clone(), runs.clone());
    let known: HashSet<RunId> = runs.iter().map(|run| run.id).collect();
    if let Some(swept) = fetch
        .sweeps
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(repo)
    {
        runs.extend(swept.iter().filter(|run| !known.contains(&run.id)).cloned());
    }
    Fetched::Good(runs)
}

/// Decides whether `ha` trusts a successful created answer of a repository.
///
/// Without a last good answer there is nothing to compare. Otherwise the
/// answer goes through [`first_left_out`]. A run of the last good answer is in
/// range when it was created since the start of the query. When GitHub cut the
/// answer, the run must also be as new as the oldest run of the answer, because
/// a run past the cut is not missing. A run that is in range and missing is
/// read directly. A run that still exists makes the answer suspect. A failed
/// read does the same, and a run that is gone does not.
fn suspicion_of_created<S: Source>(
    source: &S,
    repo: &RepoName,
    fetch: &Fetch<'_>,
    list: &RunList,
) -> Option<Suspicion> {
    let last_good = fetch
        .created_answers
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(repo)
        .cloned()?;
    let oldest = if list.cut {
        list.runs.iter().map(|run| run.created_at).min()
    } else {
        None
    };
    first_left_out(
        &last_good,
        &list.runs,
        |run| run.created_at >= fetch.since && oldest.is_none_or(|oldest| run.created_at >= oldest),
        |run| match source.run(repo, run.id) {
            Ok(Some(_)) => Presence::InRange,
            Ok(None) => Presence::Gone,
            Err(error) => Presence::Unread(error),
        },
    )
}

/// Fetches the runs of every repository with a pool of threads.
///
/// After a good fetch the function stores the facts of the runs of that
/// repository and sends [`Update::Runs`]. A failed fetch keeps the old facts and
/// sends [`Update::Failed`]. A suspect fetch keeps the old facts too and sends
/// [`Update::Suspect`].
fn fetch_all<S: Source>(
    source: &S,
    repos: &BTreeSet<RepoName>,
    fetch: &Fetch<'_>,
    updates: &Sender<Update>,
) -> Result<(), ReceiverGone> {
    let repos: Vec<&RepoName> = repos.iter().collect();
    run_pool(&repos, updates, |repo| {
        match fetch_repo(source, repo, fetch) {
            Fetched::Good(runs) => {
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
            Fetched::Suspect(reason) => Update::Suspect {
                repo: (*repo).clone(),
                reason,
            },
            Fetched::Failed(error) => Update::Failed {
                repo: (*repo).clone(),
                error,
            },
        }
    })
}

/// Fetches the jobs of the given runs with the shared pool of threads.
///
/// A good fetch sends [`Update::Jobs`] and marks the run with its current
/// facts. The mark is active when the run or one of its jobs is active. A run
/// with no facts gets no mark. A failed fetch sends [`Update::JobsFailed`]. A
/// run with facts then gets an active mark, so the next cycle tries again, also
/// when the run is not watched and not active. A run with no facts loses its
/// mark.
///
/// Each call of the source can leave pages in its cache, also a failed call.
/// Thus the function records each fetched run in `held_jobs`. After the
/// fetches, it tells the source to forget the pages of each held run that has
/// no active mark. An active mark makes the run due again on the next cycle.
/// A run with no mark is not due again until a change makes it due, so its
/// pages have no use. This one step covers every way that a mark goes away.
fn fetch_jobs<S: Source>(
    source: &S,
    state: &State,
    runs: &[RunKey],
    updates: &Sender<Update>,
) -> Result<(), ReceiverGone> {
    run_pool(runs, updates, |(repo, run)| {
        let result = source.jobs_of(repo, *run);
        state
            .held_jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((repo.clone(), *run));
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
                let active = facts.active || jobs.iter().any(|job| job.state.is_active());
                marks.insert((repo.clone(), *run), RunFacts { active, ..facts });
                Update::Jobs { run: *run, jobs }
            }
            (Ok(jobs), None) => {
                marks.remove(&(repo.clone(), *run));
                Update::Jobs { run: *run, jobs }
            }
            (Err(error), facts) => {
                let key = (repo.clone(), *run);
                match facts {
                    Some(facts) => marks.insert(
                        key,
                        RunFacts {
                            active: true,
                            ..facts
                        },
                    ),
                    None => marks.remove(&key),
                };
                Update::JobsFailed {
                    run: *run,
                    error: error.to_string(),
                }
            }
        }
    })?;
    forget_jobs_with_no_active_mark(source, state);
    Ok(())
}

/// Tells the source to forget the pages of each held run with no active mark.
///
/// Only an active mark makes the worker fetch a run again. A held run with no
/// mark, or with an inactive mark, is thus a run that the worker stops fetching.
/// The function takes the runs in order and removes them from `held_jobs`. It
/// drops both locks before the first call of the source.
fn forget_jobs_with_no_active_mark<S: Source>(source: &S, state: &State) {
    let stopped: Vec<RunKey> = {
        let marks = state
            .job_marks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut held = state
            .held_jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let stopped: Vec<RunKey> = held
            .iter()
            .filter(|key| !marks.get(*key).is_some_and(|mark| mark.active))
            .cloned()
            .collect();
        for key in &stopped {
            held.remove(key);
        }
        stopped
    };
    // No thread holds a lock during a call of the source.
    for (repo, run) in &stopped {
        source.forget_jobs(repo, *run);
    }
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

/// Decides whether `ha` trusts a successful list of an owner.
///
/// A list that GitHub cut gets no suspicion, because a missing repository can
/// be past the cut. Without a last good list there is nothing to compare. The
/// other lists go through [`first_left_out`]: a repository that the last good
/// list held and the new list leaves out is read directly. A repository that
/// is still listable makes the list suspect. A failed read does the same.
fn suspicion_of_list<S: Source>(
    source: &S,
    last_good: Option<&[RepoName]>,
    list: &RepoList,
) -> Option<Suspicion> {
    if list.cut {
        return None;
    }
    first_left_out(
        last_good?,
        &list.repos,
        |_| true,
        |repo| match source.repo_is_listable(repo) {
            Ok(true) => Presence::InRange,
            Ok(false) => Presence::Gone,
            Err(error) => Presence::Unread(error),
        },
    )
}

/// Lists one owner and records the result in `state`.
///
/// A good list replaces the last good list, with its time, and clears the
/// problem of the owner (a cut list leaves a [`OwnerProblem::Cut`]). A suspect
/// list keeps the last good list and its time. A failed list does the same.
fn list_owner<S: Source>(source: &S, owner: &OwnerName, state: &mut State) {
    let list = match source.repos_of(owner) {
        Ok(list) => list,
        Err(error) => {
            state
                .owner_errors
                .insert(owner.clone(), OwnerProblem::Failed(error));
            return;
        }
    };
    let last_good = state
        .owner_lists
        .get(owner)
        .map(|last| last.repos.as_slice());
    if let Some(suspicion) = suspicion_of_list(source, last_good, &list) {
        state
            .owner_errors
            .insert(owner.clone(), OwnerProblem::Suspect(suspicion));
        return;
    }
    match list.warning() {
        Some(text) => {
            state
                .owner_errors
                .insert(owner.clone(), OwnerProblem::Cut(text));
        }
        None => {
            state.owner_errors.remove(owner);
        }
    }
    state.owner_lists.insert(
        owner.clone(),
        OwnerList {
            repos: list.repos,
            listed_at: Utc::now(),
        },
    );
}

/// Builds the repository set and sends it when it can have changed.
///
/// When the owner cadence is due, the function lists every owner again. On
/// every cycle it also lists each owner that has no good list yet, each owner
/// whose good list is empty, and each owner whose last list failed or was
/// suspect, so the `r` key tries such an owner again at once. An owner whose
/// good list is empty can have got a wrong answer: `gh repo list` gave `[]`
/// with exit 0 about one time in 20, and the comparison cannot judge a first
/// empty list. An owner with a good list that holds a repository, or with a
/// cut list, waits for the cadence. A successful list goes through [`suspicion_of_list`]. A
/// suspect list keeps the last good list of the owner. The function keeps the
/// last list problem of each owner in `state`, and it clears that problem when
/// a good list replaces it. It sends [`Update::Repos`] when the cadence is due
/// or when it listed an owner.
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
        let needs_a_list = state
            .owner_lists
            .get(owner)
            .is_none_or(|list| list.repos.is_empty())
            || matches!(
                state.owner_errors.get(owner),
                Some(OwnerProblem::Failed(_) | OwnerProblem::Suspect(_))
            );
        if !due && !needs_a_list {
            continue;
        }
        listed = true;
        list_owner(source, owner, state);
    }
    let mut repos = BTreeSet::new();
    for target in &settings.targets {
        match target {
            Target::Repo(repo) => {
                repos.insert(repo.clone());
            }
            Target::Owner(owner) => {
                repos.extend(
                    state
                        .owner_lists
                        .get(owner)
                        .into_iter()
                        .flat_map(|list| list.repos.iter().cloned()),
                );
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
                listed_at: state
                    .owner_lists
                    .iter()
                    .map(|(owner, list)| (owner.clone(), list.listed_at))
                    .collect(),
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
    use crate::github::{RepoList, RunList, RunQuery, SourceError};
    use crate::job::Job;
    use crate::run::{RunId, RunState};
    use crate::suspect::{Item, Suspicion};
    use crate::target::Target;

    /// The longest a test waits for one update.
    const WAIT: Duration = Duration::from_secs(30);

    /// The answers and the call log of a [`FakeSource`].
    #[derive(Default)]
    struct Script {
        owners: HashMap<OwnerName, Result<Vec<RepoName>, SourceError>>,
        /// The owners whose list the fake gives as cut.
        cut: std::collections::HashSet<OwnerName>,
        /// The repositories whose `runs_of` answers the fake gives as cut.
        cut_runs: std::collections::HashSet<RepoName>,
        created: HashMap<RepoName, Result<Vec<Run>, SourceError>>,
        active: HashMap<RepoName, Result<Vec<Run>, SourceError>>,
        /// The jobs of each run. A run with no entry has no jobs.
        jobs: HashMap<RunId, Result<Vec<Job>, SourceError>>,
        /// The answer of `newest_run` for each repository. A repository with no
        /// entry has no run.
        newest: HashMap<RepoName, Result<Option<Run>, SourceError>>,
        /// The answer of `run` for each run. A run with no entry is gone.
        direct: HashMap<RunId, Result<Option<Run>, SourceError>>,
        /// The answer of `repo_is_listable` for each repository. A repository
        /// with no entry is not listable.
        listable: HashMap<RepoName, Result<bool, SourceError>>,
        calls: Vec<(&'static str, String)>,
        since: Vec<DateTime<Utc>>,
        rate: Option<RateLimit>,
        in_flight: usize,
        max_in_flight: usize,
        /// The runs whose job pages the fake holds. Each `jobs_of` call adds
        /// its run, whatever the result, because a call can leave pages whether
        /// it fails or not. Each `forget_jobs` call removes its run.
        held: BTreeSet<(RepoName, RunId)>,
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

        fn runs_of(&self, repo: &RepoName, query: RunQuery) -> Result<RunList, SourceError> {
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
            let cut = script.cut_runs.contains(repo);
            let answer = answers
                .get(repo)
                .cloned()
                .unwrap_or(Ok(Vec::new()))
                .map(|runs| RunList { runs, cut });
            script.in_flight += 1;
            script.max_in_flight = script.max_in_flight.max(script.in_flight);
            drop(script);
            std::thread::sleep(self.delay);
            self.script().in_flight -= 1;
            answer
        }

        fn jobs_of(&self, repo: &RepoName, run: RunId) -> Result<Vec<Job>, SourceError> {
            let mut script = self.script();
            script.calls.push(("jobs", run.0.to_string()));
            script.held.insert((repo.clone(), run));
            let answer = script.jobs.get(&run).cloned().unwrap_or(Ok(Vec::new()));
            script.in_flight += 1;
            script.max_in_flight = script.max_in_flight.max(script.in_flight);
            drop(script);
            std::thread::sleep(self.delay);
            self.script().in_flight -= 1;
            answer
        }

        fn forget_jobs(&self, repo: &RepoName, run: RunId) {
            let mut script = self.script();
            script.calls.push(("forget", run.0.to_string()));
            script.held.remove(&(repo.clone(), run));
        }

        fn repo_is_listable(&self, repo: &RepoName) -> Result<bool, SourceError> {
            let mut script = self.script();
            script.calls.push(("listable", repo.to_string()));
            script.listable.get(repo).cloned().unwrap_or(Ok(false))
        }

        fn run(&self, _repo: &RepoName, run: RunId) -> Result<Option<Run>, SourceError> {
            let mut script = self.script();
            script.calls.push(("run", run.0.to_string()));
            script.direct.get(&run).cloned().unwrap_or(Ok(None))
        }

        fn newest_run(&self, repo: &RepoName) -> Result<Option<Run>, SourceError> {
            let mut script = self.script();
            script.calls.push(("newest", repo.to_string()));
            script.newest.get(repo).cloned().unwrap_or(Ok(None))
        }

        fn forget_runs(&self, repo: &RepoName, query: RunQuery) {
            let what = match query {
                RunQuery::CreatedSince(_) => "created",
                RunQuery::Active => "active",
            };
            self.script()
                .calls
                .push(("forget-runs", format!("{repo} {what}")));
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
            repo: repo(repo_text),
            workflow: "ci".to_owned(),
            branch: "main".to_owned(),
            title: format!("run {id}"),
            state: RunState::Passed,
            started_at: at,
            created_at: at,
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
            ..
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
        };
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
            untimed(&first[0]),
            Update::Repos {
                repos: expected.clone(),
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: Vec::new()
            }
        );
        assert_eq!(
            untimed(&second[0]),
            Update::Repos {
                repos: expected,
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: vec![(owner("acme"), OwnerProblem::Failed(failure))]
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
                error: failure
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
            untimed(&first[0]),
            Update::Repos {
                repos: [repo("acme/a")].into(),
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: vec![(
                    owner("acme"),
                    OwnerProblem::Cut(
                        "more than 1000 repositories, ha watches the first 1000".to_owned()
                    )
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
            untimed(&first[0]),
            Update::Repos {
                repos: BTreeSet::new(),
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: vec![(owner("acme"), OwnerProblem::Failed(failure))]
            }
        );
        assert_eq!(
            untimed(&second[0]),
            Update::Repos {
                repos: [repo("acme/a")].into(),
                listed_at: std::collections::BTreeMap::new(),
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
        let listed_at = Utc::now();
        let mut state = State {
            owner_cadence: cadence,
            owner_lists: BTreeMap::from([(
                owner("acme-b"),
                OwnerList {
                    repos: vec![repo("acme-b/x")],
                    listed_at,
                },
            )]),
            owner_errors: BTreeMap::from([(
                owner("acme-b"),
                OwnerProblem::Cut("old error".to_owned()),
            )]),
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
                    (owner("acme-a"), OwnerProblem::Failed(failure)),
                    (owner("acme-b"), OwnerProblem::Cut("old error".to_owned())),
                ],
                listed_at: BTreeMap::from([(owner("acme-b"), listed_at)]),
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

    /// The update without its list times, which a test cannot state.
    fn untimed(update: &Update) -> Update {
        match update {
            Update::Repos {
                repos,
                owner_errors,
                ..
            } => Update::Repos {
                repos: repos.clone(),
                owner_errors: owner_errors.clone(),
                listed_at: BTreeMap::new(),
            },
            other => other.clone(),
        }
    }

    /// The owners that `fake` listed, in call order.
    fn listed_owners(fake: &FakeSource) -> Vec<String> {
        fake.script()
            .calls
            .iter()
            .filter(|(what, _)| *what == "owner")
            .map(|(_, name)| name.clone())
            .collect()
    }

    /// A state with a good list of `acme` and a cadence that is not due.
    fn state_with_a_good_list_of_acme() -> State {
        let mut cadence = Cadence::new(SLOW_CADENCE);
        cadence.mark(Instant::now());
        State {
            owner_cadence: cadence,
            owner_lists: BTreeMap::from([(
                owner("acme"),
                OwnerList {
                    repos: vec![repo("acme/a"), repo("acme/b")],
                    listed_at: Utc::now(),
                },
            )]),
            ..State::new(SLOW_CADENCE)
        }
    }

    #[test]
    fn an_owner_whose_last_list_was_suspect_is_listed_again_before_the_cadence() {
        let fake = Arc::new(FakeSource::default());
        script_owner(&fake, &["acme/a", "acme/b"]);
        let mut state = state_with_a_good_list_of_acme();
        state.owner_errors.insert(
            owner("acme"),
            OwnerProblem::Suspect(Suspicion::LeftOut(Item::Repo(repo("acme/a")))),
        );
        let (updates_tx, updates_rx) = mpsc::channel();

        let repos = refresh_repos(
            &Arc::clone(&fake),
            &settings(&["acme"]),
            &mut state,
            &updates_tx,
        )
        .unwrap_or_else(|_| panic!("the receiver is alive"));

        assert_eq!(listed_owners(&fake), vec!["acme".to_owned()]);
        assert_eq!(repos, BTreeSet::from([repo("acme/a"), repo("acme/b")]));
        let sent: Vec<Update> = updates_rx.try_iter().collect();
        let (_, owner_errors) = repos_of_cycle(&sent);
        assert!(owner_errors.is_empty(), "a good list clears the problem");
    }

    #[test]
    fn an_owner_with_a_good_list_whose_last_list_failed_is_listed_on_the_next_cycle() {
        let fake = Arc::new(FakeSource::default());
        let failure = SourceError::Failed("boom".to_owned());
        fake.script()
            .owners
            .insert(owner("acme"), Err(failure.clone()));
        let mut state = state_with_a_good_list_of_acme();
        state.owner_cadence = Cadence::new(SLOW_CADENCE);
        let (updates_tx, updates_rx) = mpsc::channel();
        let targets = settings(&["acme"]);
        refresh_repos(&Arc::clone(&fake), &targets, &mut state, &updates_tx)
            .unwrap_or_else(|_| panic!("the receiver is alive"));
        let first: Vec<Update> = updates_rx.try_iter().collect();
        assert_eq!(
            repos_of_cycle(&first).1,
            &vec![(owner("acme"), OwnerProblem::Failed(failure))]
        );

        script_owner(&fake, &["acme/a", "acme/b"]);
        refresh_repos(&Arc::clone(&fake), &targets, &mut state, &updates_tx)
            .unwrap_or_else(|_| panic!("the receiver is alive"));

        assert_eq!(
            listed_owners(&fake),
            vec!["acme".to_owned(), "acme".to_owned()]
        );
        let second: Vec<Update> = updates_rx.try_iter().collect();
        assert!(repos_of_cycle(&second).1.is_empty());
    }

    /// The time of the last good list of `acme` in the first `Repos` update.
    fn listed_at_of_acme(updates: &[Update]) -> DateTime<Utc> {
        let Some(Update::Repos { listed_at, .. }) = updates
            .iter()
            .find(|update| matches!(update, Update::Repos { .. }))
        else {
            panic!("no Repos update: {updates:?}");
        };
        *listed_at
            .get(&owner("acme"))
            .unwrap_or_else(|| panic!("no time for acme in {listed_at:?}"))
    }

    #[test]
    fn the_time_of_a_list_stays_after_a_suspect_list_and_moves_after_a_good_list() {
        let fake = Arc::new(FakeSource::default());
        script_owner(&fake, &["acme/a", "acme/b"]);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme"]), Duration::ZERO);
        let first = listed_at_of_acme(&one_cycle(&worker));
        std::thread::sleep(Duration::from_millis(5));

        fake.script().owners.insert(owner("acme"), Ok(Vec::new()));
        let suspect = listed_at_of_acme(&refresh_now(&worker));
        assert_eq!(suspect, first, "a suspect list keeps the old time");
        std::thread::sleep(Duration::from_millis(5));

        script_owner(&fake, &["acme/a", "acme/b"]);
        let good = listed_at_of_acme(&refresh_now(&worker));
        assert!(good > first, "a good list moves the time on");
    }

    #[test]
    fn an_owner_with_no_good_list_has_no_time() {
        let fake = Arc::new(FakeSource::default());
        fake.script()
            .owners
            .insert(owner("acme"), Err(SourceError::Failed("boom".to_owned())));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme"]), SLOW_CADENCE);

        let updates = one_cycle(&worker);

        let Some(Update::Repos { listed_at, .. }) = updates.first() else {
            panic!("no Repos update: {updates:?}");
        };
        assert!(listed_at.is_empty(), "{listed_at:?}");
    }

    /// Asks for a new cycle at once and reads its updates.
    fn refresh_now(worker: &Worker) -> Vec<Update> {
        worker
            .commands
            .send(Command::RefreshNow)
            .expect("the worker listens");
        one_cycle(worker)
    }

    /// The repository set and the owner problems of the first `Repos` update.
    fn repos_of_cycle(
        updates: &[Update],
    ) -> (&BTreeSet<RepoName>, &Vec<(OwnerName, OwnerProblem)>) {
        let Some(Update::Repos {
            repos,
            owner_errors,
            ..
        }) = updates
            .iter()
            .find(|update| matches!(update, Update::Repos { .. }))
        else {
            panic!("no Repos update: {updates:?}");
        };
        (repos, owner_errors)
    }

    /// Scripts the owner `acme` to list `names`, and each name as listable.
    fn script_owner(fake: &FakeSource, names: &[&str]) {
        let repos: Vec<RepoName> = names.iter().map(|name| repo(name)).collect();
        let mut script = fake.script();
        for repo in &repos {
            script.listable.insert(repo.clone(), Ok(true));
        }
        script.owners.insert(owner("acme"), Ok(repos));
    }

    #[test]
    fn an_empty_list_after_a_good_list_keeps_the_repositories_and_is_suspect() {
        let fake = Arc::new(FakeSource::default());
        let names = ["acme/a", "acme/b", "acme/c", "acme/d", "acme/e"];
        script_owner(&fake, &names);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme"]), Duration::ZERO);
        let first = one_cycle(&worker);
        assert_eq!(repos_of_cycle(&first).0.len(), 5);

        fake.script().owners.insert(owner("acme"), Ok(Vec::new()));
        fake.script()
            .created
            .insert(repo("acme/a"), Ok(vec![run("acme/a", 1)]));
        let second = refresh_now(&worker);

        let (repos, owner_errors) = repos_of_cycle(&second);
        let expected: BTreeSet<RepoName> = names.iter().map(|name| repo(name)).collect();
        assert_eq!(repos, &expected);
        assert_eq!(
            owner_errors,
            &vec![(
                owner("acme"),
                OwnerProblem::Suspect(Suspicion::LeftOut(Item::Repo(repo("acme/a"))))
            )]
        );
        assert!(
            second.iter().any(|update| matches!(
                update,
                Update::Runs { repo: r, runs } if *r == repo("acme/a") && runs.len() == 1
            )),
            "the runs of the kept repository are fetched: {second:?}"
        );
    }

    #[test]
    fn a_repository_that_a_later_list_drops_and_that_is_gone_leaves_the_set() {
        let fake = Arc::new(FakeSource::default());
        script_owner(&fake, &["acme/a", "acme/b"]);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme"]), Duration::ZERO);
        one_cycle(&worker);

        fake.script()
            .owners
            .insert(owner("acme"), Ok(vec![repo("acme/a")]));
        fake.script().listable.insert(repo("acme/b"), Ok(false));
        let second = refresh_now(&worker);

        let (repos, owner_errors) = repos_of_cycle(&second);
        assert_eq!(repos, &BTreeSet::from([repo("acme/a")]));
        assert!(owner_errors.is_empty(), "{owner_errors:?}");
    }

    #[test]
    fn a_failed_direct_read_of_a_dropped_repository_is_suspect_and_keeps_the_list() {
        let fake = Arc::new(FakeSource::default());
        script_owner(&fake, &["acme/a", "acme/b"]);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme"]), Duration::ZERO);
        one_cycle(&worker);

        let failure = SourceError::Failed("boom".to_owned());
        fake.script()
            .owners
            .insert(owner("acme"), Ok(vec![repo("acme/a")]));
        fake.script()
            .listable
            .insert(repo("acme/b"), Err(failure.clone()));
        let second = refresh_now(&worker);

        let (repos, owner_errors) = repos_of_cycle(&second);
        assert_eq!(repos, &BTreeSet::from([repo("acme/a"), repo("acme/b")]));
        assert_eq!(
            owner_errors,
            &vec![(
                owner("acme"),
                OwnerProblem::Suspect(Suspicion::Unread(Item::Repo(repo("acme/b")), failure))
            )]
        );
    }

    #[test]
    fn a_cut_list_that_drops_a_repository_is_accepted_without_a_direct_read() {
        let fake = Arc::new(FakeSource::default());
        script_owner(&fake, &["acme/a", "acme/b"]);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme"]), Duration::ZERO);
        one_cycle(&worker);

        fake.script()
            .owners
            .insert(owner("acme"), Ok(vec![repo("acme/a")]));
        fake.script().cut.insert(owner("acme"));
        let second = refresh_now(&worker);

        let (repos, owner_errors) = repos_of_cycle(&second);
        assert_eq!(repos, &BTreeSet::from([repo("acme/a")]));
        assert!(
            matches!(owner_errors.as_slice(), [(_, OwnerProblem::Cut(_))]),
            "{owner_errors:?}"
        );
        let reads = fake
            .script()
            .calls
            .iter()
            .filter(|(what, _)| *what == "listable")
            .count();
        assert_eq!(reads, 0, "a cut list gives no suspicion");
    }

    fn job(id: u64) -> Job {
        Job {
            id: crate::job::JobId(id),
            name: format!("job {id}"),
            state: RunState::Passed,
            runner: None,
            created_at: None,
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

    /// The runs that the worker told the source to forget, in order.
    fn forget_calls(fake: &FakeSource) -> Vec<String> {
        fake.script()
            .calls
            .iter()
            .filter(|(what, _)| *what == "forget")
            .map(|(_, run)| run.clone())
            .collect()
    }

    /// Runs one `fetch_jobs` for run 5 of `acme/a`. The facts of the run are
    /// those of `the_run`, or none. The source gives `answer`.
    fn fetch_run_5(
        the_run: Option<&Run>,
        answer: Result<Vec<Job>, SourceError>,
    ) -> Arc<FakeSource> {
        let fake = Arc::new(FakeSource::default());
        fake.script().jobs.insert(RunId(5), answer);
        let state = State::new(SLOW_CADENCE);
        if let Some(the_run) = the_run {
            state
                .facts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(repo("acme/a"))
                .or_default()
                .insert(the_run.id, RunFacts::of(the_run));
        }
        let (updates_tx, _updates_rx) = mpsc::channel();
        let sent = fetch_jobs(&fake, &state, &[(repo("acme/a"), RunId(5))], &updates_tx);
        assert!(sent.is_ok(), "the receiver lives");
        fake
    }

    #[test]
    fn a_finished_run_with_no_active_job_tells_the_source_to_forget_its_jobs() {
        let fake = fetch_run_5(Some(&run("acme/a", 5)), Ok(vec![job(1)]));
        assert_eq!(forget_calls(&fake), vec!["5".to_owned()]);
    }

    #[test]
    fn an_active_run_whose_jobs_are_all_finished_keeps_its_cached_jobs() {
        let fake = fetch_run_5(Some(&active_run("acme/a", 5)), Ok(vec![job(1)]));
        assert_eq!(forget_calls(&fake), Vec::<String>::new());
    }

    #[test]
    fn a_finished_run_with_an_active_job_keeps_its_cached_jobs() {
        let running = Job {
            state: RunState::Running,
            ..job(1)
        };
        let fake = fetch_run_5(Some(&run("acme/a", 5)), Ok(vec![running]));
        assert_eq!(forget_calls(&fake), Vec::<String>::new());
    }

    #[test]
    fn a_run_with_no_facts_forgets_its_cached_jobs() {
        let fake = fetch_run_5(None, Ok(vec![job(1)]));
        assert_eq!(forget_calls(&fake), vec!["5".to_owned()]);
    }

    #[test]
    fn a_run_with_no_facts_whose_job_fetch_failed_forgets_its_cached_jobs() {
        let fake = fetch_run_5(None, Err(SourceError::Failed("boom".to_owned())));
        assert_eq!(forget_calls(&fake), vec!["5".to_owned()]);
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
        // Four requests: the first cycle, the fetch at once after the watch,
        // and the two refresh cycles.
        assert_eq!(job_calls(&fake), 4);
    }

    /// The runs answer says that run 5 finished. The jobs answer shows a job
    /// that still runs. The `updated_at` of the run stays the same in each
    /// cycle, so only the active job can make the run due again.
    #[test]
    fn a_finished_run_with_an_active_job_gets_its_jobs_again() {
        let fake = fake_with_runs(vec![run("acme/a", 5)]);
        let running = Job {
            state: RunState::Running,
            ..job(1)
        };
        fake.script().jobs.insert(RunId(5), Ok(vec![running]));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let _ = one_cycle(&worker);
        watch(&worker, &[5]);
        assert!(matches!(next_update(&worker), Update::Jobs { .. }));

        refresh(&worker);
        let _ = one_cycle(&worker);

        assert_eq!(job_calls(&fake), 2, "requests for the jobs of run 5");
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

    /// GitHub reports a run as queued while one job waits for a runner, also
    /// when another job runs. The screen needs the jobs of a collapsed run to
    /// show it as running, thus the worker fetches the jobs of every active run.
    #[test]
    fn a_collapsed_queued_run_gets_its_job_states_in_one_cycle() {
        let queued = Run {
            state: RunState::Queued,
            ..run("acme/a", 5)
        };
        let fake = fake_with_runs(vec![queued]);
        let jobs = vec![
            Job {
                state: RunState::Running,
                ..job(1)
            },
            Job {
                state: RunState::Queued,
                ..job(2)
            },
        ];
        fake.script().jobs.insert(RunId(5), Ok(jobs.clone()));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);

        let cycle = one_cycle(&worker);

        assert!(
            cycle.contains(&Update::Jobs {
                run: RunId(5),
                jobs
            }),
            "{cycle:?}"
        );
    }

    /// The jobs of run 5 are fetched while the run is active. Then the run
    /// finishes and the screen shows it collapsed all the time. The worker
    /// must fetch the jobs one last time, so the board sees the final states.
    #[test]
    fn a_collapsed_run_that_finishes_gets_one_last_fetch_and_is_forgotten() {
        let fake = fake_with_runs(vec![active_run("acme/a", 5)]);
        let running = Job {
            state: RunState::Running,
            ..job(1)
        };
        fake.script().jobs.insert(RunId(5), Ok(vec![running]));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let first = one_cycle(&worker);
        assert!(has_jobs(&first), "{first:?}");
        let mut finished = run("acme/a", 5);
        finished.updated_at += chrono::TimeDelta::minutes(1);
        fake.script()
            .created
            .insert(repo("acme/a"), Ok(vec![finished]));
        fake.script().jobs.insert(RunId(5), Ok(vec![job(1)]));

        refresh(&worker);
        let second = one_cycle(&worker);
        let forgotten = forget_calls(&fake);
        refresh(&worker);
        let third = one_cycle(&worker);

        assert!(
            second.contains(&Update::Jobs {
                run: RunId(5),
                jobs: vec![job(1)]
            }),
            "{second:?}"
        );
        assert_eq!(forgotten, vec!["5".to_owned()]);
        assert!(!has_jobs(&third), "{third:?}");
        assert_eq!(job_calls(&fake), 2);
    }

    /// A person collapses run 5 while it is active. The run then finishes. The
    /// run keeps its mark, so the worker still fetches its jobs one last time.
    #[test]
    fn a_run_that_finishes_after_a_collapse_gets_one_last_fetch() {
        let fake = fake_with_runs(vec![active_run("acme/a", 5)]);
        let running = Job {
            state: RunState::Running,
            ..job(1)
        };
        fake.script().jobs.insert(RunId(5), Ok(vec![running]));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let _ = one_cycle(&worker);
        watch(&worker, &[5]);
        assert!(matches!(next_update(&worker), Update::Jobs { .. }));
        let mut finished = run("acme/a", 5);
        finished.updated_at += chrono::TimeDelta::minutes(1);
        fake.script()
            .created
            .insert(repo("acme/a"), Ok(vec![finished]));
        fake.script().jobs.insert(RunId(5), Ok(vec![job(1)]));

        watch(&worker, &[]);
        refresh(&worker);
        let last = one_cycle(&worker);
        let forgotten = forget_calls(&fake);
        refresh(&worker);
        let after = one_cycle(&worker);

        assert!(
            last.contains(&Update::Jobs {
                run: RunId(5),
                jobs: vec![job(1)]
            }),
            "{last:?}"
        );
        assert_eq!(forgotten, vec!["5".to_owned()]);
        assert!(!has_jobs(&after), "{after:?}");
    }

    /// The state has no thread here. A run that joins the watched set loses its
    /// mark, so the next cycle fetches it, also when the fetch at once fails to
    /// happen. The mark says the run is finished and unchanged, thus only the
    /// removal at the join can make the run due.
    #[test]
    fn a_run_that_joins_the_watched_set_again_is_due() {
        let finished = run("acme/a", 5);
        let key = (repo("acme/a"), RunId(5));
        let mut state = State::new(SLOW_CADENCE);
        let facts = RunFacts::of(&finished);
        state
            .facts
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(repo("acme/a"))
            .or_default()
            .insert(finished.id, facts);
        state
            .job_marks
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                key.clone(),
                RunFacts {
                    active: false,
                    ..facts
                },
            );

        let _ = state.watch([key.clone()].into_iter().collect());
        let _ = state.watch(BTreeSet::new());
        let _ = state.watch([key.clone()].into_iter().collect());

        assert_eq!(state.due_jobs(), vec![key]);
    }

    /// The fake lists run 5 in the first cycle and no run in the second cycle.
    /// The mark of run 5 must go with the run.
    #[test]
    fn the_mark_of_a_run_that_leaves_the_list_of_runs_is_removed() {
        let fake = fake_with_runs(vec![run("acme/a", 5)]);
        let mut state = State::new(SLOW_CADENCE);
        let (updates_tx, _updates_rx) = mpsc::channel();
        let settings = settings(&["acme/a"]);
        assert!(cycle(&fake, &settings, &mut state, &updates_tx).is_ok());
        let key = (repo("acme/a"), RunId(5));
        let facts = RunFacts::of(&run("acme/a", 5));
        state
            .job_marks
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                key.clone(),
                RunFacts {
                    active: false,
                    ..facts
                },
            );
        fake.script().created.insert(repo("acme/a"), Ok(Vec::new()));

        assert!(cycle(&fake, &settings, &mut state, &updates_tx).is_ok());

        let marks = state
            .job_marks
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        assert!(!marks.contains_key(&key), "{marks:?}");
    }

    /// Run 5 stays watched and its jobs are fetched once. Then it leaves the
    /// list of runs. The board ignores a job answer for such a run, so the
    /// worker must not fetch its jobs again.
    #[test]
    fn a_watched_run_that_leaves_the_list_of_runs_gets_no_job_fetch() {
        let fake = fake_with_runs(vec![run("acme/a", 5)]);
        fake.script().jobs.insert(RunId(5), Ok(vec![job(1)]));
        let mut state = State::new(SLOW_CADENCE);
        let (updates_tx, updates_rx) = mpsc::channel();
        let settings = settings(&["acme/a"]);
        let key = (repo("acme/a"), RunId(5));
        let _ = state.watch([key].into_iter().collect());
        assert!(cycle(&fake, &settings, &mut state, &updates_tx).is_ok());
        assert!(cycle(&fake, &settings, &mut state, &updates_tx).is_ok());
        assert_eq!(job_calls(&fake), 1);
        assert_eq!(forget_calls(&fake), vec!["5".to_owned()]);
        while updates_rx.try_recv().is_ok() {}
        fake.script().created.insert(repo("acme/a"), Ok(Vec::new()));

        assert!(cycle(&fake, &settings, &mut state, &updates_tx).is_ok());

        let updates: Vec<Update> = updates_rx.try_iter().collect();
        assert!(!has_jobs(&updates), "{updates:?}");
        assert_eq!(job_calls(&fake), 1);
        assert_eq!(forget_calls(&fake), vec!["5".to_owned()]);
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

    /// Run 5 is never watched. It finishes and its last job fetch fails. The
    /// next cycle must fetch it again and forget its pages when it is good.
    #[test]
    fn a_collapsed_run_whose_last_fetch_failed_is_fetched_again_and_forgotten() {
        let fake = fake_with_runs(vec![active_run("acme/a", 5)]);
        let running = Job {
            state: RunState::Running,
            ..job(1)
        };
        fake.script().jobs.insert(RunId(5), Ok(vec![running]));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);
        let first = one_cycle(&worker);
        assert!(has_jobs(&first), "{first:?}");
        let mut finished = run("acme/a", 5);
        finished.updated_at += chrono::TimeDelta::minutes(1);
        fake.script()
            .created
            .insert(repo("acme/a"), Ok(vec![finished]));
        fake.script()
            .jobs
            .insert(RunId(5), Err(SourceError::Failed("boom".to_owned())));

        refresh(&worker);
        let second = one_cycle(&worker);
        fake.script().jobs.insert(RunId(5), Ok(vec![job(1)]));
        refresh(&worker);
        let third = one_cycle(&worker);
        let forgotten = forget_calls(&fake);
        refresh(&worker);
        let fourth = one_cycle(&worker);

        assert!(
            second
                .iter()
                .any(|update| matches!(update, Update::JobsFailed { run: RunId(5), .. })),
            "{second:?}"
        );
        assert!(
            third.contains(&Update::Jobs {
                run: RunId(5),
                jobs: vec![job(1)]
            }),
            "{third:?}"
        );
        assert_eq!(forgotten, vec!["5".to_owned()]);
        assert!(!has_jobs(&fourth), "{fourth:?}");
        assert_eq!(job_calls(&fake), 3);
    }

    #[test]
    fn a_collapsed_finished_run_is_not_fetched_and_an_expanded_again_run_is_fetched_at_once() {
        let fake = fake_with_runs(vec![run("acme/a", 5)]);
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

    /// The three commands are in the queue before the call. The backlog drain
    /// sees all of them, thus it adds run 5 two times. The run must still get
    /// one request and one answer. The test uses no thread and no delay, thus
    /// no timing can change the result.
    #[test]
    fn a_run_that_the_backlog_adds_two_times_is_fetched_one_time() {
        let fake = Arc::new(FakeSource::default());
        let (updates_tx, updates_rx) = mpsc::channel();
        let (commands_tx, commands_rx) = mpsc::channel();
        let run_5: BTreeSet<RunKey> = [(repo("acme/a"), RunId(5))].into_iter().collect();
        for set in [run_5.clone(), BTreeSet::new(), run_5] {
            commands_tx
                .send(Command::WatchJobs(set))
                .expect("the receiver is alive");
        }

        let wake = wait_for_next_cycle(
            &fake,
            &mut State::new(SLOW_CADENCE),
            &updates_tx,
            &commands_rx,
            Duration::ZERO,
            false,
        );

        assert!(matches!(wake, Ok(Wake::Cycle)));
        let answers = updates_rx
            .try_iter()
            .filter(|update| matches!(update, Update::Jobs { run: RunId(5), .. }))
            .count();
        assert_eq!(job_calls(&fake), 1, "requests for the jobs of run 5");
        assert_eq!(answers, 1, "Jobs updates for run 5");
        drop(commands_tx);
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

    /// At the end of each cycle, the source holds the job pages of a run only
    /// while the run has an active mark. Only an active mark makes the worker
    /// fetch the run again, so no other run can use its pages.
    fn assert_held_pages_have_active_marks(fake: &FakeSource, state: &State) {
        let held = fake.script().held.clone();
        let marks = state
            .job_marks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for key in &held {
            assert!(
                marks.get(key).is_some_and(|mark| mark.active),
                "the source holds {key:?} with no active mark: held {held:?}, marks {marks:?}"
            );
        }
    }

    /// Run 5 is active and not watched. Its jobs are fetched in the first
    /// cycle. Then it leaves the list of runs. The worker never fetches it
    /// again, so it must tell the source to forget its pages.
    #[test]
    fn an_active_run_that_is_not_watched_and_leaves_the_list_of_runs_is_forgotten() {
        let fake = fake_with_runs(vec![active_run("acme/a", 5)]);
        let mut state = State::new(SLOW_CADENCE);
        let (updates_tx, _updates_rx) = mpsc::channel();
        let settings = settings(&["acme/a"]);
        assert!(cycle(&fake, &settings, &mut state, &updates_tx).is_ok());
        assert!(fake.script().held.contains(&(repo("acme/a"), RunId(5))));
        fake.script().created.insert(repo("acme/a"), Ok(Vec::new()));

        assert!(cycle(&fake, &settings, &mut state, &updates_tx).is_ok());

        assert_held_pages_have_active_marks(&fake, &state);
        assert_eq!(forget_calls(&fake), vec!["5".to_owned()]);
    }

    /// Run 5 is finished and watched. Its job fetch fails, so it gets an
    /// active mark. Then it leaves the list of runs. The failed fetch can leave
    /// pages, so the worker must tell the source to forget them.
    #[test]
    fn a_run_whose_last_job_fetch_failed_and_that_leaves_the_list_of_runs_is_forgotten() {
        let fake = fake_with_runs(vec![run("acme/a", 5)]);
        fake.script()
            .jobs
            .insert(RunId(5), Err(SourceError::Failed("boom".to_owned())));
        let mut state = State::new(SLOW_CADENCE);
        let (updates_tx, _updates_rx) = mpsc::channel();
        let settings = settings(&["acme/a"]);
        let _ = state.watch([(repo("acme/a"), RunId(5))].into_iter().collect());
        assert!(cycle(&fake, &settings, &mut state, &updates_tx).is_ok());
        assert!(fake.script().held.contains(&(repo("acme/a"), RunId(5))));
        fake.script().created.insert(repo("acme/a"), Ok(Vec::new()));

        assert!(cycle(&fake, &settings, &mut state, &updates_tx).is_ok());

        assert_held_pages_have_active_marks(&fake, &state);
        assert_eq!(forget_calls(&fake), vec!["5".to_owned()]);
    }

    /// Run 5 is active. Its repository comes from an owner list. Then the
    /// owner lists no repository, so the run has no facts and is never due.
    #[test]
    fn an_active_run_whose_repository_leaves_the_repository_set_is_forgotten() {
        let fake = fake_with_runs(vec![active_run("acme/a", 5)]);
        fake.script()
            .owners
            .insert(owner("acme"), Ok(vec![repo("acme/a")]));
        let mut state = State::new(Duration::ZERO);
        let (updates_tx, _updates_rx) = mpsc::channel();
        let settings = settings(&["acme"]);
        assert!(cycle(&fake, &settings, &mut state, &updates_tx).is_ok());
        assert!(fake.script().held.contains(&(repo("acme/a"), RunId(5))));
        fake.script().owners.insert(owner("acme"), Ok(Vec::new()));

        assert!(cycle(&fake, &settings, &mut state, &updates_tx).is_ok());

        assert_held_pages_have_active_marks(&fake, &state);
        assert_eq!(forget_calls(&fake), vec!["5".to_owned()]);
    }

    /// Run 5 is active and not watched. A watch command during a backoff
    /// removes its mark and fetches nothing. Then the run leaves the list of
    /// runs. It has no mark for the cycle to see, but the source holds pages.
    #[test]
    fn a_run_that_joins_the_watched_set_during_a_backoff_and_leaves_the_list_of_runs_is_forgotten()
    {
        let fake = fake_with_runs(vec![active_run("acme/a", 5)]);
        let mut state = State::new(SLOW_CADENCE);
        let (updates_tx, _updates_rx) = mpsc::channel();
        let (commands_tx, commands_rx) = mpsc::channel();
        let settings = settings(&["acme/a"]);
        assert!(cycle(&fake, &settings, &mut state, &updates_tx).is_ok());
        assert!(fake.script().held.contains(&(repo("acme/a"), RunId(5))));
        let calls_before = job_calls(&fake);
        commands_tx
            .send(Command::WatchJobs(
                [(repo("acme/a"), RunId(5))].into_iter().collect(),
            ))
            .expect("the receiver lives");
        let woke = wait_for_next_cycle(
            &fake,
            &mut state,
            &updates_tx,
            &commands_rx,
            Duration::ZERO,
            true,
        );
        assert!(woke.is_ok());
        assert_eq!(job_calls(&fake), calls_before);
        fake.script().created.insert(repo("acme/a"), Ok(Vec::new()));

        assert!(cycle(&fake, &settings, &mut state, &updates_tx).is_ok());

        assert_held_pages_have_active_marks(&fake, &state);
        assert_eq!(forget_calls(&fake), vec!["5".to_owned()]);
        drop(commands_tx);
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

    /// A run of `acme/a` that GitHub created `minutes` ago.
    fn created_ago(id: u64, minutes: i64) -> Run {
        let at = Utc::now() - chrono::TimeDelta::minutes(minutes);
        Run {
            created_at: at,
            started_at: at,
            ..run("acme/a", id)
        }
    }

    /// Scripts the created answer of `acme/a`.
    fn script_created(fake: &FakeSource, runs: Vec<Run>) {
        fake.script().created.insert(repo("acme/a"), Ok(runs));
    }

    /// Scripts the direct read of each run as "still there".
    fn script_direct(fake: &FakeSource, runs: &[Run]) {
        let mut script = fake.script();
        for run in runs {
            script.direct.insert(run.id, Ok(Some(run.clone())));
        }
    }

    /// The updates of `acme/a` that carry runs, a suspicion, or a failure.
    fn answers_of_a(updates: &[Update]) -> Vec<&Update> {
        updates
            .iter()
            .filter(|update| match update {
                Update::Runs { repo: r, .. }
                | Update::Suspect { repo: r, .. }
                | Update::Failed { repo: r, .. } => *r == repo("acme/a"),
                _ => false,
            })
            .collect()
    }

    fn calls_named(fake: &FakeSource, name: &str) -> Vec<String> {
        fake.script()
            .calls
            .iter()
            .filter(|(what, _)| *what == name)
            .map(|(_, detail)| detail.clone())
            .collect()
    }

    fn three_runs() -> Vec<Run> {
        vec![created_ago(1, 10), created_ago(2, 20), created_ago(3, 30)]
    }

    #[test]
    fn an_empty_created_answer_after_three_runs_is_suspect_and_keeps_the_runs() {
        let fake = Arc::new(FakeSource::default());
        let runs = three_runs();
        script_created(&fake, runs.clone());
        script_direct(&fake, &runs);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), Duration::ZERO);
        one_cycle(&worker);

        script_created(&fake, Vec::new());
        let second = refresh_now(&worker);

        let answers = answers_of_a(&second);
        assert!(
            matches!(
                answers.as_slice(),
                [Update::Suspect {
                    reason: Suspicion::LeftOut(Item::Run(RunId(1))),
                    ..
                }]
            ),
            "{answers:?}"
        );
        assert_eq!(
            calls_named(&fake, "forget-runs"),
            vec!["acme/a created".to_owned()]
        );
    }

    #[test]
    fn a_created_answer_that_holds_the_runs_again_is_good_after_a_suspect_one() {
        let fake = Arc::new(FakeSource::default());
        let runs = three_runs();
        script_created(&fake, runs.clone());
        script_direct(&fake, &runs);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), Duration::ZERO);
        one_cycle(&worker);
        script_created(&fake, Vec::new());
        refresh_now(&worker);

        script_created(&fake, runs.clone());
        let third = refresh_now(&worker);

        let answers = answers_of_a(&third);
        assert!(
            matches!(answers.as_slice(), [Update::Runs { runs: got, .. }] if *got == runs),
            "{answers:?}"
        );
    }

    #[test]
    fn a_run_that_the_answer_leaves_out_and_that_is_deleted_is_not_suspect() {
        let fake = Arc::new(FakeSource::default());
        let runs = three_runs();
        script_created(&fake, runs.clone());
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), Duration::ZERO);
        one_cycle(&worker);

        // The fake answers `Ok(None)` for run 3: GitHub deleted it.
        script_created(&fake, runs[..2].to_vec());
        script_direct(&fake, &runs[..2]);
        let second = refresh_now(&worker);

        let answers = answers_of_a(&second);
        assert!(
            matches!(answers.as_slice(), [Update::Runs { runs: got, .. }] if *got == runs[..2]),
            "{answers:?}"
        );
        assert!(calls_named(&fake, "forget-runs").is_empty());
    }

    #[test]
    fn a_run_from_before_the_window_that_the_answer_leaves_out_is_never_read() {
        let fake = Arc::new(FakeSource::default());
        // Three days old. The window is 8 hours, plus one day.
        let old = created_ago(1, 3 * 24 * 60);
        let recent = created_ago(2, 10);
        script_created(&fake, vec![old.clone(), recent.clone()]);
        script_direct(&fake, &[old, recent.clone()]);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), Duration::ZERO);
        one_cycle(&worker);

        script_created(&fake, vec![recent.clone()]);
        let second = refresh_now(&worker);

        let answers = answers_of_a(&second);
        assert!(
            matches!(answers.as_slice(), [Update::Runs { runs: got, .. }] if *got == [recent]),
            "{answers:?}"
        );
        assert!(calls_named(&fake, "run").is_empty());
    }

    #[test]
    fn a_cut_answer_may_leave_out_a_run_older_than_its_oldest_run() {
        let fake = Arc::new(FakeSource::default());
        let runs = three_runs();
        fake.script().cut_runs.insert(repo("acme/a"));
        script_created(&fake, runs.clone());
        script_direct(&fake, &runs);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), Duration::ZERO);
        one_cycle(&worker);

        // The second answer is cut after run 2. Run 3 is older than run 2.
        script_created(&fake, runs[..2].to_vec());
        let second = refresh_now(&worker);

        let answers = answers_of_a(&second);
        assert!(
            matches!(answers.as_slice(), [Update::Runs { runs: got, .. }] if *got == runs[..2]),
            "{answers:?}"
        );
        assert!(calls_named(&fake, "run").is_empty());
    }

    #[test]
    fn a_failed_direct_read_of_a_left_out_run_is_suspect() {
        let fake = Arc::new(FakeSource::default());
        let runs = three_runs();
        script_created(&fake, runs);
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), Duration::ZERO);
        one_cycle(&worker);

        let boom = SourceError::Failed("boom".to_owned());
        fake.script().direct.insert(RunId(1), Err(boom.clone()));
        script_created(&fake, Vec::new());
        let second = refresh_now(&worker);

        let answers = answers_of_a(&second);
        assert!(
            matches!(
                answers.as_slice(),
                [Update::Suspect { reason: Suspicion::Unread(Item::Run(RunId(1)), error), .. }]
                    if *error == boom
            ),
            "{answers:?}"
        );
    }

    #[test]
    fn the_jobs_of_an_active_run_of_a_suspect_repository_are_fetched_again() {
        let fake = Arc::new(FakeSource::default());
        let active = Run {
            state: RunState::Running,
            ..created_ago(1, 5)
        };
        script_created(&fake, vec![active.clone()]);
        script_direct(&fake, std::slice::from_ref(&active));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), Duration::ZERO);
        one_cycle(&worker);

        script_created(&fake, Vec::new());
        let second = refresh_now(&worker);

        assert!(
            matches!(answers_of_a(&second).as_slice(), [Update::Suspect { .. }]),
            "{second:?}"
        );
        assert_eq!(
            calls_named(&fake, "jobs").len(),
            2,
            "one fetch in each cycle"
        );
        assert!(second
            .iter()
            .any(|update| matches!(update, Update::Jobs { run: RunId(1), .. })));
    }

    #[test]
    fn an_owner_whose_first_list_is_empty_is_listed_again_before_the_cadence() {
        let fake = Arc::new(FakeSource::default());
        fake.script().owners.insert(owner("acme"), Ok(Vec::new()));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme"]), SLOW_CADENCE);
        let first = one_cycle(&worker);
        assert!(repos_of_cycle(&first).0.is_empty());

        script_owner(&fake, &["acme/a"]);
        let second = refresh_now(&worker);

        assert_eq!(
            repos_of_cycle(&second).0,
            &BTreeSet::from([repo("acme/a")]),
            "{second:?}"
        );
    }

    /// Scripts the answer of `newest_run` for `acme/a`.
    fn script_newest(fake: &FakeSource, answer: Result<Option<Run>, SourceError>) {
        fake.script().newest.insert(repo("acme/a"), answer);
    }

    #[test]
    fn an_empty_created_answer_with_a_newest_run_in_the_window_is_suspect() {
        let fake = Arc::new(FakeSource::default());
        script_newest(&fake, Ok(Some(created_ago(9, 10))));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);

        let first = one_cycle(&worker);

        let answers = answers_of_a(&first);
        assert!(
            matches!(
                answers.as_slice(),
                [Update::Suspect {
                    reason: Suspicion::NewestLeftOut(RunId(9)),
                    ..
                }]
            ),
            "{answers:?}"
        );
        assert_eq!(
            calls_named(&fake, "forget-runs"),
            vec!["acme/a created".to_owned()]
        );
    }

    #[test]
    fn an_empty_created_answer_of_a_quiet_repository_is_good_after_one_probe() {
        for newest in [None, Some(created_ago(9, 3 * 24 * 60))] {
            let fake = Arc::new(FakeSource::default());
            script_newest(&fake, Ok(newest.clone()));
            let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), Duration::ZERO);

            let first = one_cycle(&worker);

            let answers = answers_of_a(&first);
            assert!(
                matches!(answers.as_slice(), [Update::Runs { runs, .. }] if runs.is_empty()),
                "{newest:?}: {answers:?}"
            );
            assert_eq!(calls_named(&fake, "newest").len(), 1, "{newest:?}");
        }
    }

    #[test]
    fn a_failed_read_of_the_newest_run_makes_an_empty_answer_suspect() {
        let fake = Arc::new(FakeSource::default());
        let boom = SourceError::Failed("boom".to_owned());
        script_newest(&fake, Err(boom.clone()));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);

        let first = one_cycle(&worker);

        let answers = answers_of_a(&first);
        assert!(
            matches!(
                answers.as_slice(),
                [Update::Suspect { reason: Suspicion::NewestUnread(error), .. }] if *error == boom
            ),
            "{answers:?}"
        );
    }

    #[test]
    fn the_newest_run_is_read_one_time_when_the_created_answer_and_the_sweep_are_empty() {
        let fake = Arc::new(FakeSource::default());
        script_newest(&fake, Ok(Some(created_ago(9, 3 * 24 * 60))));
        let worker = spawn_with(Arc::clone(&fake), &settings(&["acme/a"]), SLOW_CADENCE);

        let first = one_cycle(&worker);

        assert_eq!(calls_named(&fake, "active").len(), 1, "the sweep ran");
        assert_eq!(calls_named(&fake, "newest").len(), 1);
        assert!(
            matches!(answers_of_a(&first).as_slice(), [Update::Runs { runs, .. }] if runs.is_empty()),
            "{first:?}"
        );
    }
}
