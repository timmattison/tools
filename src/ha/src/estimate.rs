//! The time that a running job, or an active run, has left, from the jobs of
//! the same kind that passed.
//!
//! Two jobs are of the same kind when they have the same repository, the same
//! workflow, the same job name, and the same runner. The estimate of a kind is
//! the mean runner time of its newest passed jobs. A job with no such match has
//! no estimate.
//!
//! The estimate of an active run uses two more values. Both are keyed on the
//! repository, the workflow, and the job name. The first is the mean runner
//! time of the newest passed jobs on any runner. A job that waits uses it,
//! because a waiting job has no sure runner. The second is the tail of a job
//! name. The tail is the time from the end of a job to the end of its passed
//! run. The end of a run is the latest end of any of its jobs. The mean tail of
//! the newest passed runs is the tail of the name.
//!
//! Each job of the active run gives one term: the time from now to the end of
//! the run, if this job is the last one. The latest term is the estimate of the
//! run.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::job::{Job, JobId, RunnerName};
use crate::run::{span, Run, RunId, RunState};
use crate::target::RepoName;

/// How many of the newest matches an estimate reads.
pub const MATCHES_PER_ESTIMATE: usize = 5;

/// How the time of a running job, or of an active run, compares with its
/// estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remaining {
    /// The time that the job has left before it reaches its estimate. It is
    /// zero when the job is exactly at its estimate.
    Left(Duration),
    /// The least time that an active run has left. The latest end of the run
    /// comes from a job that waits for a runner, and the start of that job is
    /// unknown. Thus the run can take longer. Only a run row gets this value.
    AtLeast(Duration),
    /// The time that the job ran past its estimate. It is never zero.
    Over(Duration),
}

/// The kind of a job: repository, workflow, job name, and runner.
type Key<'a> = (&'a RepoName, &'a str, &'a str, &'a RunnerName);

/// The name of a job: repository, workflow, and job name.
type NameKey<'a> = (&'a RepoName, &'a str, &'a str);

/// One term of the estimate of an active run, in whole seconds from now.
struct Term {
    /// The end of the run if this job is the last one. A running job past its
    /// estimate ends now or later, thus its end is now plus the tail.
    end: i64,
    /// The end of the run if this job ended at its estimate. This value is
    /// different from `end` only for a running job past its estimate.
    unfloored: i64,
    /// True when the job waits for a runner.
    waits: bool,
}

/// What one job of an active run gives to the estimate of the run, when the
/// job has the estimates that it needs.
enum Counted {
    /// The job gives a term.
    Term(Term),
    /// The job gives no term, and it needs no estimate.
    NoTerm,
}

/// The estimates of one frame, built one time from the shown runs.
#[derive(Debug)]
pub struct Estimates<'a> {
    means: HashMap<Key<'a>, Duration>,
    any_runner: HashMap<NameKey<'a>, Duration>,
    tails: HashMap<NameKey<'a>, Duration>,
}

/// The mean of the newest [`MATCHES_PER_ESTIMATE`] of `matches`.
///
/// Each match is a time, an ID, and a number of seconds. The newest match has
/// the latest time. A tie goes to the higher ID. The mean drops the remainder.
/// The result is `None` for no matches.
fn mean_of_newest<I: Ord>(mut matches: Vec<(DateTime<Utc>, I, u64)>) -> Option<Duration> {
    matches.sort_unstable_by(|left, right| (&right.0, &right.1).cmp(&(&left.0, &left.1)));
    matches.truncate(MATCHES_PER_ESTIMATE);
    let count = u64::try_from(matches.len()).ok()?;
    let total = matches
        .iter()
        .fold(0_u64, |sum, entry| sum.saturating_add(entry.2));
    total.checked_div(count).map(Duration::from_secs)
}

/// Whole seconds as a signed number. A value that is too large saturates.
fn signed(seconds: u64) -> i64 {
    i64::try_from(seconds).unwrap_or(i64::MAX)
}

/// The signed whole seconds from `now` to `time`.
fn seconds_from(now: DateTime<Utc>, time: DateTime<Utc>) -> i64 {
    (time - now).num_seconds()
}

