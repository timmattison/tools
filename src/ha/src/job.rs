//! One job of a GitHub Actions run.
//!
//! A run holds one or more jobs. The statuses of a job are a subset of the
//! statuses of a run, thus a job uses [`RunState`] for its state.

use chrono::{DateTime, Utc};

use crate::run::{span, RunState};

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

impl Job {
    /// The time of this job on a runner. It is the time that the `TIME` cell of
    /// a job row shows after the job starts.
    ///
    /// A finished job gives the time from its start to its finish. A job that
    /// runs gives the time from its start to `now`. A job that waits gives
    /// `None`, because a wait uses no runner. A job with no start time gives
    /// `None`. A finished job with no finish time gives `None`. A finish time
    /// before the start time gives zero.
    ///
    /// `None` means that the job adds nothing to the `JOB SUM` of its run. The
    /// time uses whole seconds. It is not the billed time, because GitHub
    /// rounds each job up to a whole minute. Issue #639 rounds this time up to
    /// whole minutes for its cost, and calls this function rather than a copy
    /// of the rule.
    #[must_use]
    pub fn runner_time(&self, now: DateTime<Utc>) -> Option<std::time::Duration> {
        if self.state == RunState::Queued {
            return None;
        }
        let started = self.started_at?;
        if self.state == RunState::Running {
            Some(span(started, now))
        } else {
            self.completed_at.map(|done| span(started, done))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::TimeZone;

    use super::*;

    fn at(hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 6, hour, minute, second)
            .unwrap()
    }

    fn job(
        state: RunState,
        started_at: Option<DateTime<Utc>>,
        completed_at: Option<DateTime<Utc>>,
    ) -> Job {
        Job {
            id: JobId(1),
            name: "build".to_owned(),
            state,
            runner: None,
            created_at: Some(at(11, 59, 0)),
            started_at,
            completed_at,
            url: String::new(),
        }
    }

    #[test]
    fn a_finished_job_gives_its_start_to_its_finish() {
        let finished = job(RunState::Passed, Some(at(12, 0, 0)), Some(at(12, 3, 31)));
        assert_eq!(
            finished.runner_time(at(13, 0, 0)),
            Some(Duration::from_secs(211))
        );
    }

    #[test]
    fn a_running_job_gives_its_start_to_now() {
        let running = job(RunState::Running, Some(at(12, 0, 0)), None);
        assert_eq!(
            running.runner_time(at(12, 1, 0)),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            running.runner_time(at(12, 14, 57)),
            Some(Duration::from_secs(897))
        );
    }

    #[test]
    fn a_waiting_job_gives_nothing_even_with_times() {
        let waiting = job(RunState::Queued, Some(at(12, 0, 0)), Some(at(12, 3, 31)));
        assert_eq!(waiting.runner_time(at(12, 5, 0)), None);
    }

    #[test]
    fn a_job_with_no_start_time_gives_nothing() {
        let skipped = job(RunState::Skipped, None, None);
        assert_eq!(skipped.runner_time(at(12, 5, 0)), None);
        let running = job(RunState::Running, None, None);
        assert_eq!(running.runner_time(at(12, 5, 0)), None);
    }

    #[test]
    fn a_finished_job_with_no_finish_time_gives_nothing() {
        let finished = job(RunState::Failed, Some(at(12, 0, 0)), None);
        assert_eq!(finished.runner_time(at(12, 5, 0)), None);
    }

    #[test]
    fn a_finish_time_before_the_start_time_gives_zero() {
        let behind = job(RunState::Passed, Some(at(12, 3, 0)), Some(at(12, 0, 0)));
        assert_eq!(behind.runner_time(at(12, 5, 0)), Some(Duration::ZERO));
    }
}
