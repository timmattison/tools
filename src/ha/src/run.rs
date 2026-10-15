//! One GitHub Actions run, as the board and the source share it.
//!
//! GitHub names many statuses and conclusions. The board needs only five
//! states, thus [`RunState`] folds the GitHub values into those five.

use std::collections::HashSet;

use chrono::{DateTime, Utc};

use crate::target::RepoName;

/// The number that GitHub gives to one workflow run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(pub u64);

/// What a run is doing now, or how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// The run waits for a runner, for an approval, or for another run of its
    /// concurrency group.
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

    /// Returns true while the run waits or runs.
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, RunState::Queued | RunState::Running)
    }

    /// Returns true when the run passed or failed. The board shows these.
    #[must_use]
    pub fn is_finished(self) -> bool {
        matches!(self, RunState::Passed | RunState::Failed)
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

/// Keeps the first run of each ID and drops the later runs of the same ID.
///
/// The order of the kept runs does not change. A list can hold one run two
/// times when the run moves between two requests, and the board needs one row
/// for each ID.
pub fn keep_first_of_each_id(runs: &mut Vec<Run>) {
    let mut seen = HashSet::new();
    runs.retain(|run| seen.insert(run.id));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_with(id: u64, title: &str) -> Run {
        let time = DateTime::<Utc>::UNIX_EPOCH;
        Run {
            id: RunId(id),
            repo: RepoName::parse("o/r").expect("a valid repository"),
            workflow: "ci".to_owned(),
            branch: "main".to_owned(),
            title: title.to_owned(),
            state: RunState::Running,
            started_at: time,
            updated_at: time,
            url: String::new(),
        }
    }

    #[test]
    fn the_first_run_of_an_id_stays() {
        let mut runs = vec![run_with(5, "first"), run_with(5, "second")];
        keep_first_of_each_id(&mut runs);
        assert_eq!(runs, [run_with(5, "first")]);
    }

    #[test]
    fn the_order_of_the_kept_runs_does_not_change() {
        let mut runs = vec![
            run_with(7, "a"),
            run_with(3, "b"),
            run_with(7, "c"),
            run_with(9, "d"),
            run_with(3, "e"),
        ];
        keep_first_of_each_id(&mut runs);
        let ids: Vec<u64> = runs.iter().map(|run| run.id.0).collect();
        assert_eq!(ids, [7, 3, 9]);
    }

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

    #[test]
    fn a_value_that_github_does_not_document_maps_to_other() {
        assert_eq!(RunState::from_github("brand_new", None), RunState::Other);
        assert_eq!(
            RunState::from_github("brand_new", Some("success")),
            RunState::Other
        );
        assert_eq!(
            RunState::from_github("completed", Some("brand_new")),
            RunState::Other
        );
        assert_eq!(RunState::from_github("completed", None), RunState::Other);
        assert_eq!(RunState::from_github("", None), RunState::Other);
    }

    #[test]
    fn only_queued_and_running_are_active() {
        assert!(RunState::Queued.is_active());
        assert!(RunState::Running.is_active());
        assert!(!RunState::Passed.is_active());
        assert!(!RunState::Failed.is_active());
        assert!(!RunState::Other.is_active());
    }

    #[test]
    fn only_passed_and_failed_are_finished() {
        assert!(RunState::Passed.is_finished());
        assert!(RunState::Failed.is_finished());
        assert!(!RunState::Queued.is_finished());
        assert!(!RunState::Running.is_finished());
        assert!(!RunState::Other.is_finished());
    }
}
