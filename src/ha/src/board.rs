//! The board: the runs of all watched repositories, as rows for one screen.
//!
//! The board holds the last good runs of each repository, the last error of
//! each repository, the time window, the selection, and the scroll offset.
//! It does no I/O. Every reader takes the current time as an argument, so a
//! test uses one fixed time and never reads the clock.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::job::Job;
use crate::run::{keep_first_of_each_id, Run, RunId, RunState};
use crate::target::RepoName;

/// One of the two lists on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Runs that wait or run.
    Active,
    /// Runs that finished inside the window.
    Recent,
}

/// One row of the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row<'a> {
    /// The title row of a section.
    Header {
        /// The section that the header opens.
        section: Section,
        /// The number of runs in the section.
        count: usize,
        /// The number of passed runs. This is 0 for the active section.
        passed: usize,
        /// The number of failed runs. This is 0 for the active section.
        failed: usize,
    },
    /// The row that shows an empty section.
    Empty(Section),
    /// One run.
    Run {
        /// The run.
        run: &'a Run,
        /// True for the one row that the selection marks.
        selected: bool,
    },
    /// One job of an expanded run.
    Job {
        /// The job.
        job: &'a Job,
        /// True for the last row of the block of its run. The render draws
        /// `└` for it, and `├` for any other row.
        last: bool,
    },
    /// The row of an expanded run before the first answer: "Loading jobs…".
    JobsLoading,
    /// The row of an expanded run whose answer holds no job: "No jobs yet".
    NoJobs,
    /// The error of the last failed fetch of the jobs of an expanded run.
    JobsError(&'a str),
}

/// The jobs of one expanded run.
#[derive(Debug)]
enum Jobs {
    /// The run is expanded and no answer came yet.
    Loading,
    /// The last good answer.
    Ready(Vec<Job>),
    /// The last fetch failed.
    Failed {
        /// The last good jobs. The list is empty when there is none.
        jobs: Vec<Job>,
        /// The error of the last fetch.
        error: String,
    },
}

/// The rows that fit on the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View<'a> {
    /// The visible slice of the rows.
    pub rows: Vec<Row<'a>>,
    /// The index of the first visible row in the full list of rows.
    pub first: usize,
}

/// The state of the board.
#[derive(Debug)]
pub struct Board {
    window: Duration,
    runs: BTreeMap<RepoName, Vec<Run>>,
    errors: BTreeMap<RepoName, String>,
    selected: Option<(RunId, usize)>,
    offset: usize,
    expanded: BTreeMap<RunId, Jobs>,
}

impl Board {
    /// Makes an empty board. A finished run shows while it ended inside `window`.
    #[must_use]
    pub fn new(window: Duration) -> Board {
        Board {
            window,
            runs: BTreeMap::new(),
            errors: BTreeMap::new(),
            selected: None,
            offset: 0,
            expanded: BTreeMap::new(),
        }
    }

    /// The time window of the finished runs.
    #[must_use]
    pub fn window(&self) -> Duration {
        self.window
    }

    /// Stores the runs of one repository and clears the error of that repository.
    /// A repeated run ID keeps its first run only.
    pub fn replace(&mut self, repo: RepoName, mut runs: Vec<Run>) {
        keep_first_of_each_id(&mut runs);
        self.errors.remove(&repo);
        self.runs.insert(repo, runs);
    }

    /// Records an error for one repository. The last good runs stay.
    pub fn fail(&mut self, repo: RepoName, error: String) {
        self.errors.insert(repo, error);
    }

    /// Drops the runs and the errors of every repository that is not in `repos`.
    pub fn keep_only(&mut self, repos: &BTreeSet<RepoName>) {
        self.runs.retain(|repo, _| repos.contains(repo));
        self.errors.retain(|repo, _| repos.contains(repo));
    }

    /// The error of each failed repository, sorted by repository.
    #[must_use]
    pub fn errors(&self) -> Vec<(&RepoName, &str)> {
        self.errors
            .iter()
            .map(|(repo, error)| (repo, error.as_str()))
            .collect()
    }

