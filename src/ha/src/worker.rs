//! The background fetch loop.
//!
//! The worker runs on its own thread. It asks the [`Source`] for the runs of
//! every repository, sends each answer to the screen as an [`Update`], and
//! then waits for the next cycle. The screen sends [`Command`] values back.

use std::collections::BTreeSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::github::Source;
use crate::run::Run;
use crate::settings::Settings;
use crate::status::RateLimit;
use crate::target::{OwnerName, RepoName};

/// How many repositories the worker fetches at the same time.
pub const CONCURRENCY: usize = 8;

/// How often the worker lists the repositories of the owners and sweeps for
/// active runs. The sweep finds runs that started before the window.
pub const SLOW_CADENCE: Duration = Duration::from_secs(600);

/// One message from the worker to the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// The repositories to watch now, and the owners that could not be listed.
    Repos {
        /// Every repository to watch, without duplicates.
        repos: BTreeSet<RepoName>,
        /// The owners whose list failed, with the error text.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Start the next cycle now.
    RefreshNow,
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

fn spawn_with<S: Source + 'static>(_source: S, _settings: &Settings, _slow: Duration) -> Worker {
    let (_updates_tx, updates) = mpsc::channel();
    let (commands, _commands_rx) = mpsc::channel();
    Worker { updates, commands }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::mpsc::RecvTimeoutError;
    use std::sync::{Arc, Mutex, PoisonError};

    use super::*;
    use crate::github::{RunQuery, SourceError};
    use crate::run::{RunId, RunState};
    use crate::target::Target;

    /// The longest a test waits for one update.
    const WAIT: Duration = Duration::from_secs(30);

    /// The answers and the call log of a [`FakeSource`].
    #[derive(Default)]
    struct Script {
        owners: HashMap<OwnerName, Result<Vec<RepoName>, SourceError>>,
        created: HashMap<RepoName, Result<Vec<Run>, SourceError>>,
        calls: Vec<(&'static str, String)>,
    }

    /// A [`Source`] that answers from a script and logs each call.
    #[derive(Default)]
    struct FakeSource {
        script: Mutex<Script>,
    }

    impl FakeSource {
        fn script(&self) -> std::sync::MutexGuard<'_, Script> {
            self.script.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl Source for Arc<FakeSource> {
        fn repos_of(&self, owner: &OwnerName) -> Result<Vec<RepoName>, SourceError> {
            let mut script = self.script();
            script.calls.push(("owner", owner.to_string()));
            script.owners.get(owner).cloned().unwrap_or(Ok(Vec::new()))
        }

        fn runs_of(&self, repo: &RepoName, query: RunQuery) -> Result<Vec<Run>, SourceError> {
            let mut script = self.script();
            let what = match query {
                RunQuery::CreatedSince(_) => "created",
                RunQuery::Active => "active",
            };
            script.calls.push((what, repo.to_string()));
            script.created.get(repo).cloned().unwrap_or(Ok(Vec::new()))
        }

        fn rate_limit(&self) -> Option<RateLimit> {
            None
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
            updated_at: at,
            url: format!("https://example.test/{id}"),
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
}
