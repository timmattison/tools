//! One job of a GitHub Actions run.
//!
//! A run holds one or more jobs. The statuses of a job are a subset of the
//! statuses of a run, thus a job uses [`RunState`] for its state.

use chrono::{DateTime, Utc};

use crate::run::RunState;

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
    /// When GitHub created the job. A job that waits, waits from this time.
    /// The value is `None` when GitHub gives none.
    pub created_at: Option<DateTime<Utc>>,
    /// When the job started. The creation time is the fallback. The value is
    /// `None` when GitHub gives neither time.
    pub started_at: Option<DateTime<Utc>>,
    /// When the job finished. The value is `None` while the job is active.
    pub completed_at: Option<DateTime<Utc>>,
    /// The address of the job page on GitHub. The text is empty when GitHub
    /// gives none.
    pub url: String,
}