    /// All rows of the board at time `now`.
    pub fn rows(&mut self, now: DateTime<Utc>) -> Vec<Row<'_>> {
        self.settle(now);
        self.build_rows(now)
    }

    /// The rows that fit in `height` lines, with the selected row inside.
    pub fn view(&mut self, now: DateTime<Utc>, height: usize) -> View<'_> {
        self.settle(now);
        if height > 0 {
            let rows = self.build_rows(now);
            let selected_row = rows
                .iter()
                .position(|row| matches!(row, Row::Run { selected: true, .. }));
            let header_above = selected_row.is_some_and(|row| {
                row > 0 && matches!(rows.get(row - 1), Some(Row::Header { .. }))
            });
            let len = rows.len();
            let below = selected_row.map_or(0, |row| {
                rows[row + 1..]
                    .iter()
                    .take_while(|next| {
                        !matches!(next, Row::Header { .. } | Row::Empty(_) | Row::Run { .. })
                    })
                    .count()
            });
            self.offset = scroll(self.offset, height, len, selected_row, header_above, below);
        }
        let rows = self
            .build_rows(now)
            .into_iter()
            .skip(self.offset)
            .take(height)
            .collect();
        View {
            rows,
            first: self.offset,
        }
    }

    /// Splits the shown runs into the active list and the recent list.
    ///
    /// Both lists are sorted. A run that is not shown at `now` is in neither.
    fn sections(&self, now: DateTime<Utc>) -> (Vec<&Run>, Vec<&Run>) {
        let mut active: Vec<&Run> = self
            .runs
            .values()
            .flatten()
            .filter(|run| run.state.is_active())
            .collect();
        active.sort_by_key(|run| (run.started_at, run.id));
        let oldest = chrono::Duration::from_std(self.window)
            .ok()
            .and_then(|window| now.checked_sub_signed(window))
            .unwrap_or(DateTime::<Utc>::MIN_UTC);
        let mut recent: Vec<&Run> = self
            .runs
            .values()
            .flatten()
            .filter(|run| run.state.is_finished() && run.updated_at >= oldest)
            .collect();
        recent.sort_by_key(|run| std::cmp::Reverse((run.updated_at, run.id)));
        (active, recent)
    }

    /// All runs that show at `now`, in the order of the board: active, then recent.
    fn selectable(&self, now: DateTime<Utc>) -> Vec<&Run> {
        let (active, recent) = self.sections(now);
        active.into_iter().chain(recent).collect()
    }

    /// Makes the selection valid for the runs that show at `now`.
    ///
    /// The selection follows its run. When the run is gone, the run at the old
    /// position is selected, or the last run when the old position is past the
    /// end. A board with no run has no selection. A new selection is the first run.
    fn settle(&mut self, now: DateTime<Utc>) {
        let runs = self.selectable(now);
        let position = match self.selected {
            None => 0,
            Some((id, old)) => runs
                .iter()
                .position(|run| run.id == id)
                .unwrap_or_else(|| old.min(runs.len().saturating_sub(1))),
        };
        let selected = runs.get(position).map(|run| (run.id, position));
        let shown: BTreeSet<RunId> = runs.iter().map(|run| run.id).collect();
        self.selected = selected;
        self.expanded.retain(|id, _| shown.contains(id));
    }

    /// Builds the full list of rows. The list has two sections.
    ///
    /// The row of the selected run carries the selection mark. The block of
    /// each expanded run follows the row of that run.
    fn build_rows(&self, now: DateTime<Utc>) -> Vec<Row<'_>> {
        let (active, recent) = self.sections(now);
        let passed = recent
            .iter()
            .filter(|run| run.state == RunState::Passed)
            .count();
        let mut rows = vec![Row::Header {
            section: Section::Active,
            count: active.len(),
            passed: 0,
            failed: 0,
        }];
        let selected = self.selected.map(|(_, index)| index);
        push_runs(
            &mut rows,
            &active,
            Section::Active,
            0,
            selected,
            &self.expanded,
        );
        rows.push(Row::Header {
            section: Section::Recent,
            count: recent.len(),
            passed,
            failed: recent.len() - passed,
        });
        push_runs(
            &mut rows,
            &recent,
            Section::Recent,
            active.len(),
            selected,
            &self.expanded,
        );
        rows
    }

    /// Expands the selected run. The run shows a loading row until `set_jobs`
    /// gives its jobs. A run that is already expanded keeps its jobs. With no
    /// selection, nothing happens.
    pub fn expand(&mut self, now: DateTime<Utc>) {
        self.settle(now);
        if let Some((id, _)) = self.selected {
            self.expanded.entry(id).or_insert(Jobs::Loading);
        }
    }

    /// Collapses the selected run and drops its jobs. A run that is not
    /// expanded, or no selection, changes nothing.
    pub fn collapse(&mut self, now: DateTime<Utc>) {
        self.settle(now);
        if let Some((id, _)) = self.selected {
            self.expanded.remove(&id);
        }
    }

    /// Stores the jobs of `run` when the run is expanded. A late answer for a
    /// run that is not expanded is dropped.
    pub fn set_jobs(&mut self, run: RunId, jobs: Vec<Job>) {
        if let Some(state) = self.expanded.get_mut(&run) {
            *state = Jobs::Ready(jobs);
        }
    }

    /// Records the error of a failed fetch for `run` when the run is expanded.
    /// The last good jobs stay. Nothing happens for a run that is not expanded.
    pub fn fail_jobs(&mut self, run: RunId, error: String) {
        if let Some(state) = self.expanded.get_mut(&run) {
            let jobs = match std::mem::replace(state, Jobs::Loading) {
                Jobs::Loading => Vec::new(),
                Jobs::Ready(jobs) | Jobs::Failed { jobs, .. } => jobs,
            };
            *state = Jobs::Failed { jobs, error };
        }
    }

    /// The repository and the ID of each expanded run that shows at `now`.
    /// The app sends this set to the worker each time it changes.
    pub fn expanded(&mut self, now: DateTime<Utc>) -> BTreeSet<(RepoName, RunId)> {
        self.settle(now);
        self.selectable(now)
            .into_iter()
            .filter(|run| self.expanded.contains_key(&run.id))
            .map(|run| (run.repo.clone(), run.id))
            .collect()
    }

    /// Moves the selection to the next run.
    pub fn select_next(&mut self, now: DateTime<Utc>) {
        self.settle(now);
        self.move_selection(now, |index, count| (index + 1).min(count - 1));
    }

    /// Moves the selection to the previous run.
    pub fn select_previous(&mut self, now: DateTime<Utc>) {
        self.settle(now);
        self.move_selection(now, |index, _| index.saturating_sub(1));
    }

    /// Moves the selection to the run at the position that `target` gives.
    ///
    /// `target` gets the current position and the number of runs.
    fn move_selection(&mut self, now: DateTime<Utc>, target: impl Fn(usize, usize) -> usize) {
        let runs = self.selectable(now);
        let moved = self.selected.and_then(|(_, index)| {
            let position = target(index, runs.len());
            runs.get(position).map(|run| (run.id, position))
        });
        self.selected = moved;
    }

    /// The address of the selected run, or `None` when the board has no run.
    pub fn selected_url(&mut self, now: DateTime<Utc>) -> Option<String> {
        self.settle(now);
        let (id, _) = self.selected?;
        self.selectable(now)
            .into_iter()
            .find(|run| run.id == id)
            .map(|run| run.url.clone())
    }
}

/// Finds the scroll offset for a view of `height` rows over `len` rows.
///
/// The offset changes as little as possible to put `selected_row` inside the
/// view. With a `header_above` the selected row and also that header stay
/// inside, when the view has two rows or more. Then the offset moves as far as
/// it can to show the `below` rows of the block of the selected run, and the
/// top row stays inside. The offset never goes past the last full view.
/// `height` is at least 1.
fn scroll(
    offset: usize,
    height: usize,
    len: usize,
    selected_row: Option<usize>,
    header_above: bool,
    below: usize,
) -> usize {
    let mut offset = offset;
    if let Some(row) = selected_row {
        let top = if header_above && height >= 2 {
            row - 1
        } else {
            row
        };
        offset = offset.min(top);
        if row >= offset + height {
            offset = row + 1 - height;
        }
        if row + below >= offset + height {
            offset = (row + below + 1 - height).min(top);
        }
    }
    offset.min(len.saturating_sub(height))
}

