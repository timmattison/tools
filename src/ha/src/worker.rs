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
    pub fn due(&self, _now: Instant) -> bool {
        false
    }

    /// Records that the job ran at `now`.
    pub fn mark(&mut self, now: Instant) {
        self.last = Some(now);
    }
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
    use super::*;

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
}
