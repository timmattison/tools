//! The time that a running job has left, from the jobs of the same kind that
//! passed on the same runner.
//!
//! Two jobs are of the same kind when they have the same repository, the same
//! workflow, the same job name, and the same runner. The estimate of a kind is
//! the mean runner time of its newest passed jobs. A job with no such match has
//! no estimate.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::job::{Job, RunnerName};
use crate::run::{Run, RunState};
use crate::target::RepoName;

/// How many of the newest matches an estimate reads.
pub const MATCHES_PER_ESTIMATE: usize = 5;

/// How the time of a running job compares with its estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remaining {
    /// The time that the job has left before it reaches its estimate. It is
    /// zero when the job is exactly at its estimate.
    Left(Duration),
    /// The time that the job ran past its estimate. It is never zero.
    Over(Duration),
}

/// The kind of a job: repository, workflow, job name, and runner.
type Key<'a> = (&'a RepoName, &'a str, &'a str, &'a RunnerName);

/// The estimates of one frame, built one time from the shown runs.
#[derive(Debug)]
pub struct Estimates<'a> {
    means: HashMap<Key<'a>, Duration>,
}

impl<'a> Estimates<'a> {
    /// Builds the index from each shown run and its last good jobs.
    pub fn new(
        runs: impl IntoIterator<Item = (&'a Run, &'a [Job])>,
        now: DateTime<Utc>,
    ) -> Estimates<'a> {
        let _ = (runs, now);
        Estimates {
            means: HashMap::new(),
        }
    }

    /// The time left of `job` of `run` at `now`, or `None`.
    pub fn remaining(&self, run: &Run, job: &Job, now: DateTime<Utc>) -> Option<Remaining> {
        let _ = (&self.means, run, job, now);
        None
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::job::JobId;
    use crate::run::{RunId, Visibility};

    const REPO: &str = "timmattison/megarepo";
    const RUNNER: &str = "m1-ts";

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 23, 12, 0, 0).unwrap()
    }

    /// A time `seconds` after 10:00:00 on the day of `now()`.
    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 23, 10, 0, 0).unwrap() + chrono::Duration::seconds(seconds)
    }

    fn runner(name: &str) -> Option<RunnerName> {
        Some(RunnerName::new(name.to_owned()).expect("a runner name"))
    }

    fn run(id: u64, repo: &str, workflow: &str) -> Run {
        Run {
            id: RunId(id),
            repo: RepoName::parse(repo).expect("a repository"),
            visibility: Visibility::Private,
            workflow: workflow.to_owned(),
            branch: "main".to_owned(),
            title: "a run".to_owned(),
            state: RunState::Running,
            started_at: at(0),
            created_at: at(0),
            updated_at: at(0),
            url: String::new(),
        }
    }

    fn job(
        id: u64,
        name: &str,
        state: RunState,
        runner_name: Option<&str>,
        started: Option<i64>,
        completed: Option<i64>,
    ) -> Job {
        Job {
            id: JobId(id),
            name: name.to_owned(),
            state,
            runner: runner_name.and_then(runner),
            labels: Vec::new(),
            created_at: Some(at(0)),
            started_at: started.map(at),
            completed_at: completed.map(at),
            url: String::new(),
        }
    }

    /// A passed job that ran `length` seconds and ended at `completed`.
    fn passed(id: u64, name: &str, runner_name: &str, completed: i64, length: i64) -> Job {
        job(
            id,
            name,
            RunState::Passed,
            Some(runner_name),
            Some(completed - length),
            Some(completed),
        )
    }

    /// A running job that started `elapsed` seconds before `now()`.
    fn running(id: u64, name: &str, runner_name: Option<&str>, elapsed: i64) -> Job {
        let mut running = job(id, name, RunState::Running, runner_name, None, None);
        running.started_at = Some(now() - chrono::Duration::seconds(elapsed));
        running
    }

    fn remaining_of(history: &[Job], current: &Run, running_job: &Job) -> Option<Remaining> {
        let past = run(1, REPO, "CI");
        let estimates = Estimates::new([(&past, history)], now());
        estimates.remaining(current, running_job, now())
    }

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    #[test]
    fn the_issue_example_gives_the_time_left_from_the_mean_of_three_matches() {
        let history = [
            passed(1, "TypeScript", RUNNER, 1000, 290),
            passed(2, "TypeScript", RUNNER, 2000, 310),
            passed(3, "TypeScript", RUNNER, 3000, 300),
        ];
        let current = run(9, REPO, "CI");
        let live = running(10, "TypeScript", Some(RUNNER), 118);
        assert_eq!(
            remaining_of(&history, &current, &live),
            Some(Remaining::Left(secs(182)))
        );
    }

    #[test]
    fn a_passed_job_on_a_different_runner_is_not_a_match() {
        let history = [passed(1, "TypeScript", "mbp-ts", 1000, 300)];
        let live = running(10, "TypeScript", Some(RUNNER), 60);
        assert_eq!(remaining_of(&history, &run(9, REPO, "CI"), &live), None);
    }

    #[test]
    fn a_different_job_name_is_not_a_match() {
        let history = [passed(1, "Rust", RUNNER, 1000, 300)];
        let live = running(10, "TypeScript", Some(RUNNER), 60);
        assert_eq!(remaining_of(&history, &run(9, REPO, "CI"), &live), None);
    }

    #[test]
    fn a_matrix_name_matches_only_the_same_text() {
        let history = [passed(1, "test (ubuntu, 18)", RUNNER, 1000, 300)];
        let live = running(10, "test (ubuntu, 20)", Some(RUNNER), 60);
        assert_eq!(remaining_of(&history, &run(9, REPO, "CI"), &live), None);
        let same = running(11, "test (ubuntu, 18)", Some(RUNNER), 60);
        assert_eq!(
            remaining_of(&history, &run(9, REPO, "CI"), &same),
            Some(Remaining::Left(secs(240)))
        );
    }

    #[test]
    fn a_different_workflow_is_not_a_match() {
        let history = [passed(1, "TypeScript", RUNNER, 1000, 300)];
        let live = running(10, "TypeScript", Some(RUNNER), 60);
        assert_eq!(remaining_of(&history, &run(9, REPO, "Deploy"), &live), None);
    }

    #[test]
    fn a_different_repository_is_not_a_match() {
        let history = [passed(1, "TypeScript", RUNNER, 1000, 300)];
        let live = running(10, "TypeScript", Some(RUNNER), 60);
        assert_eq!(
            remaining_of(&history, &run(9, "timmattison/tools", "CI"), &live),
            None
        );
    }

    #[test]
    fn a_job_that_did_not_pass_is_not_a_match() {
        let live = running(10, "TypeScript", Some(RUNNER), 60);
        for state in [
            RunState::Failed,
            RunState::Cancelled,
            RunState::Skipped,
            RunState::Other,
        ] {
            let history = [job(
                1,
                "TypeScript",
                state,
                Some(RUNNER),
                Some(700),
                Some(1000),
            )];
            assert_eq!(
                remaining_of(&history, &run(9, REPO, "CI"), &live),
                None,
                "state {state:?}"
            );
        }
    }

    #[test]
    fn a_sixth_and_older_match_does_not_change_the_mean() {
        let mut history: Vec<Job> = (0..5)
            .map(|index| passed(10 + index, "TypeScript", RUNNER, 2000 + 100 * index as i64, 300))
            .collect();
        history.push(passed(1, "TypeScript", RUNNER, 1000, 900));
        let live = running(50, "TypeScript", Some(RUNNER), 100);
        assert_eq!(
            remaining_of(&history, &run(9, REPO, "CI"), &live),
            Some(Remaining::Left(secs(200)))
        );
    }

    #[test]
    fn the_newest_five_by_end_time_win_whatever_the_input_order() {
        // Five newer matches of 300 s and one older of 900 s, in a mixed order.
        let history = [
            passed(3, "TypeScript", RUNNER, 3000, 300),
            passed(1, "TypeScript", RUNNER, 1000, 900),
            passed(5, "TypeScript", RUNNER, 5000, 300),
            passed(2, "TypeScript", RUNNER, 2000, 300),
            passed(6, "TypeScript", RUNNER, 6000, 300),
            passed(4, "TypeScript", RUNNER, 4000, 300),
        ];
        let live = running(50, "TypeScript", Some(RUNNER), 100);
        assert_eq!(
            remaining_of(&history, &run(9, REPO, "CI"), &live),
            Some(Remaining::Left(secs(200)))
        );
        // A newer match of 600 s pushes the oldest of the five out.
        // Kept: 600, 300, 300, 300, 300 gives a mean of 360.
        let mut more = history.to_vec();
        more.push(passed(7, "TypeScript", RUNNER, 7000, 600));
        assert_eq!(
            remaining_of(&more, &run(9, REPO, "CI"), &live),
            Some(Remaining::Left(secs(260)))
        );
    }

    #[test]
    fn a_job_past_its_estimate_gives_over_and_a_job_at_its_estimate_gives_zero_left() {
        let history = [passed(1, "TypeScript", RUNNER, 1000, 300)];
        let current = run(9, REPO, "CI");
        let past = running(10, "TypeScript", Some(RUNNER), 345);
        assert_eq!(
            remaining_of(&history, &current, &past),
            Some(Remaining::Over(secs(45)))
        );
        let exact = running(11, "TypeScript", Some(RUNNER), 300);
        assert_eq!(
            remaining_of(&history, &current, &exact),
            Some(Remaining::Left(Duration::ZERO))
        );
    }

    #[test]
    fn a_running_job_with_no_runner_has_no_estimate() {
        let history = [passed(1, "TypeScript", RUNNER, 1000, 300)];
        let live = running(10, "TypeScript", None, 60);
        assert_eq!(remaining_of(&history, &run(9, REPO, "CI"), &live), None);
    }

    #[test]
    fn a_job_that_does_not_run_has_no_estimate() {
        let history = [passed(1, "TypeScript", RUNNER, 1000, 300)];
        let current = run(9, REPO, "CI");
        let queued = job(10, "TypeScript", RunState::Queued, Some(RUNNER), None, None);
        assert_eq!(remaining_of(&history, &current, &queued), None);
        let done = passed(11, "TypeScript", RUNNER, 1500, 200);
        assert_eq!(remaining_of(&history, &current, &done), None);
    }

    #[test]
    fn a_running_job_with_no_match_has_no_estimate() {
        let live = running(10, "TypeScript", Some(RUNNER), 60);
        assert_eq!(remaining_of(&[], &run(9, REPO, "CI"), &live), None);
    }

    #[test]
    fn the_mean_drops_the_remainder() {
        let history = [
            passed(1, "TypeScript", RUNNER, 1000, 300),
            passed(2, "TypeScript", RUNNER, 2000, 301),
        ];
        let live = running(10, "TypeScript", Some(RUNNER), 100);
        assert_eq!(
            remaining_of(&history, &run(9, REPO, "CI"), &live),
            Some(Remaining::Left(secs(200)))
        );
    }
}