/// Adds the runs of one section to `rows`, or the empty row when there is none.
///
/// `first` is the position of the first run of `runs` among all runs. The run
/// at position `selected` gets the mark. The block of jobs of each run in
/// `expanded` follows the row of that run.
fn push_runs<'a>(
    rows: &mut Vec<Row<'a>>,
    runs: &[&'a Run],
    section: Section,
    first: usize,
    selected: Option<usize>,
    expanded: &'a BTreeMap<RunId, Jobs>,
) {
    if runs.is_empty() {
        rows.push(Row::Empty(section));
    }
    for (offset, &run) in runs.iter().enumerate() {
        rows.push(Row::Run {
            run,
            selected: selected == Some(first + offset),
        });
        if let Some(jobs) = expanded.get(&run.id) {
            push_jobs(rows, jobs);
        }
    }
}

/// Adds the block of rows of one expanded run to `rows`.
fn push_jobs<'a>(rows: &mut Vec<Row<'a>>, jobs: &'a Jobs) {
    match jobs {
        Jobs::Loading => rows.push(Row::JobsLoading),
        Jobs::Ready(list) if list.is_empty() => rows.push(Row::NoJobs),
        Jobs::Ready(list) => rows.extend(list.iter().enumerate().map(|(index, job)| Row::Job {
            job,
            last: index + 1 == list.len(),
        })),
        Jobs::Failed { jobs, error } => {
            rows.extend(jobs.iter().map(|job| Row::Job { job, last: false }));
            rows.push(Row::JobsError(error));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::JobId;
    use crate::run::RunNumber;
    use chrono::{Duration as Span, TimeZone};

    const HOUR: u64 = 3600;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0)
            .single()
            .expect("a valid time")
    }

    fn repo(text: &str) -> RepoName {
        RepoName::parse(text).expect("a valid repository")
    }

    /// Builds a run. The times are minutes before `now()`.
    fn run(id: u64, repo_text: &str, state: RunState, started: i64, updated: i64) -> Run {
        Run {
            id: RunId(id),
            number: RunNumber(id + 100),
            repo: repo(repo_text),
            workflow: "ci".to_owned(),
            branch: "main".to_owned(),
            title: format!("run {id}"),
            state,
            started_at: now() - Span::minutes(started),
            updated_at: now() - Span::minutes(updated),
            url: format!("https://example.test/{repo_text}/runs/{id}"),
        }
    }

    /// Builds a job.
    fn job(id: u64, state: RunState) -> Job {
        Job {
            id: JobId(id),
            name: format!("job {id}"),
            state,
            started_at: None,
            completed_at: None,
            url: String::new(),
        }
    }

    /// Builds a running job with the name `name`.
    fn named_job(id: u64, name: &str) -> Job {
        Job {
            name: name.to_owned(),
            ..job(id, RunState::Running)
        }
    }

    fn board() -> Board {
        Board::new(Duration::from_secs(8 * HOUR))
    }

    /// Writes each row as short text: `Header(Active,2,0,0)`, `Empty(Recent)`,
    /// `Run(7)`, `Job(11)`, `LastJob(12)`, `JobsLoading`, `NoJobs`, or
    /// `JobsError(text)`. The selection mark is left out, see `marked`.
    fn shape(rows: &[Row<'_>]) -> Vec<String> {
        marked(rows)
            .into_iter()
            .map(|row| row.trim_end_matches('*').to_owned())
            .collect()
    }

    /// Like `shape`, and a selected run reads `Run(7)*`.
    fn marked(rows: &[Row<'_>]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Header {
                    section,
                    count,
                    passed,
                    failed,
                } => format!("Header({section:?},{count},{passed},{failed})"),
                Row::Empty(section) => format!("Empty({section:?})"),
                Row::Run { run, selected } => {
                    format!("Run({}){}", run.id.0, if *selected { "*" } else { "" })
                }
                Row::Job { job, last } => {
                    format!("{}({})", if *last { "LastJob" } else { "Job" }, job.id.0)
                }
                Row::JobsLoading => "JobsLoading".to_owned(),
                Row::NoJobs => "NoJobs".to_owned(),
                Row::JobsError(error) => format!("JobsError({error})"),
            })
            .collect()
    }

    #[test]
    fn an_empty_board_shows_two_empty_sections() {
        let mut board = board();
        assert_eq!(
            shape(&board.rows(now())),
            [
                "Header(Active,0,0,0)",
                "Empty(Active)",
                "Header(Recent,0,0,0)",
                "Empty(Recent)"
            ]
        );
    }

    #[test]
    fn the_sections_hold_their_runs_and_the_headers_count_them() {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Running, 5, 1),
                run(3, "a/x", RunState::Failed, 40, 10),
                run(4, "a/x", RunState::Passed, 50, 15),
                run(2, "a/x", RunState::Passed, 30, 20),
            ],
        );
        let rows = board.rows(now());
        assert_eq!(
            shape(&rows),
            [
                "Header(Active,1,0,0)",
                "Run(1)",
                "Header(Recent,3,2,1)",
                "Run(3)",
                "Run(4)",
                "Run(2)",
            ]
        );
    }

    #[test]
    fn an_empty_section_shows_an_empty_row_after_its_header() {
        let mut only_active = board();
        only_active.replace(repo("a/x"), vec![run(1, "a/x", RunState::Running, 5, 1)]);
        let rows = only_active.rows(now());
        assert_eq!(shape(&rows)[2..], ["Header(Recent,0,0,0)", "Empty(Recent)"]);

        let mut only_recent = board();
        only_recent.replace(repo("a/x"), vec![run(2, "a/x", RunState::Passed, 30, 20)]);
        let rows = only_recent.rows(now());
        assert_eq!(shape(&rows)[..2], ["Header(Active,0,0,0)", "Empty(Active)"]);
    }

    #[test]
    fn queued_and_running_runs_are_active_and_other_runs_never_show() {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Queued, 20, 20),
                run(2, "a/x", RunState::Running, 10, 5),
                run(3, "a/x", RunState::Other, 30, 25),
                run(4, "a/x", RunState::Passed, 40, 30),
            ],
        );
        assert_eq!(
            shape(&board.rows(now())),
            [
                "Header(Active,2,0,0)",
                "Run(1)",
                "Run(2)",
                "Header(Recent,1,1,0)",
                "Run(4)",
            ]
        );
    }

    #[test]
    fn active_runs_are_ordered_oldest_first_and_ties_by_id() {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                run(9, "a/x", RunState::Running, 5, 1),
                run(8, "a/x", RunState::Queued, 30, 1),
                run(7, "a/x", RunState::Running, 10, 1),
                run(5, "a/x", RunState::Running, 10, 1),
            ],
        );
        let rows = board.rows(now());
        assert_eq!(shape(&rows)[1..5], ["Run(8)", "Run(5)", "Run(7)", "Run(9)"]);
    }

    #[test]
    fn recent_runs_are_ordered_newest_first_and_ties_by_id_descending() {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Passed, 100, 50),
                run(2, "a/x", RunState::Failed, 100, 10),
                run(3, "a/x", RunState::Passed, 100, 30),
                run(4, "a/x", RunState::Passed, 100, 30),
            ],
        );
        let rows = board.rows(now());
        assert_eq!(shape(&rows)[3..], ["Run(2)", "Run(4)", "Run(3)", "Run(1)"]);
    }

    /// Makes a run that finished `seconds` before `now()`.
    fn finished_run(id: u64, state: RunState, started_secs: i64, finished_secs: i64) -> Run {
        let mut made = run(id, "a/x", state, 0, 0);
        made.started_at = now() - Span::seconds(started_secs);
        made.updated_at = now() - Span::seconds(finished_secs);
        made
    }

    #[test]
    fn a_finished_run_shows_while_it_ended_inside_the_window() {
        let window = 8 * 3600;
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                finished_run(1, RunState::Passed, window + 100, window - 1),
                finished_run(2, RunState::Passed, window + 100, window + 1),
                finished_run(3, RunState::Failed, window + 100, window),
            ],
        );
        let rows = board.rows(now());
        assert_eq!(
            shape(&rows),
            [
                "Header(Active,0,0,0)",
                "Empty(Active)",
                "Header(Recent,2,1,1)",
                "Run(1)",
                "Run(3)",
            ]
        );
    }

    #[test]
    fn a_run_that_started_before_the_window_and_ended_inside_it_shows() {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![finished_run(1, RunState::Passed, 9 * 3600, 3600)],
        );
        assert_eq!(shape(&board.rows(now()))[3..], ["Run(1)"]);
    }

    #[test]
    fn active_runs_show_whatever_the_window_is() {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                finished_run(1, RunState::Running, 3 * 24 * 3600, 3 * 24 * 3600),
                finished_run(2, RunState::Queued, 20 * 3600, 20 * 3600),
            ],
        );
        assert_eq!(
            shape(&board.rows(now()))[..4],
            [
                "Header(Active,2,0,0)",
                "Run(1)",
                "Run(2)",
                "Header(Recent,0,0,0)",
            ]
        );
    }

    #[test]
    fn replace_changes_only_the_runs_of_its_repository() {
        let mut board = board();
        board.replace(repo("a/x"), vec![run(1, "a/x", RunState::Running, 5, 1)]);
        board.replace(repo("b/y"), vec![run(2, "b/y", RunState::Running, 4, 1)]);
        assert_eq!(shape(&board.rows(now()))[1..3], ["Run(1)", "Run(2)"]);

        board.replace(
            repo("a/x"),
            vec![
                run(3, "a/x", RunState::Running, 6, 1),
                run(4, "a/x", RunState::Passed, 9, 8),
            ],
        );
        let rows = board.rows(now());
        let shown = shape(&rows);
        assert_eq!(shown[1], "Run(3)");
        assert_eq!(shown[2], "Run(2)");
        assert_eq!(shown[3..], ["Header(Recent,1,1,0)", "Run(4)"]);
        assert_eq!(shown.len(), 5);
    }

    #[test]
    fn a_failed_repository_keeps_its_last_good_runs_and_lists_its_error() {
        let mut board = board();
        board.replace(repo("a/x"), vec![run(1, "a/x", RunState::Running, 5, 1)]);
        board.fail(repo("b/y"), "HTTP 404".to_owned());
        board.fail(repo("a/x"), "timed out".to_owned());
        board.fail(repo("a/w"), "HTTP 500".to_owned());

        assert_eq!(shape(&board.rows(now()))[1], "Run(1)");
        let errors = board.errors();
        let listed: Vec<(String, &str)> = errors
            .iter()
            .map(|(repo, error)| (repo.to_string(), *error))
            .collect();
        assert_eq!(
            listed,
            [
                ("a/w".to_owned(), "HTTP 500"),
                ("a/x".to_owned(), "timed out"),
                ("b/y".to_owned(), "HTTP 404"),
            ]
        );
    }

    #[test]
    fn a_later_replace_clears_the_error_of_its_repository() {
        let mut board = board();
        board.fail(repo("a/x"), "timed out".to_owned());
        board.fail(repo("b/y"), "HTTP 404".to_owned());
        board.replace(repo("a/x"), Vec::new());
        let errors = board.errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0.to_string(), "b/y");
    }

    #[test]
    fn a_repository_that_never_had_runs_can_fail_and_shows_no_run() {
        let mut board = board();
        board.fail(repo("a/x"), "HTTP 404".to_owned());
        assert_eq!(
            shape(&board.rows(now())),
            [
                "Header(Active,0,0,0)",
                "Empty(Active)",
                "Header(Recent,0,0,0)",
                "Empty(Recent)"
            ]
        );
        assert_eq!(board.errors().len(), 1);
    }

    #[test]
    fn keep_only_drops_the_runs_and_errors_of_other_repositories() {
        let mut board = board();
        board.replace(repo("a/x"), vec![run(1, "a/x", RunState::Running, 5, 1)]);
        board.replace(repo("b/y"), vec![run(2, "b/y", RunState::Running, 4, 1)]);
        board.fail(repo("b/y"), "timed out".to_owned());
        board.fail(repo("c/z"), "HTTP 404".to_owned());
        board.fail(repo("a/x"), "HTTP 500".to_owned());

        let keep: BTreeSet<RepoName> = [repo("A/X")].into_iter().collect();
        board.keep_only(&keep);

        assert_eq!(
            shape(&board.rows(now()))[..3],
            ["Header(Active,1,0,0)", "Run(1)", "Header(Recent,0,0,0)"]
        );
        let errors = board.errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].1, "HTTP 500");
    }

    #[test]
    fn the_first_run_is_selected_at_the_start() {
        let mut with_active = board();
        with_active.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Running, 5, 1),
                run(2, "a/x", RunState::Passed, 30, 20),
            ],
        );
        assert_eq!(
            marked(&with_active.rows(now())),
            [
                "Header(Active,1,0,0)",
                "Run(1)*",
                "Header(Recent,1,1,0)",
                "Run(2)"
            ]
        );

        let mut only_recent = board();
        only_recent.replace(
            repo("a/x"),
            vec![
                run(3, "a/x", RunState::Passed, 30, 10),
                run(2, "a/x", RunState::Failed, 30, 20),
            ],
        );
        assert_eq!(
            marked(&only_recent.rows(now()))[2..],
            ["Header(Recent,2,1,1)", "Run(3)*", "Run(2)"]
        );
    }

    #[test]
    fn an_empty_board_has_no_selection() {
        let mut board = board();
        let rows = board.rows(now());
        assert!(marked(&rows).iter().all(|row| !row.ends_with('*')));
        assert_eq!(board.selected_url(now()), None);
    }

    /// The text of the one selected row, or `None` when no row is selected.
    fn selected_text(board: &mut Board) -> Option<String> {
        let rows = board.rows(now());
        let found: Vec<String> = marked(&rows)
            .into_iter()
            .filter(|row| row.ends_with('*'))
            .collect();
        assert!(found.len() <= 1, "more than one selected row: {found:?}");
        found.into_iter().next()
    }

    fn four_runs_in_two_sections() -> Board {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Running, 30, 1),
                run(2, "a/x", RunState::Queued, 20, 1),
                run(3, "a/x", RunState::Passed, 60, 10),
                run(4, "a/x", RunState::Failed, 60, 20),
            ],
        );
        board
    }

    #[test]
    fn select_next_moves_through_one_list_and_stops_at_the_end() {
        let mut board = four_runs_in_two_sections();
        let mut seen = Vec::new();
        for _ in 0..5 {
            seen.push(selected_text(&mut board).expect("a selection"));
            board.select_next(now());
        }
        assert_eq!(
            seen,
            ["Run(1)*", "Run(2)*", "Run(3)*", "Run(4)*", "Run(4)*"]
        );
    }

    #[test]
    fn a_repeated_run_id_shows_one_row() {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                run(5, "a/x", RunState::Running, 10, 1),
                run(5, "a/x", RunState::Running, 10, 1),
                run(4, "a/x", RunState::Running, 5, 1),
            ],
        );
        let rows = board.rows(now());
        let shown = shape(&rows);
        assert_eq!(shown.iter().filter(|row| *row == "Run(5)").count(), 1);
        assert_eq!(shown[0], "Header(Active,2,0,0)");
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(selected_text(&mut board).expect("a selection"));
            board.select_next(now());
        }
        assert_eq!(seen, ["Run(5)*", "Run(4)*", "Run(4)*"]);
    }

    #[test]
    fn select_previous_moves_back_through_one_list_and_stops_at_the_start() {
        let mut board = four_runs_in_two_sections();
        for _ in 0..3 {
            board.select_next(now());
        }
        let mut seen = Vec::new();
        for _ in 0..5 {
            seen.push(selected_text(&mut board).expect("a selection"));
            board.select_previous(now());
        }
        assert_eq!(
            seen,
            ["Run(4)*", "Run(3)*", "Run(2)*", "Run(1)*", "Run(1)*"]
        );
    }

    #[test]
    fn the_selection_moves_on_an_empty_board_without_a_selection() {
        let mut board = board();
        board.select_next(now());
        board.select_previous(now());
        assert_eq!(selected_text(&mut board), None);
    }

    #[test]
    fn selected_url_is_the_url_of_the_selected_run() {
        let mut board = four_runs_in_two_sections();
        assert_eq!(
            board.selected_url(now()).as_deref(),
            Some("https://example.test/a/x/runs/1")
        );
        board.select_next(now());
        board.select_next(now());
        assert_eq!(
            board.selected_url(now()).as_deref(),
            Some("https://example.test/a/x/runs/3")
        );
    }

    #[test]
    fn the_selection_stays_on_its_run_when_runs_arrive_above_it() {
        let mut board = four_runs_in_two_sections();
        board.select_next(now());
        board.select_next(now());
        assert_eq!(selected_text(&mut board).as_deref(), Some("Run(3)*"));

        board.replace(
            repo("b/y"),
            vec![
                run(5, "b/y", RunState::Running, 90, 1),
                run(6, "b/y", RunState::Passed, 60, 2),
            ],
        );
        let rows = board.rows(now());
        assert_eq!(
            marked(&rows),
            [
                "Header(Active,3,0,0)",
                "Run(5)",
                "Run(1)",
                "Run(2)",
                "Header(Recent,3,2,1)",
                "Run(6)",
                "Run(3)*",
                "Run(4)",
            ]
        );
    }

    fn select_third(board: &mut Board) {
        board.select_next(now());
        board.select_next(now());
        assert_eq!(selected_text(board).as_deref(), Some("Run(3)*"));
    }

    #[test]
    fn when_the_selected_run_is_dropped_the_run_at_its_position_is_selected() {
        let mut board = four_runs_in_two_sections();
        select_third(&mut board);
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Running, 30, 1),
                run(2, "a/x", RunState::Queued, 20, 1),
                run(4, "a/x", RunState::Failed, 60, 20),
            ],
        );
        assert_eq!(selected_text(&mut board).as_deref(), Some("Run(4)*"));
    }

    #[test]
    fn when_the_selected_run_was_the_last_the_new_last_run_is_selected() {
        let mut board = four_runs_in_two_sections();
        for _ in 0..3 {
            board.select_next(now());
        }
        assert_eq!(selected_text(&mut board).as_deref(), Some("Run(4)*"));
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Running, 30, 1),
                run(2, "a/x", RunState::Queued, 20, 1),
                run(3, "a/x", RunState::Passed, 60, 10),
            ],
        );
        assert_eq!(selected_text(&mut board).as_deref(), Some("Run(3)*"));
    }

    #[test]
    fn when_the_selected_run_leaves_the_window_the_nearest_run_is_selected() {
        let mut board = four_runs_in_two_sections();
        select_third(&mut board);
        let later = now() + Span::hours(8) + Span::minutes(15);
        let rows = board.rows(later);
        assert_eq!(
            marked(&rows),
            [
                "Header(Active,2,0,0)",
                "Run(1)",
                "Run(2)*",
                "Header(Recent,0,0,0)",
                "Empty(Recent)",
            ]
        );
    }

    #[test]
    fn with_no_run_left_nothing_is_selected_and_the_first_run_is_selected_again_later() {
        let mut board = four_runs_in_two_sections();
        select_third(&mut board);
        board.replace(repo("a/x"), Vec::new());
        assert_eq!(selected_text(&mut board), None);
        assert_eq!(board.selected_url(now()), None);

        board.replace(
            repo("a/x"),
            vec![
                run(7, "a/x", RunState::Running, 30, 1),
                run(8, "a/x", RunState::Passed, 60, 10),
            ],
        );
        assert_eq!(selected_text(&mut board).as_deref(), Some("Run(7)*"));
    }

    #[test]
    fn a_tall_view_shows_all_rows_from_the_first() {
        let mut board = four_runs_in_two_sections();
        let all = marked(&board.rows(now()));
        let view = board.view(now(), 100);
        assert_eq!(view.first, 0);
        assert_eq!(marked(&view.rows), all);
        assert_eq!(all.len(), 6);
    }

    #[test]
    fn a_view_of_height_zero_has_no_rows() {
        let mut board = four_runs_in_two_sections();
        let view = board.view(now(), 0);
        assert!(view.rows.is_empty());
        assert_eq!(view.first, 0);
        let mut empty = Board::new(Duration::from_secs(HOUR));
        assert!(empty.view(now(), 0).rows.is_empty());
    }

    #[test]
    fn a_short_view_shows_the_first_rows() {
        let mut board = four_runs_in_two_sections();
        let view = board.view(now(), 3);
        assert_eq!(view.first, 0);
        assert_eq!(
            marked(&view.rows),
            ["Header(Active,2,0,0)", "Run(1)*", "Run(2)"]
        );
    }

    /// A board with `active` running runs (ids 1.., oldest first) and `recent`
    /// passed runs (ids 101.., newest first).
    fn long_board(active: u64, recent: u64) -> Board {
        let mut board = board();
        let mut runs = Vec::new();
        for id in 1..=active {
            let started = 200 - i64::try_from(id).expect("a small id");
            runs.push(run(id, "a/x", RunState::Running, started, 1));
        }
        for id in 101..101 + recent {
            let finished = i64::try_from(id).expect("a small id") - 100;
            runs.push(run(id, "a/x", RunState::Passed, 300, finished));
        }
        board.replace(repo("a/x"), runs);
        board
    }

    /// Moves the selection with `step` and reads the scroll offset after each
    /// move. The selected row must be inside the view each time.
    fn offsets_while(
        board: &mut Board,
        height: usize,
        moves: usize,
        step: fn(&mut Board, DateTime<Utc>),
    ) -> Vec<usize> {
        let mut offsets = Vec::new();
        for _ in 0..moves {
            step(board, now());
            let view = board.view(now(), height);
            assert!(
                marked(&view.rows).iter().any(|row| row.ends_with('*')),
                "the selected row is outside the view at first = {}",
                view.first
            );
            offsets.push(view.first);
        }
        offsets
    }

    #[test]
    fn moving_the_selection_down_scrolls_by_the_least_amount() {
        let mut board = long_board(10, 0);
        assert_eq!(board.view(now(), 4).first, 0);
        let offsets = offsets_while(&mut board, 4, 6, Board::select_next);
        assert_eq!(offsets, [0, 0, 1, 2, 3, 4]);
    }

    #[test]
    fn moving_the_selection_up_scrolls_by_the_least_amount() {
        let mut board = long_board(10, 0);
        let down = offsets_while(&mut board, 4, 5, Board::select_next);
        assert_eq!(down.last(), Some(&3));
        let up = offsets_while(&mut board, 4, 4, Board::select_previous);
        assert_eq!(up, [3, 3, 3, 2]);
    }

    #[test]
    fn the_header_above_the_selected_row_stays_in_view() {
        let mut board = long_board(6, 3);
        let down = offsets_while(&mut board, 3, 8, Board::select_next);
        assert_eq!(down, [0, 1, 2, 3, 4, 6, 7, 8]);
        let up = offsets_while(&mut board, 3, 3, Board::select_previous);
        assert_eq!(up, [8, 7, 6]);
    }

    #[test]
    fn a_view_of_one_row_cannot_keep_the_header_too() {
        let mut board = long_board(6, 3);
        for _ in 0..6 {
            board.select_next(now());
        }
        let view = board.view(now(), 1);
        assert_eq!(view.first, 8);
        assert_eq!(marked(&view.rows), ["Run(101)*"]);
    }

    #[test]
    fn a_taller_view_shows_more_rows_above_instead_of_blank_rows_below() {
        let mut board = long_board(10, 0);
        for _ in 0..9 {
            board.select_next(now());
        }
        assert_eq!(board.view(now(), 4).first, 7);

        let taller = board.view(now(), 8);
        assert_eq!(taller.first, 5);
        assert_eq!(taller.rows.len(), 8);
        assert!(marked(&taller.rows).contains(&"Run(10)*".to_owned()));

        let everything = board.view(now(), 100);
        assert_eq!(everything.first, 0);
        assert_eq!(everything.rows.len(), 13);
    }

    #[test]
    fn a_board_that_shrinks_pulls_the_offset_back() {
        let mut board = long_board(10, 0);
        for _ in 0..9 {
            board.select_next(now());
        }
        assert_eq!(board.view(now(), 4).first, 7);
        board.replace(repo("a/x"), vec![run(1, "a/x", RunState::Running, 5, 1)]);
        let view = board.view(now(), 4);
        assert_eq!(view.first, 0);
        assert_eq!(view.rows.len(), 4);
    }

    /// A board with run 1 (running) and run 2 (passed) in repository `a/x`.
    fn two_run_board() -> Board {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Running, 30, 1),
                run(2, "a/x", RunState::Passed, 60, 10),
            ],
        );
        board
    }

    #[test]
    fn expanding_the_selected_run_shows_a_loading_row_below_it() {
        let mut board = two_run_board();
        board.expand(now());
        assert_eq!(
            marked(&board.rows(now())),
            [
                "Header(Active,1,0,0)",
                "Run(1)*",
                "JobsLoading",
                "Header(Recent,1,1,0)",
                "Run(2)",
            ]
        );
    }

    #[test]
    fn set_jobs_shows_one_row_per_job_and_marks_the_last() {
        let mut board = two_run_board();
        board.expand(now());
        board.set_jobs(
            RunId(1),
            vec![
                job(11, RunState::Passed),
                job(12, RunState::Running),
                job(13, RunState::Queued),
            ],
        );
        assert_eq!(
            shape(&board.rows(now()))[1..6],
            [
                "Run(1)",
                "Job(11)",
                "Job(12)",
                "LastJob(13)",
                "Header(Recent,1,1,0)"
            ]
        );
    }

    /// GitHub gives the jobs in the order of their IDs, which has no meaning
    /// that a person can see. The board sorts them by name, with no difference
    /// between capital and small letters, and then by ID.
    #[test]
    fn set_jobs_sorts_the_jobs_by_name_and_then_by_id() {
        let mut board = two_run_board();
        board.expand(now());
        board.set_jobs(
            RunId(1),
            vec![
                named_job(21, "test (stable)"),
                named_job(22, "build"),
                named_job(23, "Lint"),
                named_job(24, "Build"),
                named_job(25, "deploy"),
                named_job(26, "test (beta)"),
            ],
        );
        assert_eq!(
            shape(&board.rows(now()))[2..8],
            [
                "Job(22)",
                "Job(24)",
                "Job(25)",
                "Job(23)",
                "Job(26)",
                "LastJob(21)"
            ]
        );
    }

    /// A re-run gives a job a new ID. The job keeps its row, because the
    /// order is by name.
    #[test]
    fn a_re_run_job_with_a_new_id_keeps_its_row() {
        let mut board = two_run_board();
        board.expand(now());
        board.set_jobs(
            RunId(1),
            vec![
                named_job(31, "build"),
                named_job(32, "lint"),
                named_job(33, "test"),
            ],
        );
        board.set_jobs(
            RunId(1),
            vec![
                named_job(31, "build"),
                named_job(33, "test"),
                named_job(39, "lint"),
            ],
        );
        assert_eq!(
            shape(&board.rows(now()))[2..5],
            ["Job(31)", "Job(39)", "LastJob(33)"]
        );
    }

    #[test]
    fn an_answer_with_no_job_shows_a_no_jobs_row() {
        let mut board = two_run_board();
        board.expand(now());
        board.set_jobs(RunId(1), Vec::new());
        assert_eq!(shape(&board.rows(now()))[1..3], ["Run(1)", "NoJobs"]);
    }

    #[test]
    fn collapse_removes_the_block_and_the_jobs() {
        let mut board = two_run_board();
        board.expand(now());
        board.set_jobs(RunId(1), vec![job(11, RunState::Passed)]);
        board.collapse(now());
        assert_eq!(
            shape(&board.rows(now())),
            [
                "Header(Active,1,0,0)",
                "Run(1)",
                "Header(Recent,1,1,0)",
                "Run(2)"
            ]
        );
        assert!(board.expanded(now()).is_empty());
        board.expand(now());
        assert_eq!(shape(&board.rows(now()))[2], "JobsLoading");
    }

    #[test]
    fn expanding_a_run_twice_keeps_its_jobs() {
        let mut board = two_run_board();
        board.expand(now());
        board.set_jobs(RunId(1), vec![job(11, RunState::Passed)]);
        board.expand(now());
        assert_eq!(shape(&board.rows(now()))[1..3], ["Run(1)", "LastJob(11)"]);
    }

    #[test]
    fn collapse_of_a_run_that_is_not_expanded_changes_nothing() {
        let mut board = two_run_board();
        let before = shape(&board.rows(now()));
        board.collapse(now());
        assert_eq!(shape(&board.rows(now())), before);
        assert!(board.expanded(now()).is_empty());
    }

    #[test]
    fn expand_and_collapse_on_an_empty_board_do_nothing() {
        let mut board = board();
        board.expand(now());
        board.collapse(now());
        assert!(board.expanded(now()).is_empty());
        assert_eq!(shape(&board.rows(now())).len(), 4);
    }

    #[test]
    fn jobs_for_a_run_that_is_not_expanded_are_dropped() {
        let mut board = two_run_board();
        board.set_jobs(RunId(1), vec![job(11, RunState::Passed)]);
        board.fail_jobs(RunId(1), "HTTP 500".to_owned());
        board.set_jobs(RunId(99), vec![job(12, RunState::Passed)]);
        assert_eq!(shape(&board.rows(now())).len(), 4);
        board.expand(now());
        assert_eq!(shape(&board.rows(now()))[2], "JobsLoading");
    }

    #[test]
    fn more_than_one_run_can_be_expanded() {
        let mut board = two_run_board();
        board.expand(now());
        board.set_jobs(RunId(1), vec![job(11, RunState::Passed)]);
        board.select_next(now());
        board.expand(now());
        assert_eq!(
            marked(&board.rows(now())),
            [
                "Header(Active,1,0,0)",
                "Run(1)",
                "LastJob(11)",
                "Header(Recent,1,1,0)",
                "Run(2)*",
                "JobsLoading",
            ]
        );
    }

    #[test]
    fn up_and_down_skip_the_job_rows() {
        let mut board = two_run_board();
        board.expand(now());
        board.set_jobs(RunId(1), vec![job(11, RunState::Passed)]);
        board.select_next(now());
        assert_eq!(selected_text(&mut board).as_deref(), Some("Run(2)*"));
        board.select_previous(now());
        assert_eq!(selected_text(&mut board).as_deref(), Some("Run(1)*"));
    }

    #[test]
    fn expanded_gives_the_repository_and_the_id_of_each_expanded_run() {
        let mut board = board();
        board.replace(repo("a/x"), vec![run(1, "a/x", RunState::Running, 30, 1)]);
        board.replace(repo("b/y"), vec![run(2, "b/y", RunState::Running, 20, 1)]);
        board.expand(now());
        board.select_next(now());
        board.expand(now());
        let expected: BTreeSet<(RepoName, RunId)> =
            [(repo("a/x"), RunId(1)), (repo("b/y"), RunId(2))]
                .into_iter()
                .collect();
        assert_eq!(board.expanded(now()), expected);
    }

    #[test]
    fn a_failed_fetch_after_an_answer_keeps_the_jobs_and_adds_the_error() {
        let mut board = two_run_board();
        board.expand(now());
        board.set_jobs(
            RunId(1),
            vec![job(11, RunState::Passed), job(12, RunState::Running)],
        );
        board.fail_jobs(RunId(1), "HTTP 500".to_owned());
        assert_eq!(
            shape(&board.rows(now()))[1..5],
            ["Run(1)", "Job(11)", "Job(12)", "JobsError(HTTP 500)"]
        );
        board.fail_jobs(RunId(1), "timed out".to_owned());
        assert_eq!(
            shape(&board.rows(now()))[1..5],
            ["Run(1)", "Job(11)", "Job(12)", "JobsError(timed out)"]
        );
    }

    #[test]
    fn a_failed_fetch_before_any_answer_shows_the_error_only() {
        let mut board = two_run_board();
        board.expand(now());
        board.fail_jobs(RunId(1), "HTTP 404".to_owned());
        assert_eq!(
            shape(&board.rows(now()))[1..3],
            ["Run(1)", "JobsError(HTTP 404)"]
        );
    }

    #[test]
    fn a_later_answer_clears_the_error() {
        let mut board = two_run_board();
        board.expand(now());
        board.set_jobs(RunId(1), vec![job(11, RunState::Passed)]);
        board.fail_jobs(RunId(1), "HTTP 500".to_owned());
        board.set_jobs(RunId(1), vec![job(13, RunState::Passed)]);
        assert_eq!(shape(&board.rows(now()))[1..3], ["Run(1)", "LastJob(13)"]);
    }

    #[test]
    fn a_run_that_moves_to_the_recent_section_stays_expanded() {
        let mut board = two_run_board();
        board.expand(now());
        board.set_jobs(RunId(1), vec![job(11, RunState::Passed)]);
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Passed, 30, 1),
                run(2, "a/x", RunState::Passed, 60, 10),
            ],
        );
        assert_eq!(
            marked(&board.rows(now())),
            [
                "Header(Active,0,0,0)",
                "Empty(Active)",
                "Header(Recent,2,2,0)",
                "Run(1)*",
                "LastJob(11)",
                "Run(2)",
            ]
        );
        assert_eq!(board.expanded(now()).len(), 1);
    }

    #[test]
    fn a_run_that_replace_drops_loses_its_expansion() {
        let mut board = two_run_board();
        board.expand(now());
        board.set_jobs(RunId(1), vec![job(11, RunState::Passed)]);
        board.replace(repo("a/x"), Vec::new());
        assert!(board.expanded(now()).is_empty());
        board.replace(repo("a/x"), vec![run(1, "a/x", RunState::Running, 30, 1)]);
        assert_eq!(shape(&board.rows(now())).len(), 4);
    }

    #[test]
    fn a_repository_that_keep_only_drops_loses_its_expansion() {
        let mut board = two_run_board();
        board.expand(now());
        board.keep_only(&BTreeSet::new());
        assert!(board.expanded(now()).is_empty());
        board.replace(repo("a/x"), vec![run(1, "a/x", RunState::Running, 30, 1)]);
        assert_eq!(shape(&board.rows(now())).len(), 4);
    }

    #[test]
    fn a_run_that_leaves_the_window_loses_its_expansion() {
        let mut board = two_run_board();
        board.select_next(now());
        board.expand(now());
        let later = now() + Span::hours(8) + Span::minutes(15);
        assert!(board.expanded(later).is_empty());
        assert_eq!(
            shape(&board.rows(now())),
            [
                "Header(Active,1,0,0)",
                "Run(1)",
                "Header(Recent,1,1,0)",
                "Run(2)"
            ]
        );
    }

    #[test]
    fn a_run_that_becomes_other_loses_its_expansion() {
        let mut board = two_run_board();
        board.expand(now());
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Other, 30, 1),
                run(2, "a/x", RunState::Passed, 60, 10),
            ],
        );
        assert!(board.expanded(now()).is_empty());
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Running, 30, 1),
                run(2, "a/x", RunState::Passed, 60, 10),
            ],
        );
        assert_eq!(shape(&board.rows(now())).len(), 4);
    }

    /// Gives run `id` a block of `count` jobs. The run must be selected.
    fn expand_with_jobs(board: &mut Board, id: u64, count: u64) {
        board.expand(now());
        let jobs = (1..=count)
            .map(|n| job(id * 100 + n, RunState::Passed))
            .collect();
        board.set_jobs(RunId(id), jobs);
    }

    #[test]
    fn an_expanded_run_at_the_bottom_of_the_view_scrolls_to_show_its_jobs() {
        let mut board = long_board(6, 0);
        for _ in 0..5 {
            board.select_next(now());
        }
        expand_with_jobs(&mut board, 6, 2);
        let view = board.view(now(), 4);
        assert_eq!(view.first, 5);
        assert_eq!(
            marked(&view.rows),
            ["Run(5)", "Run(6)*", "Job(601)", "LastJob(602)"]
        );
    }

    #[test]
    fn many_jobs_keep_the_run_and_its_header_in_view() {
        let mut board = long_board(3, 3);
        for _ in 0..3 {
            board.select_next(now());
        }
        assert_eq!(board.view(now(), 4).first, 2);
        expand_with_jobs(&mut board, 101, 10);
        let view = board.view(now(), 4);
        assert_eq!(view.first, 4);
        assert_eq!(
            marked(&view.rows),
            [
                "Header(Recent,3,3,0)",
                "Run(101)*",
                "Job(10101)",
                "Job(10102)"
            ]
        );
    }

    #[test]
    fn a_view_of_one_row_shows_the_run_row_and_not_its_jobs() {
        let mut board = long_board(3, 3);
        for _ in 0..3 {
            board.select_next(now());
        }
        expand_with_jobs(&mut board, 101, 10);
        let view = board.view(now(), 1);
        assert_eq!(view.first, 5);
        assert_eq!(marked(&view.rows), ["Run(101)*"]);
    }
}
