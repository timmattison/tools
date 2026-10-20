//! One job of a GitHub Actions run.
//!
//! A run holds one or more jobs. The statuses of a job are a subset of the
//! statuses of a run, thus a job uses [`RunState`] for its state.

use chrono::{DateTime, Utc};

use crate::run::RunState;

/// The name of the runner that has a job. The name is never empty.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RunnerName(String);

impl RunnerName {
    /// Makes a runner name from the text that GitHub gives.
    ///
    /// Returns `None` for an empty text. The text is not trimmed.
    #[must_use]
    pub fn new(name: String) -> Option<RunnerName> {
        if name.is_empty() {
            None
        } else {
            Some(RunnerName(name))
        }
    }

    /// The name as text. The text is never empty.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The database ID that GitHub gives to one job of a workflow run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub u64);

/// One job of a workflow run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// The database ID from GitHub.
    pub id: JobId,
    /// The name of the job. The text is empty when GitHub gives none.
    pub name: String,
    /// What the job is doing now, or how it ended.
    pub state: RunState,
    /// The runner that has the job. The value is `None` while the job waits for
    /// a runner, and when the job ended before a runner took it. A name that is
    /// not empty always shows, whatever the state of the job, because a name on
    /// a waiting job means that GitHub gave the job to that runner.
    pub runner: Option<RunnerName>,
    /// When GitHub created the job. A job that waits, waits from this time.
    /// The value is `None` when GitHub gives none.
    pub created_at: Option<DateTime<Utc>>,
    /// When a runner started the job. The value is `None` while the job waits,
    /// also when GitHub gives a start time for it. A job that does not wait and
    /// has no start time takes its creation time. The value is `None` when
    /// GitHub gives neither time.
    pub started_at: Option<DateTime<Utc>>,
    /// When the job finished. The value is `None` while the job is active.
    pub completed_at: Option<DateTime<Utc>>,
    /// The address of the job page on GitHub. The text is empty when GitHub
    /// gives none.
    pub url: String,
}