impl<'a> Estimates<'a> {
    /// Builds the index from each shown run and its last good jobs.
    ///
    /// A job is a match only when it passed, has a runner, has an end time, and
    /// has a runner time at `now`. Each kind keeps its newest
    /// [`MATCHES_PER_ESTIMATE`] matches by end time. A tie goes to the higher job
    /// ID. The mean of a kind is the sum of those runner times in whole seconds,
    /// divided by their count, with the remainder dropped.
    ///
    /// The same matches, with the runner ignored, give the any-runner duration
    /// of each job name. A run that passed gives one tail for each job name that
    /// passed in it. The end of that run is the latest end time of all of its
    /// jobs, in any state. The tail is the time from the latest end of the name
    /// to the end of the run. Each job name keeps the newest
    /// [`MATCHES_PER_ESTIMATE`] runs by end of run, and a tie goes to the higher
    /// run ID. A run that did not pass gives no tail.
    pub fn new(
        runs: impl IntoIterator<Item = (&'a Run, &'a [Job])>,
        now: DateTime<Utc>,
    ) -> Estimates<'a> {
        let mut groups: HashMap<Key<'a>, Vec<(DateTime<Utc>, JobId, u64)>> = HashMap::new();
        let mut any_groups: HashMap<NameKey<'a>, Vec<(DateTime<Utc>, JobId, u64)>> = HashMap::new();
        let mut tail_groups: HashMap<NameKey<'a>, Vec<(DateTime<Utc>, RunId, u64)>> =
            HashMap::new();
        for (run, jobs) in runs {
            for job in jobs {
                if job.state != RunState::Passed {
                    continue;
                }
                let (Some(runner), Some(completed), Some(time)) =
                    (job.runner.as_ref(), job.completed_at, job.runner_time(now))
                else {
                    continue;
                };
                let name = (&run.repo, run.workflow.as_str(), job.name.as_str());
                let entry = (completed, job.id, time.as_secs());
                groups
                    .entry((name.0, name.1, name.2, runner))
                    .or_default()
                    .push(entry);
                any_groups.entry(name).or_default().push(entry);
            }
            if run.state != RunState::Passed {
                continue;
            }
            let Some(end) = jobs.iter().filter_map(|job| job.completed_at).max() else {
                continue;
            };
            let mut latest: HashMap<&str, DateTime<Utc>> = HashMap::new();
            for job in jobs.iter().filter(|job| job.state == RunState::Passed) {
                if let Some(completed) = job.completed_at {
                    let slot = latest.entry(job.name.as_str()).or_insert(completed);
                    *slot = (*slot).max(completed);
                }
            }
            for (job_name, completed) in latest {
                tail_groups
                    .entry((&run.repo, run.workflow.as_str(), job_name))
                    .or_default()
                    .push((end, run.id, span(completed, end).as_secs()));
            }
        }
        Estimates {
            means: Self::means_of(groups),
            any_runner: Self::means_of(any_groups),
            tails: Self::means_of(tail_groups),
        }
    }

    /// The mean of the newest matches of each key.
    fn means_of<K: std::hash::Hash + Eq, I: Ord>(
        groups: HashMap<K, Vec<(DateTime<Utc>, I, u64)>>,
    ) -> HashMap<K, Duration> {
        groups
            .into_iter()
            .filter_map(|(key, matches)| Some((key, mean_of_newest(matches)?)))
            .collect()
    }

    /// The time left of `job` of `run` at `now`, or `None`.
    ///
    /// The result is `None` unless the job is running, has a runner, and has a
    /// start time. It is also `None` when no match gives a mean for the kind of
    /// the job. Else the result is [`Remaining::Left`] up to the mean, and
    /// [`Remaining::Over`] past it.
    #[must_use]
    pub fn remaining(&self, run: &Run, job: &Job, now: DateTime<Utc>) -> Option<Remaining> {
        if job.state != RunState::Running {
            return None;
        }
        let runner = job.runner.as_ref()?;
        let mean =
            *self
                .means
                .get(&(&run.repo, run.workflow.as_str(), job.name.as_str(), runner))?;
        let elapsed = job.runner_time(now)?;
        if elapsed <= mean {
            Some(Remaining::Left(mean.saturating_sub(elapsed)))
        } else {
            Some(Remaining::Over(elapsed.saturating_sub(mean)))
        }
    }

    /// The time until an active run finishes, at `now`, or `None`.
    ///
    /// `jobs` are the jobs of `run`. The result is `None` for a run that is not
    /// active, and for a run with no jobs. A job that passed, runs, or waits
    /// gives one term. A job in any other state gives no term. The result is
    /// `None` when no job gives a term, and when one job that gives a term has
    /// no estimate. Thus the run row shows no estimate when one of its running
    /// jobs shows a blank cell.
    ///
    /// The term of a job that passed is its end time plus the tail of its name.
    /// The term of a job that runs is its time left plus the tail. A job past
    /// its estimate gives only the tail, and the time over lowers its term
    /// without the floor. The term of a job that waits is the any-runner
    /// duration of its name plus the tail, because its start is unknown.
    ///
    /// The latest term is the result. It is [`Remaining::AtLeast`] when a job
    /// that waits gives the latest term, also in a tie. Else it is
    /// [`Remaining::Left`]. When no term is after now, the result is
    /// [`Remaining::Over`] by the greatest term without the floor. A run that is
    /// exactly at its estimate gives a left time of zero, never an over time of
    /// zero.
    #[must_use]
    pub fn run_remaining(&self, run: &Run, jobs: &[Job], now: DateTime<Utc>) -> Option<Remaining> {
        if !run.state.is_active() || jobs.is_empty() {
            return None;
        }
        let mut terms = Vec::new();
        for job in jobs {
            if let Counted::Term(term) = self.term(run, job, now)? {
                terms.push(term);
            }
        }
        let latest = terms.iter().map(|term| term.end).max()?;
        let from_queued = terms.iter().any(|term| term.end == latest && term.waits);
        let wrap = |seconds: i64| {
            let time = Duration::from_secs(u64::try_from(seconds).unwrap_or(0));
            if from_queued {
                Remaining::AtLeast(time)
            } else {
                Remaining::Left(time)
            }
        };
        if latest > 0 {
            return Some(wrap(latest));
        }
        let over = terms
            .iter()
            .map(|term| term.unfloored)
            .max()?
            .saturating_neg();
        if over > 0 {
            Some(Remaining::Over(Duration::from_secs(
                u64::try_from(over).unwrap_or(0),
            )))
        } else {
            Some(wrap(0))
        }
    }

    /// The term of one job of an active run, or `None` when the job needs an
    /// estimate and has none. Then the run has no estimate.
    fn term(&self, run: &Run, job: &Job, now: DateTime<Utc>) -> Option<Counted> {
        let name = (&run.repo, run.workflow.as_str(), job.name.as_str());
        let tail = self.tails.get(&name).map(|time| signed(time.as_secs()));
        let term = match job.state {
            RunState::Passed => {
                let completed = job.completed_at?;
                let end = seconds_from(now, completed).saturating_add(tail?);
                Term {
                    end,
                    unfloored: end,
                    waits: false,
                }
            }
            RunState::Running => {
                let tail = tail?;
                match self.remaining(run, job, now)? {
                    Remaining::Left(time) | Remaining::AtLeast(time) => {
                        let end = signed(time.as_secs()).saturating_add(tail);
                        Term {
                            end,
                            unfloored: end,
                            waits: false,
                        }
                    }
                    Remaining::Over(time) => Term {
                        end: tail,
                        unfloored: tail.saturating_sub(signed(time.as_secs())),
                        waits: false,
                    },
                }
            }
            RunState::Queued => {
                let duration = signed(self.any_runner.get(&name)?.as_secs());
                let end = duration.saturating_add(tail?);
                Term {
                    end,
                    unfloored: end,
                    waits: true,
                }
            }
            RunState::Failed | RunState::Cancelled | RunState::Skipped | RunState::Other => {
                return Some(Counted::NoTerm);
            }
        };
        Some(Counted::Term(term))
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

    fn runner(name: &str) -> RunnerName {
        RunnerName::new(name.to_owned()).expect("a runner name")
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
            runner: runner_name.map(runner),
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
        let mut history: Vec<Job> = (0_i64..5)
            .map(|index| {
                passed(
                    10 + index.cast_unsigned(),
                    "TypeScript",
                    RUNNER,
                    2000 + 100 * index,
                    300,
                )
            })
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

    /// A run of the given state.
    fn run_in(id: u64, state: RunState) -> Run {
        let mut made = run(id, REPO, "CI");
        made.state = state;
        made
    }

    /// Seconds after `at(0)` of a time `seconds` before `now()`.
    fn ago(seconds: i64) -> i64 {
        7200 - seconds
    }

    /// The jobs of one past run of the chain: build, test, and deploy.
    fn chain_jobs() -> Vec<Job> {
        vec![
            passed(1, "build", RUNNER, 300, 300),
            passed(2, "test", RUNNER, 900, 600),
            passed(3, "deploy", RUNNER, 1020, 120),
        ]
    }

    /// The estimate of a run of `state` with `jobs`, from `history`.
    fn run_remaining_in(
        state: RunState,
        history: &[(Run, Vec<Job>)],
        jobs: &[Job],
    ) -> Option<Remaining> {
        let current = run_in(99, state);
        let estimates = Estimates::new(
            history.iter().map(|(past, list)| (past, list.as_slice())),
            now(),
        );
        estimates.run_remaining(&current, jobs, now())
    }

    /// The estimate of a running run with `jobs`, from `history`.
    fn run_remaining_of(history: &[(Run, Vec<Job>)], jobs: &[Job]) -> Option<Remaining> {
        run_remaining_in(RunState::Running, history, jobs)
    }

    fn chain() -> Vec<(Run, Vec<Job>)> {
        vec![(run_in(1, RunState::Passed), chain_jobs())]
    }

    fn assert_text(value: Option<Remaining>, expected: Remaining, text: &str) {
        assert_eq!(value, Some(expected));
        assert_eq!(crate::render::format_remaining(expected), text);
    }

    #[test]
    fn the_chain_example_gives_the_time_until_the_run_finishes() {
        let history = chain();
        // Build runs 120 s. Its term is 180 s left plus a tail of 720 s.
        let jobs = [running(10, "build", Some(RUNNER), 120)];
        assert_text(
            run_remaining_of(&history, &jobs),
            Remaining::Left(secs(900)),
            "~15m",
        );
        // Build passed. Test waits. The queued term is 600 + 120 s.
        let jobs = [
            passed(10, "build", RUNNER, ago(30), 300),
            job(11, "test", RunState::Queued, None, None, None),
        ];
        assert_text(
            run_remaining_of(&history, &jobs),
            Remaining::AtLeast(secs(720)),
            ">12m",
        );
        // Test runs past its mean. The build term still gives the latest end.
        let jobs = [
            passed(10, "build", RUNNER, ago(780), 300),
            running(11, "test", Some(RUNNER), 780),
        ];
        assert_text(
            run_remaining_of(&history, &jobs),
            Remaining::Left(secs(120)),
            "~2m 00s",
        );
        // Deploy runs past its mean, and each earlier job is late too.
        let jobs = [
            passed(10, "build", RUNNER, ago(750), 300),
            passed(11, "test", RUNNER, ago(150), 600),
            running(12, "deploy", Some(RUNNER), 150),
        ];
        assert_text(
            run_remaining_of(&history, &jobs),
            Remaining::Over(secs(30)),
            "+30s",
        );
    }

    /// One past run of the parallel example: lint and test start together.
    fn parallel() -> Vec<(Run, Vec<Job>)> {
        vec![(
            run_in(1, RunState::Passed),
            vec![
                passed(1, "lint", RUNNER, 120, 120),
                passed(2, "test", RUNNER, 600, 600),
            ],
        )]
    }

    #[test]
    fn a_queued_run_gets_an_estimate() {
        // GitHub reports a run as queued while one of its jobs waits.
        let jobs = [
            passed(10, "build", RUNNER, ago(30), 300),
            job(11, "test", RunState::Queued, None, None, None),
        ];
        assert_text(
            run_remaining_in(RunState::Queued, &chain(), &jobs),
            Remaining::AtLeast(secs(720)),
            ">12m",
        );
    }

    #[test]
    fn the_parallel_example_gives_the_latest_of_the_two_terms() {
        let jobs = [
            running(10, "lint", Some(RUNNER), 60),
            running(11, "test", Some(RUNNER), 60),
        ];
        assert_text(
            run_remaining_of(&parallel(), &jobs),
            Remaining::Left(secs(540)),
            "~9m 00s",
        );
    }

    #[test]
    fn the_end_of_a_past_run_is_its_latest_job_end_not_its_update_time() {
        let mut history = chain();
        history[0].0.updated_at = at(1620);
        history[0].1.reverse();
        let jobs = [running(10, "deploy", Some(RUNNER), 60)];
        assert_text(
            run_remaining_of(&history, &jobs),
            Remaining::Left(secs(60)),
            "~1m 00s",
        );
    }

    #[test]
    fn a_past_run_that_failed_or_was_cancelled_gives_no_tail() {
        for state in [RunState::Failed, RunState::Cancelled] {
            let history = vec![(run_in(1, state), chain_jobs())];
            let jobs = [running(10, "build", Some(RUNNER), 120)];
            assert_eq!(run_remaining_of(&history, &jobs), None, "state {state:?}");
        }
    }

    /// A passed run whose build ended at `base` and whose run ended `tail`
    /// seconds later.
    fn tail_run(id: u64, base: i64, tail: i64) -> (Run, Vec<Job>) {
        (
            run_in(id, RunState::Passed),
            vec![
                passed(id * 10, "build", RUNNER, base, 300),
                passed(id * 10 + 1, "deploy", RUNNER, base + tail, 120),
            ],
        )
    }

    #[test]
    fn a_sixth_and_older_run_does_not_change_a_tail() {
        let mut history: Vec<(Run, Vec<Job>)> = (0_i64..5)
            .map(|index| tail_run(10 + index.cast_unsigned(), 20_000 + 2000 * index, 720))
            .collect();
        history.push(tail_run(1, 500, 7200));
        let jobs = [running(50, "build", Some(RUNNER), 120)];
        assert_eq!(
            run_remaining_of(&history, &jobs),
            Some(Remaining::Left(secs(900)))
        );
    }

    #[test]
    fn a_job_with_no_estimate_gives_no_estimate_for_the_run() {
        let mut history = chain();
        // A name with a mean on a runner, but no tail, because its run failed.
        history.push((
            run_in(2, RunState::Failed),
            vec![passed(20, "lint", RUNNER, 500, 100)],
        ));
        // A name with a tail, but no match on any runner, because it has no runner.
        history.push((
            run_in(3, RunState::Passed),
            vec![job(
                30,
                "docs",
                RunState::Passed,
                None,
                Some(300),
                Some(400),
            )],
        ));
        let build = running(10, "build", Some(RUNNER), 120);
        let cases: Vec<(&str, Vec<Job>)> = vec![
            ("no runner", vec![running(10, "build", None, 120)]),
            (
                "no match on the runner",
                vec![running(10, "build", Some("GitHub Actions 1000001050"), 120)],
            ),
            (
                "queued with no match",
                vec![job(10, "docs", RunState::Queued, None, None, None)],
            ),
            (
                "passed with no tail",
                vec![passed(11, "lint", RUNNER, ago(100), 100), build.clone()],
            ),
            (
                "running with no tail",
                vec![running(10, "lint", Some(RUNNER), 50)],
            ),
            (
                "queued with no tail",
                vec![job(10, "lint", RunState::Queued, None, None, None)],
            ),
            ("no jobs", Vec::new()),
        ];
        for (label, jobs) in cases {
            assert_eq!(run_remaining_of(&history, &jobs), None, "{label}");
        }
        assert_eq!(
            run_remaining_in(RunState::Passed, &history, std::slice::from_ref(&build)),
            None
        );
    }

    #[test]
    fn a_job_that_did_not_pass_adds_no_term_and_needs_no_estimate() {
        let history = chain();
        for state in [
            RunState::Failed,
            RunState::Cancelled,
            RunState::Skipped,
            RunState::Other,
        ] {
            let jobs = [
                running(10, "test", Some(RUNNER), 60),
                job(
                    11,
                    "build",
                    state,
                    Some(RUNNER),
                    Some(ago(300)),
                    Some(ago(0)),
                ),
                job(12, "no tail", state, None, None, None),
            ];
            assert_text(
                run_remaining_of(&history, &jobs),
                Remaining::Left(secs(660)),
                "~11m",
            );
        }
    }

    #[test]
    fn an_active_run_where_no_job_gives_a_term_has_no_estimate() {
        for state in [
            RunState::Failed,
            RunState::Cancelled,
            RunState::Skipped,
            RunState::Other,
        ] {
            let jobs = [
                job(
                    10,
                    "build",
                    state,
                    Some(RUNNER),
                    Some(ago(300)),
                    Some(ago(0)),
                ),
                job(11, "test", state, None, None, None),
            ];
            assert_eq!(run_remaining_of(&chain(), &jobs), None, "state {state:?}");
        }
    }

    #[test]
    fn a_queued_job_that_is_not_the_latest_term_does_not_give_at_least() {
        let jobs = [
            running(10, "test", Some(RUNNER), 60),
            job(11, "deploy", RunState::Queued, None, None, None),
        ];
        assert_text(
            run_remaining_of(&chain(), &jobs),
            Remaining::Left(secs(660)),
            "~11m",
        );
    }

    #[test]
    fn a_tie_goes_to_the_queued_job() {
        let jobs = [
            running(10, "lint", Some(RUNNER), 0),
            job(11, "test", RunState::Queued, None, None, None),
        ];
        assert_text(
            run_remaining_of(&parallel(), &jobs),
            Remaining::AtLeast(secs(600)),
            ">10m",
        );
    }

    #[test]
    fn a_run_exactly_at_its_estimate_gives_zero_left_and_never_zero_over() {
        let jobs = [
            passed(10, "build", RUNNER, ago(840), 300),
            passed(11, "test", RUNNER, ago(120), 600),
            running(12, "deploy", Some(RUNNER), 120),
        ];
        assert_text(
            run_remaining_of(&chain(), &jobs),
            Remaining::Left(Duration::ZERO),
            "~0s",
        );
    }
}
