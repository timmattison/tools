//! One GitHub Actions run, as the board and the source share it.
//!
//! GitHub names many statuses and conclusions. The board needs only five
//! states, thus [`RunState`] folds the GitHub values into those five.

use chrono::{DateTime, Utc};

use crate::target::RepoName;

/// The number that GitHub gives to one workflow run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(pub u64);

/// What a run is doing now, or how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// The run waits for a runner or for an approval.
    Queued,
    /// The run is in progress.
    Running,
    /// The run finished and succeeded.
    Passed,
    /// The run finished and failed, timed out, or did not start.
    Failed,
    /// Any other state, for example a cancelled or skipped run.
    Other,
}

impl RunState {
    /// Folds a GitHub `status` and `conclusion` into a state.
    ///
    /// A value that GitHub does not document gives [`RunState::Other`].
    /// This function never fails.
    #[must_use]
    pub fn from_github(status: &str, conclusion: Option<&str>) -> RunState {
        match status {
            "completed" => match conclusion {
                Some("success") => RunState::Passed,
                Some("failure" | "timed_out" | "startup_failure") => RunState::Failed,
                _ => RunState::Other,
            },
            "queued" | "waiting" | "pending" | "requested" => RunState::Queued,
            "in_progress" => RunState::Running,
            _ => RunState::Other,
        }
    }
}

/// One workflow run of one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// The run number from GitHub.
    pub id: RunId,
    /// The repository that holds the run.
    pub repo: RepoName,
    /// The name of the workflow.
    pub workflow: String,
    /// The head branch. The text is empty when GitHub gives none.
    pub branch: String,
    /// The display title of the run. The name of the workflow is the fallback.
    pub title: String,
    /// What the run is doing now, or how it ended.
    pub state: RunState,
    /// When the run started. The creation time is the fallback.
    pub started_at: DateTime<Utc>,
    /// When the run last changed. For a finished run, this is the finish time.
    pub updated_at: DateTime<Utc>,
    /// The address of the run page on GitHub.
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_active_status_maps_to_its_state() {
        let cases = [
            ("queued", RunState::Queued),
            ("waiting", RunState::Queued),
            ("pending", RunState::Queued),
            ("requested", RunState::Queued),
            ("in_progress", RunState::Running),
        ];
        for (status, expected) in cases {
            assert_eq!(RunState::from_github(status, None), expected, "{status}");
        }
    }

    #[test]
    fn a_completed_run_maps_by_its_conclusion() {
        let cases = [
            ("success", RunState::Passed),
            ("failure", RunState::Failed),
            ("timed_out", RunState::Failed),
            ("startup_failure", RunState::Failed),
            ("cancelled", RunState::Other),
            ("skipped", RunState::Other),
            ("neutral", RunState::Other),
            ("action_required", RunState::Other),
            ("stale", RunState::Other),
        ];
        for (conclusion, expected) in cases {
            assert_eq!(
                RunState::from_github("completed", Some(conclusion)),
                expected,
                "{conclusion}"
            );
        }
    }
}
